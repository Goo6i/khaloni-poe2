//! Windows overlay stub — the real click-through overlay window lands in
//! SP3. Public API mirrors platform/linux/overlay.rs exactly.

use tiny_skia::Pixmap;

pub use crate::platform::Key;

pub struct Overlay;

impl Overlay {
    pub fn new(_target_center: (i32, i32)) -> anyhow::Result<Overlay> {
        anyhow::bail!("windows backend lands in SP3")
    }

    pub fn set_interactive(&mut self, _rect: Option<(i32, i32, u32, u32)>) -> anyhow::Result<()> {
        anyhow::bail!("windows backend lands in SP3")
    }

    pub fn take_clicks(&mut self) -> Vec<(i32, i32)> {
        Vec::new()
    }

    pub fn pointer_pos(&self) -> (i32, i32) {
        (0, 0)
    }

    pub fn button_down(&self) -> bool {
        false
    }

    pub fn take_keys(&mut self) -> Vec<Key> {
        Vec::new()
    }

    pub fn set_keyboard(&mut self, _on: bool) -> anyhow::Result<()> {
        anyhow::bail!("windows backend lands in SP3")
    }

    pub fn pump(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("windows backend lands in SP3")
    }

    pub fn present(&mut self, _pixmap: &Pixmap) -> anyhow::Result<()> {
        anyhow::bail!("windows backend lands in SP3")
    }

    pub fn hide(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("windows backend lands in SP3")
    }

    pub fn size(&self) -> (u32, u32) {
        (0, 0)
    }

    pub fn output_pos(&self) -> (i32, i32) {
        (0, 0)
    }
}
