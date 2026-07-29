//! Windows global-hotkey stub — the real backend lands in SP3. Public API
//! mirrors platform/linux/hotkeys.rs.

pub use crate::platform::Hotkey;

pub async fn listen(
    _tx: std::sync::mpsc::Sender<Hotkey>,
    _price_check: String,
    _overlay: String,
    _extra: Vec<(String, String)>,
) -> anyhow::Result<()> {
    anyhow::bail!("windows backend lands in SP3")
}
