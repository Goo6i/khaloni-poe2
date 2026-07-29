//! Live X11 test for the CLIPBOARD ownership watcher. Requires a running X
//! server on $DISPLAY with xclip; skips cleanly otherwise so it does not fail
//! in a headless CI.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use poe2_lens::clipwatch::ClipboardWatcher;

#[test]
fn detects_a_real_copy_and_times_out_on_none() {
    if Command::new("xclip").arg("-version").output().is_err() {
        eprintln!("skip: xclip unavailable");
        return;
    }
    let Some(w) = ClipboardWatcher::new() else {
        eprintln!("skip: no X/XFIXES");
        return;
    };

    // A copy (xclip taking CLIPBOARD ownership) must be detected. Null xclip's
    // stdio: it daemonizes to serve the selection and would otherwise hold the
    // test's captured-output pipe open.
    w.drain();
    let Ok(mut child) = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        eprintln!("skip: could not spawn xclip");
        return;
    };
    child.stdin.take().unwrap().write_all(b"Item Class: Test\nHi\n").unwrap();
    assert!(
        w.wait_for_change(Duration::from_secs(2)),
        "an actual clipboard copy must fire an ownership change"
    );

    // With no copy, the watcher must time out (the misclick case).
    w.drain();
    assert!(
        !w.wait_for_change(Duration::from_millis(300)),
        "no copy must not report a change"
    );
    let _ = child.kill();
}
