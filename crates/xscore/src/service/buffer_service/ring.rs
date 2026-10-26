// service/buffer_service/ring.rs - Buffer-owned byte history
//
// The pane's core is its buffer: a fixed-capacity byte history that
// survives runtime churn (detach keeps the bytes). It belongs to the pane, not
// to the connection layer, which is why it lives here rather than beside the
// conduit (arch.md §9).
//
// `copy_out` is a *snapshot* read (like `tmux capture-pane`): it copies
// the most recent `min(len, out.len())` bytes in order and does NOT
// consume them. A pane's buffer is scrollback, not a stream — reading
// twice must not lose the screen's content.
use std::collections::VecDeque;

/// Default capacity of a pane's buffer (64 KiB of scrollback).
pub const DIRECT_BUFFER_CAPACITY: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct RingBuffer {
    data: VecDeque<u8>,
    capacity: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(capacity.min(4096)),
            capacity,
        }
    }

    /// Append one byte, evicting the oldest when full.
    pub fn push(&mut self, b: u8) {
        if self.capacity == 0 {
            return;
        }
        if self.data.len() == self.capacity {
            self.data.pop_front();
        }
        self.data.push_back(b);
    }

    pub fn push_slice(&mut self, bs: &[u8]) {
        for b in bs {
            self.push(*b);
        }
    }

    /// Copy the newest bytes into `out` (oldest-first) and return the
    /// number copied. Non-destructive.
    pub fn copy_out(&self, out: &mut [u8]) -> usize {
        let n = self.data.len().min(out.len());
        if n == 0 {
            return 0;
        }
        let skip = self.data.len() - n;
        for (dst, src) in out[..n].iter_mut().zip(self.data.iter().skip(skip)) {
            *dst = *src;
        }
        n
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn clear(&mut self) {
        self.data.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_copy_out_roundtrip() {
        let mut rb = RingBuffer::new(8);
        rb.push_slice(b"hello");
        let mut out = [0u8; 8];
        assert_eq!(rb.copy_out(&mut out), 5);
        assert_eq!(&out[..5], b"hello");
    }

    #[test]
    fn overflow_keeps_newest_bytes() {
        let mut rb = RingBuffer::new(4);
        rb.push_slice(b"abcdef");
        assert_eq!(rb.len(), 4);
        let mut out = [0u8; 8];
        let n = rb.copy_out(&mut out);
        assert_eq!(&out[..n], b"cdef");
    }

    #[test]
    fn copy_out_is_non_destructive() {
        let mut rb = RingBuffer::new(8);
        rb.push_slice(b"abc");
        let mut out = [0u8; 8];
        assert_eq!(rb.copy_out(&mut out), 3);
        assert_eq!(
            rb.copy_out(&mut out),
            3,
            "second read must still see the bytes"
        );
        assert_eq!(rb.len(), 3);
    }

    #[test]
    fn small_out_window_gets_the_tail() {
        let mut rb = RingBuffer::new(8);
        rb.push_slice(b"abcdef");
        let mut out = [0u8; 2];
        assert_eq!(rb.copy_out(&mut out), 2);
        assert_eq!(&out, b"ef");
    }

    #[test]
    fn clear_empties() {
        let mut rb = RingBuffer::new(4);
        rb.push_slice(b"xy");
        rb.clear();
        assert!(rb.is_empty());
        assert_eq!(rb.copy_out(&mut [0u8; 4]), 0);
    }
}
