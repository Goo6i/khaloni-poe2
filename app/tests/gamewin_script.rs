#![cfg(target_os = "linux")]
use khaloni_poe2::platform::gamewin::{focus_from_kind, KWIN_SCRIPT};
use khaloni_poe2::platform::Focus;

#[test]
fn script_reports_geometry_focus_and_close() {
    for needle in [
        "callDBus",
        "org.khalonipoe2.App",
        "/org/khalonipoe2/App",
        "Geometry",
        "Active",
        "Math.round",
        "frameGeometryChanged",
        "captionChanged",
        "Visible",
        "stackingOrder",
        "windowActivated",
        "closed",
        "path of exile",
        "WantsKeyboard",
        "lastGame",
        "windowList()",
    ] {
        assert!(KWIN_SCRIPT.contains(needle), "script missing {needle}");
    }
}

/// The app never re-parses captions: only the script's own three words
/// count, and everything else is another window.
#[test]
fn focus_is_only_what_the_script_classified() {
    assert_eq!(focus_from_kind("game"), Focus::Game);
    assert_eq!(focus_from_kind("overlay"), Focus::Overlay);
    assert_eq!(focus_from_kind("other"), Focus::Other);
    for not_the_game in [
        "",
        "Game",
        "Path of Exile 2 Wiki - Mozilla Firefox firefox",
        "ELDEN RING steam_app_1245620",
        "Some Game gamescope",
        "Path of Exile 2 steam_app_2694490",
        "notes khaloni-poe2",
    ] {
        assert_eq!(focus_from_kind(not_the_game), Focus::Other, "{not_the_game:?}");
    }
    // The script sends the word, never a caption.
    assert!(KWIN_SCRIPT.contains(r#""Active", kind"#));
    assert!(!KWIN_SCRIPT.contains("w.caption + "));
}

/// Mocks of the KWin scripting globals, then scenarios run against the real
/// script text. `node` is the only JavaScript engine this needs.
const HARNESS: &str = r#"
const fs = require("fs");
const assert = require("assert");
function Signal() { this.fns = []; }
Signal.prototype.connect = function (f) { this.fns.push(f); };
Signal.prototype.emit = function () { for (const f of this.fns.slice()) f.apply(null, arguments); };
function Win(caption, cls, x, y, w, h, opts) {
    this.caption = caption; this.resourceClass = cls;
    this.frameGeometry = { x: x, y: y, width: w, height: h };
    this.minimized = false; this.normalWindow = true;
    this.frameGeometryChanged = new Signal(); this.closed = new Signal();
    if (!(opts && opts.noCaptionSignal)) this.captionChanged = new Signal();
    this.windowClassChanged = new Signal();
}
const calls = [];
const windows = [];
const timers = [];
globalThis.workspace = {
    windowList: function () { return windows.slice(); },
    windowAdded: new Signal(), windowActivated: new Signal(),
    activeWindow: null, cursorPos: { x: 0, y: 0 },
    get stackingOrder() { return windows.slice(); },
};
globalThis.callDBus = function () { calls.push(Array.prototype.slice.call(arguments, 3)); };
globalThis.registerShortcut = function () {};
globalThis.QTimer = function () { this.timeout = new Signal(); this.start = function () {}; timers.push(this); };
function add(w) { windows.push(w); workspace.windowAdded.emit(w); }
function close(w) { windows.splice(windows.indexOf(w), 1); w.closed.emit(); }
function activate(w) { workspace.activeWindow = w; workspace.windowActivated.emit(w); }
function tick(n) { for (let i = 0; i < n; ++i) timers[0].timeout.emit(); }
function last(name) { for (let i = calls.length - 1; i >= 0; --i) if (calls[i][0] === name) return calls[i].slice(1); return null; }
function count(name) { return calls.filter(function (c) { return c[0] === name; }).length; }

// A browser tab about the game is open before the script loads.
const wiki = new Win("Path of Exile 2 Wiki - Mozilla Firefox", "firefox", 0, 0, 800, 600);
windows.push(wiki);
workspace.activeWindow = wiki;
(0, eval)(fs.readFileSync(process.argv[2], "utf8"));

assert.strictEqual(count("Geometry"), 0, "a browser tab is not the game");
assert.deepStrictEqual(last("Active"), ["other"]);

// Another Proton game and a stray gamescope window are not the game.
const other = new Win("ELDEN RING", "steam_app_1245620", 0, 0, 1920, 1080);
add(other); activate(other);
const scope = new Win("Some Game", "gamescope", 0, 0, 1920, 1080);
add(scope); activate(scope);
assert.strictEqual(count("Geometry"), 0);
assert.deepStrictEqual(last("Active"), ["other"]);

// The game's window appears unnamed and gets its caption afterwards.
const game = new Win("", "steam_app_2694490", 2560, 0, 2560, 1440);
add(game); activate(game);
assert.strictEqual(count("Geometry"), 0, "an unnamed window is not yet the game");
assert.deepStrictEqual(last("Active"), ["other"]);
game.caption = "Path of Exile 2";
game.captionChanged.emit();
assert.deepStrictEqual(last("Geometry"), [2560, 0, 2560, 1440]);
assert.deepStrictEqual(last("Active"), ["game"]);

// Its geometry is followed from then on.
game.frameGeometry = { x: 0, y: 0, width: 1280.4, height: 720 };
game.frameGeometryChanged.emit();
assert.deepStrictEqual(last("Geometry"), [0, 0, 1280, 720]);

// Our own layer surface is the overlay, anything else is other.
const ours = new Win("", "khaloni-poe2", 0, 0, 2560, 1440);
add(ours); activate(ours);
assert.deepStrictEqual(last("Active"), ["overlay"]);
activate(wiki);
assert.deepStrictEqual(last("Active"), ["other"]);

// A second game window without a caption signal is found by the rescan.
const second = new Win("", "steam_app_2694490", 10, 20, 640, 480, { noCaptionSignal: true });
add(second);
second.caption = "Path of Exile 2";
const before = count("Geometry");
tick(10);
assert.strictEqual(count("Geometry"), before + 1);
assert.deepStrictEqual(last("Geometry"), [10, 20, 640, 480]);

// One of two closing re-anchors on the survivor; the last one is "gone".
close(second);
assert.deepStrictEqual(last("Geometry"), [0, 0, 1280, 720]);
close(game);
assert.deepStrictEqual(last("Geometry"), [0, 0, 0, 0]);
// The unrelated windows closing afterwards says nothing new.
const n = count("Geometry");
close(other);
assert.strictEqual(count("Geometry"), n);
console.log("ok");
"#;

#[test]
fn script_follows_late_captions_and_classifies_focus_itself() {
    let node = std::process::Command::new("node").arg("--version").output();
    if !node.is_ok_and(|o| o.status.success()) {
        eprintln!("skip: node unavailable");
        return;
    }
    let dir = std::env::temp_dir().join(format!("khaloni-poe2-script-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("script.js");
    let harness = dir.join("harness.js");
    std::fs::write(&script, KWIN_SCRIPT).unwrap();
    std::fs::write(&harness, HARNESS).unwrap();
    let out = std::process::Command::new("node").arg(&harness).arg(&script).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "script scenarios failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
