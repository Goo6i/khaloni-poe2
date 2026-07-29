#![cfg(target_os = "linux")]
use poe2_lens::platform::gamewin::KWIN_SCRIPT;

#[test]
fn script_reports_geometry_focus_and_close() {
    for needle in [
        "callDBus",
        "org.poe2lens.App",
        "/org/poe2lens/App",
        "Geometry",
        "Active",
        "Math.round",
        "frameGeometryChanged",
        "windowActivated",
        "closed",
        "path of exile",
    ] {
        assert!(KWIN_SCRIPT.contains(needle), "script missing {needle}");
    }
}
