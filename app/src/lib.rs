pub mod appraise_ui;
pub mod brightness;
pub mod leveling_ui;
pub mod platform;
pub mod reference_ui;
pub mod settings_ui;
pub mod tray;
pub mod config;
pub mod coord;
pub mod hover;
pub mod ocr;
pub mod popup_pos;
pub mod pricing;
pub mod prices;
pub mod refcache;
pub mod render;
pub mod rumours;
pub mod stabilize;
pub mod template;

// Thin re-export shims keeping the pre-`platform` module paths alive for
// one commit while main.rs and the tests still use them; the next commit
// moves every caller onto `platform::` and deletes these.
pub mod capture {
    pub use crate::platform::capture::*;
}
pub mod clipwatch {
    pub use crate::platform::clipwatch::*;
}
pub mod hotkeys {
    pub use crate::platform::hotkeys::*;
}
pub mod inject {
    pub use crate::platform::inject::*;
}
pub mod kwin {
    pub use crate::platform::gamewin::GameWindowFeed as GeometryFeed;
    pub use crate::platform::gamewin::KWIN_SCRIPT;
    pub use crate::platform::GameWindowEvent as KwinEvent;
}
pub mod overlay {
    pub use crate::platform::overlay::*;
}
