// transport/serial.rs
//
// Real serial-port transport. Opens the device at `path` with the
// requested baud/data_bits/parity/stop_bits on first `open()`, then
// exposes the underlying port via read/write/resize/close.
//
// The `serialport` crate's `SerialPort` is synchronous; to fit the
// async `Transport` trait, we wrap the port in `Arc<std::sync::Mutex<...>>`
// and dispatch the I/O to `tokio::task::spawn_blocking`. The async
// trait methods never block the executor thread.
//
// API notes for `serialport`:
//   `serialport::new(path, baud)` -> `SerialPortBuilder`.
//   `.data_bits(DataBits)`, `.parity(Parity)`, `.stop_bits(StopBits)`,
//   `.timeout(Duration)` are chainable on the builder.
//   `.open()` -> `Result<Box<dyn SerialPort>>`.
//   `SerialPort: io::Read + io::Write`, so `read(&mut [u8])` and
//   `write(&[u8])` are inherited from std::io.
//
// `read()` uses a oneshot channel to deliver the freshly-read bytes
// out of the blocking closure (which must be `'static`); the copy
// into the caller's buffer happens back on the executor.

use std::sync::{Arc, Mutex};

use crate::core::{Error, LinkAddress, Result};
use async_trait::async_trait;
use serialport::{DataBits, Parity, SerialPort, StopBits};
use tokio::sync::oneshot;

pub struct SerialTransport {
    pub path: String,
    pub baud: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: u8,
    /// Open port. `Mutex<Option<...>>` is the standard "empty before
    /// open" pattern; `Arc<...>` so `spawn_blocking` closures can
    /// capture it without taking any outer lock across `.await`.
    port: Arc<Mutex<Option<Box<dyn SerialPort>>>>,
}

impl SerialTransport {
    pub async fn new(
        path: String,
        baud: u32,
        data_bits: u8,
        parity: String,
        stop_bits: u8,
    ) -> Result<Self> {
        Ok(Self {
            path,
            baud,
            data_bits,
            parity,
            stop_bits,
            port: Arc::new(Mutex::new(None)),
        })
    }
}

#[async_trait]
impl super::Transport for SerialTransport {
    fn kind(&self) -> &'static str { "serial" }
    fn address(&self) -> LinkAddress { LinkAddress::Serial { path: self.path.clone() } }

    async fn open(&mut self) -> Result<()> {
        let parity = match self.parity.to_ascii_lowercase().as_str() {
            "n" | "none" => Parity::None,
            "o" | "odd" => Parity::Odd,
            "e" | "even" => Parity::Even,
            other => return Err(Error::Transport(format!("invalid parity: {other}"))),
        };
        let stop_bits = match self.stop_bits {
            1 => StopBits::One,
            2 => StopBits::Two,
            _ => {
                return Err(Error::Transport(format!(
                    "invalid stop_bits: {}",
                    self.stop_bits
                )))
            }
        };
        let data_bits = match self.data_bits {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            8 => DataBits::Eight,
            _ => {
                return Err(Error::Transport(format!(
                    "invalid data_bits: {}",
                    self.data_bits
                )))
            }
        };
        let port = serialport::new(&self.path, self.baud)
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(stop_bits)
            .timeout(std::time::Duration::from_millis(200))
            .open()
            .map_err(|e| Error::Transport(format!("open {}: {e}", self.path)))?;
        *self.port.lock().unwrap() = Some(port);
        Ok(())
    }

    async fn write(&self, data: &[u8]) -> Result<usize> {
        let port = self.port.clone();
        let data = data.to_vec();
        tokio::task::spawn_blocking(move || -> Result<usize> {
            let mut guard = port.lock().unwrap();
            let p = guard
                .as_mut()
                .ok_or_else(|| Error::Transport("not open".into()))?;
            use std::io::Write;
            p.write(&data)
                .map_err(|e| Error::Transport(format!("write: {e}")))
        })
        .await
        .map_err(|e| Error::Transport(format!("join: {e}")))?
    }

    async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        // Allocate a scratch buffer the same size as the caller's
        // destination; do the synchronous read into it inside
        // `spawn_blocking`; send the scratch buffer back via a
        // oneshot so we can copy the populated prefix into `buf` on
        // the executor thread.
        let port = self.port.clone();
        let len = buf.len();
        let (tx, rx) = oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let mut guard = port.lock().unwrap();
            let p = match guard.as_mut() {
                Some(p) => p,
                None => {
                    let _ = tx.send(Err(Error::Transport("not open".into())));
                    return;
                }
            };
            use std::io::Read;
            let mut tmp = vec![0u8; len];
            let res = match p.read(&mut tmp) {
                Ok(n) => Ok((n, tmp)),
                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => Ok((0, tmp)),
                Err(e) => Err(Error::Transport(format!("read: {e}"))),
            };
            let _ = tx.send(res);
        });
        let (n, tmp) = rx
            .await
            .map_err(|e| Error::Transport(format!("join: {e}")))??;
        if n > 0 {
            buf[..n].copy_from_slice(&tmp[..n]);
        }
        Ok(n)
    }

    async fn resize(&self, _: u16, _: u16) -> Result<()> { Ok(()) }

    async fn close(&self) -> Result<()> {
        // Drop the port; `SerialPort`'s `Drop` closes the underlying fd.
        *self.port.lock().unwrap() = None;
        Ok(())
    }
}
