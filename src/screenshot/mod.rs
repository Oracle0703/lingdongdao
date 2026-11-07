//! 截图：Windows 区域截图（`capture`、`service`）与跨平台的截图图库存储（`store`）。

pub mod library;
pub mod store;
#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
mod selection;
#[cfg(windows)]
mod service;
#[cfg(windows)]
pub use service::{unsaved, Capture, CaptureEvent};
#[cfg(windows)]
pub(crate) use service::{timing, SelectedImage};
