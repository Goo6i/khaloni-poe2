/// Emits the `ocr` cfg on every supported target. Linux links the real
/// leptess via pkg-config and Windows-MSVC (the shipped Windows build) via
/// vcpkg; the windows-gnu CHECK target gets the committed type-check stub
/// (app/leptess-stub) instead, so `cargo check --target
/// x86_64-pc-windows-gnu` covers the ENTIRE crate — including code that
/// only real Windows builds would otherwise compile. Anything else (no
/// leptess source at all) stays OCR-free.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(ocr)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os == "linux" || os == "windows" {
        println!("cargo::rustc-cfg=ocr");
    }
    if os == "linux" {
        // Steam runs launch options inside its legacy runtime, whose
        // LD_LIBRARY_PATH pins ancient libraries (libcurl among them) that
        // break the system libtesseract's version requirements — the
        // loader then kills the process before main() runs, which is fatal
        // for the --launch wrapper. DT_RPATH with old-style dtags outranks
        // LD_LIBRARY_PATH, so the system library directories win no matter
        // what environment Steam wraps around the binary. Both Arch- and
        // Debian-family lib dirs are listed; entries that do not exist on
        // a given distro are simply skipped.
        println!("cargo::rustc-link-arg=-Wl,--disable-new-dtags");
        println!(
            "cargo::rustc-link-arg=-Wl,-rpath,/usr/lib/x86_64-linux-gnu:/usr/lib:/lib/x86_64-linux-gnu"
        );
    }
}
