//! The item's art for the nameplate: GGG's CDN image for the first fetched
//! listing, downloaded once and kept on disk under the cache directory as
//! `art/<sha256 of the url>.png`. Decoding happens before the bytes are
//! kept, so a bad download is never cached; a cached file that no longer
//! decodes is replaced.

use std::path::{Path, PathBuf};

use image::RgbaImage;
use sha2::{Digest, Sha256};

pub fn cache_path(cache_dir: &Path, url: &str) -> PathBuf {
    let digest = Sha256::digest(url.as_bytes());
    let mut name = String::with_capacity(68);
    for b in digest {
        name.push_str(&format!("{b:02x}"));
    }
    name.push_str(".png");
    cache_dir.join("art").join(name)
}

/// The decoded image for `url`: from the cache when it decodes, else through
/// `fetch` (the bytes of a GET), kept on success.
pub fn load_or_fetch(
    cache_dir: &Path,
    url: &str,
    fetch: &dyn Fn(&str) -> Result<Vec<u8>, String>,
) -> Result<RgbaImage, String> {
    let path = cache_path(cache_dir, url);
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(img) = image::load_from_memory(&bytes) {
            return Ok(img.to_rgba8());
        }
    }
    let bytes = fetch(url)?;
    let img = image::load_from_memory(&bytes).map_err(|e| format!("art {url}: {e}"))?.to_rgba8();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("part");
    if std::fs::write(&tmp, &bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 200, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn the_cache_path_is_a_sha256_of_the_url_under_art() {
        let p = cache_path(Path::new("/c"), "https://x/y.png");
        assert_eq!(p.parent().unwrap(), Path::new("/c/art"));
        let name = p.file_name().unwrap().to_str().unwrap();
        assert!(name.ends_with(".png") && name.len() == 64 + 4, "{name}");
        assert_eq!(p, cache_path(Path::new("/c"), "https://x/y.png"));
        assert_ne!(p, cache_path(Path::new("/c"), "https://x/z.png"));
    }

    #[test]
    fn a_fetched_image_is_decoded_and_cached_so_the_second_load_does_not_fetch() {
        let dir = tempdir();
        let calls = Cell::new(0);
        let fetch = |_: &str| {
            calls.set(calls.get() + 1);
            Ok(png_bytes(4, 4))
        };
        let a = load_or_fetch(&dir, "https://x/a.png", &fetch).unwrap();
        assert_eq!((a.width(), a.height()), (4, 4));
        let b = load_or_fetch(&dir, "https://x/a.png", &fetch).unwrap();
        assert_eq!(a.as_raw(), b.as_raw());
        assert_eq!(calls.get(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_corrupt_cache_file_is_ignored_and_refetched() {
        let dir = tempdir();
        let path = cache_path(&dir, "https://x/c.png");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not a png").unwrap();
        let fetch = |_: &str| Ok(png_bytes(2, 2));
        let img = load_or_fetch(&dir, "https://x/c.png", &fetch).unwrap();
        assert_eq!((img.width(), img.height()), (2, 2));
        assert!(std::fs::read(&path).unwrap().len() > 9, "the good bytes replaced the bad file");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_fetch_is_an_error_and_writes_nothing() {
        let dir = tempdir();
        let fetch = |_: &str| Err("offline".to_string());
        assert!(load_or_fetch(&dir, "https://x/d.png", &fetch).is_err());
        assert!(!cache_path(&dir, "https://x/d.png").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn tempdir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("khaloni-art-test-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }
}
