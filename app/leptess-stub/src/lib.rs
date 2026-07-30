//! Type-check-only stand-in for `leptess` on the windows-gnu target, whose
//! native build script (tesseract/leptonica headers) cannot run there. It
//! exists so `cargo check --target x86_64-pc-windows-gnu` type-checks the
//! ENTIRE crate including the OCR pipeline — the real leptess links on
//! Linux and Windows-MSVC (the shipped targets); gnu is never shipped or
//! executed, only checked, so every body is unimplemented.

#[derive(Debug)]
pub struct StubError;

impl std::fmt::Display for StubError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "leptess stub: not a runnable target")
    }
}
impl std::error::Error for StubError {}

pub enum Variable {
    DebugFile,
    TesseditPagesegMode,
}

pub struct LepTess;

impl LepTess {
    pub fn new(_datapath: Option<&str>, _lang: &str) -> Result<LepTess, StubError> {
        unimplemented!("leptess stub is type-check only")
    }
    pub fn set_variable(&mut self, _v: Variable, _val: &str) -> Result<(), StubError> {
        unimplemented!("leptess stub is type-check only")
    }
    pub fn set_image_from_mem(&mut self, _png: &[u8]) -> Result<(), StubError> {
        unimplemented!("leptess stub is type-check only")
    }
    pub fn get_tsv_text(&mut self, _page: i32) -> Result<String, StubError> {
        unimplemented!("leptess stub is type-check only")
    }
}
