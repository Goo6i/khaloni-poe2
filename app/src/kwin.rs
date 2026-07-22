use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};

use crate::config::Rect;

pub const KWIN_SCRIPT: &str = r#"
function isGame(w) {
    if (!w) return false;
    var cap = (w.caption || "").toLowerCase();
    var cls = (w.resourceClass || "").toLowerCase();
    if (cap === "path of exile 2") return true;
    return (cls === "gamescope" || cls.indexOf("steam_app") === 0) && cap.indexOf("path of exile") !== -1;
}
function reportGeometry(w) {
    callDBus("org.poe2lens.App", "/org/poe2lens/App", "org.poe2lens.App", "Geometry",
             Math.round(w.frameGeometry.x), Math.round(w.frameGeometry.y),
             Math.round(w.frameGeometry.width), Math.round(w.frameGeometry.height));
}
function reportGone() {
    callDBus("org.poe2lens.App", "/org/poe2lens/App", "org.poe2lens.App", "Geometry", 0, 0, 0, 0);
}
function reportActive(w) {
    callDBus("org.poe2lens.App", "/org/poe2lens/App", "org.poe2lens.App", "Active",
             w ? (w.caption + " " + w.resourceClass) : "");
}
function hook(w) {
    if (!isGame(w)) return;
    reportGeometry(w);
    w.frameGeometryChanged.connect(function () { reportGeometry(w); });
    w.closed.connect(function () { reportGone(); });
}
for (const w of workspace.windowList()) hook(w);
workspace.windowAdded.connect(hook);
workspace.windowActivated.connect(function (w) { reportActive(w); });
reportActive(workspace.activeWindow);
// Cursor feed for popup anchoring + move-away dismissal: 100ms QTimer,
// pushed only when the pointer actually moved (>4px) so an idle desktop
// costs nothing on the bus.
var lastCx = -100000, lastCy = -100000;
var cursorTimer = new QTimer();
cursorTimer.interval = 100;
cursorTimer.timeout.connect(function () {
    var p = workspace.cursorPos;
    if (Math.abs(p.x - lastCx) > 4 || Math.abs(p.y - lastCy) > 4) {
        lastCx = p.x; lastCy = p.y;
        callDBus("org.poe2lens.App", "/org/poe2lens/App", "org.poe2lens.App", "Cursor",
                 Math.round(p.x), Math.round(p.y));
    }
});
cursorTimer.start();
"#;

pub enum KwinEvent {
    Geometry(Rect),
    /// True when the game window currently holds focus.
    Active(bool),
    GameGone,
    /// Live pointer position in global logical coordinates (throttled to
    /// 100ms and >4px moves by the KWin script).
    Cursor(i32, i32),
}

struct Service {
    tx: Sender<KwinEvent>,
}

#[zbus::interface(name = "org.poe2lens.App")]
impl Service {
    fn geometry(&self, x: i32, y: i32, w: i32, h: i32) {
        let ev = if w == 0 && h == 0 {
            KwinEvent::GameGone
        } else {
            KwinEvent::Geometry(Rect { x, y, w: w as u32, h: h as u32 })
        };
        let _ = self.tx.send(ev);
    }
    fn active(&self, caption: String) {
        let cap = caption.to_lowercase();
        let is_game = cap.starts_with("path of exile 2 ")
            || cap == "path of exile 2"
            || cap.contains(" gamescope")
            || cap.contains(" steam_app");
        let _ = self.tx.send(KwinEvent::Active(is_game));
    }
    fn cursor(&self, x: i32, y: i32) {
        let _ = self.tx.send(KwinEvent::Cursor(x, y));
    }
}

pub struct GeometryFeed {
    pub rx: Receiver<KwinEvent>,
    _conn: zbus::blocking::Connection,
    script_obj: Option<String>,
}

fn qdbus(args: &[&str]) -> Option<String> {
    let out = Command::new("qdbus6").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

impl GeometryFeed {
    pub fn start() -> anyhow::Result<GeometryFeed> {
        let (tx, rx) = channel();
        let conn = zbus::blocking::connection::Builder::session()?
            .name("org.poe2lens.App")?
            .serve_at("/org/poe2lens/App", Service { tx })?
            .build()?;

        let path = std::env::temp_dir().join("poe2-lens-kwin.js");
        std::fs::write(&path, KWIN_SCRIPT)?;
        let id = qdbus(&[
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting.loadScript",
            path.to_str().unwrap(),
        ])
        .ok_or_else(|| anyhow::anyhow!("kwin loadScript failed"))?;
        let obj = format!("/Scripting/Script{id}");
        let script_obj = if qdbus(&["org.kde.KWin", &obj, "org.kde.kwin.Script.run"]).is_some() {
            Some(obj)
        } else {
            let alt = format!("/{id}");
            qdbus(&["org.kde.KWin", &alt, "org.kde.kwin.Script.run"]).map(|_| alt)
        };
        anyhow::ensure!(script_obj.is_some(), "kwin script run failed");
        Ok(GeometryFeed { rx, _conn: conn, script_obj })
    }
}

impl Drop for GeometryFeed {
    fn drop(&mut self) {
        if let Some(obj) = self.script_obj.take() {
            let _ = qdbus(&["org.kde.KWin", &obj, "org.kde.kwin.Script.stop"]);
        }
    }
}
