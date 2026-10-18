// core/link.rs
use std::sync::Arc;
use tokio::sync::Mutex;
use super::id::{LinkId, WindowId};
use crate::model::LinkInfo;
use crate::sink::Sink;
use crate::transport::Transport;

pub struct Link {
    pub id: LinkId,
    pub info: LinkInfo,
    pub transport: Box<dyn Transport>,
    pub sink: Box<dyn Sink>,
    pub windows: Mutex<Vec<WindowId>>,
}

pub type ArcLink = Arc<Link>;
