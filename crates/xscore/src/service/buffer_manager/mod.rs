//! Buffer lifecycle, ring storage and the runtime bridge used by UI panes.

use std::sync::Arc;

pub mod bridge;
pub mod registry;

pub use crate::domain::buffer::{RingBuffer, DIRECT_BUFFER_CAPACITY};
pub use crate::domain::model::{BufferInfo, BufferOrigin};
pub use bridge::{ControlBridge, DaemonBridge, DirectBridge, PaneBridge};
pub use registry::BufferRegistry;

#[derive(Clone, Default)]
pub struct BufferManager {
    pub buffers: Arc<BufferRegistry>,
}

impl BufferManager {
    pub fn new() -> Self {
        Self::default()
    }
}
