//! Windows backend stubs: the same public items as platform/linux, with
//! bodies that bail (or empty event streams where a Receiver is expected)
//! until the real implementations land in SP3. They exist so the shared
//! code type-checks for x86_64-pc-windows against the platform-neutral
//! types in platform/mod.rs.
//!
//! OCR linking on Windows = SP3 packaging task: leptess (via leptonica-sys
//! and tesseract-sys) links the system tesseract/leptonica libraries
//! through pkg-config, which the windows-gnu cross check has no sysroot
//! for. leptess is therefore a Linux-only dependency, and the
//! OcrEngine-backed halves of ocr.rs and rumours.rs — plus their direct
//! users (main's headless/overlay modes, the scanimg bin) — are gated
//! behind cfg(target_os = "linux") until SP3 packages the libraries.
//! The pure parts of ocr.rs (OcrLine, band detection, TSV parsing, motion
//! tracking) stay portable.

pub mod capture;
pub mod gamewin;
pub mod hotkeys;
pub mod inject;
pub mod overlay;
