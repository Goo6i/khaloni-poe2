//! Windows capture stub — the real screen-capture backend lands in SP3.
//! Public API mirrors platform/linux/capture.rs; `CaptureStart` is
//! per-platform by design (the Linux one carries pipewire wiring).

use std::sync::{
    atomic::AtomicBool,
    mpsc::{Receiver, SyncSender},
    Arc,
};

use image::GrayImage;

use crate::config::Rect;

pub use crate::platform::RegionFrame;

pub struct CaptureStart;

pub async fn portal_session(_restore_token: Option<&str>) -> anyhow::Result<CaptureStart> {
    anyhow::bail!("windows backend lands in SP3")
}

pub fn consume(
    _start: CaptureStart,
    _region_rx: Receiver<Rect>,
    _region: Rect,
    _tx: SyncSender<RegionFrame>,
    _panel_open: Arc<AtomicBool>,
    _full_tx: Option<SyncSender<GrayImage>>,
) -> anyhow::Result<()> {
    anyhow::bail!("windows backend lands in SP3")
}
