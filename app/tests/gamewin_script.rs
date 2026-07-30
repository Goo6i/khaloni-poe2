#![cfg(target_os = "linux")]
use khaloni_poe2::platform::gamewin::KWIN_SCRIPT;

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
        "Visible",
        "stackingOrder",
        "windowActivated",
        "closed",
        "path of exile",
    ] {
        assert!(KWIN_SCRIPT.contains(needle), "script missing {needle}");
    }
}
