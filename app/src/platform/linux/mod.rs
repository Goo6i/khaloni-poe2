//! Linux backends: Wayland layer-shell overlay, pipewire/portal capture,
//! uinput injection, portal global shortcuts, KWin-scripted game-window
//! feed, and the X11 clipboard-ownership probe.

pub mod capture;
pub mod clipwatch;
pub mod gamewin;
pub mod hotkeys;
pub mod inject;
pub mod overlay;
