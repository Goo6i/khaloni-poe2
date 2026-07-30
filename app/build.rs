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
}
