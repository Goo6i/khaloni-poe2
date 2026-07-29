/// Emits the `ocr` cfg on targets where leptess (tesseract + leptonica)
/// can actually link: Linux via pkg-config, Windows-MSVC via vcpkg. The
/// windows-gnu cross-check target gets no OCR so it stays green without
/// native libs; the shipped Windows build is MSVC and carries full OCR.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(ocr)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "linux" || (os == "windows" && env == "msvc") {
        println!("cargo::rustc-cfg=ocr");
    }
}
