//! Windows game-window feed stub — the real window-tracking backend lands
//! in SP3. Public API mirrors platform/linux/gamewin.rs's facade.

use std::sync::mpsc::Receiver;

pub use crate::platform::GameWindowEvent;

pub struct GameWindowFeed {
    pub rx: Receiver<GameWindowEvent>,
}

impl GameWindowFeed {
    pub fn start() -> anyhow::Result<GameWindowFeed> {
        anyhow::bail!("windows backend lands in SP3")
    }
}

/// Platform-neutral facade, matching the Linux side.
pub fn start() -> anyhow::Result<GameWindowFeed> {
    GameWindowFeed::start()
}
