//! Learned-template identification: once OCR has confidently identified a
//! band, its text-region pixels become a template; later encounters of the
//! same reward are identified by normalized cross-correlation against the
//! stored strips in ~1 ms, bypassing tesseract entirely. Templates are the
//! game's own rendering, so matching is exact-by-construction across
//! sessions (same font, size, and antialiasing), and NCC's normalization
//! absorbs brightness differences between areas.

use image::{imageops, GrayImage};

/// Minimum correlation for a template hit. Same-content strips across
/// frames measure > 0.97 (see the corpus test); unrelated rewards measure
/// < 0.6. The gap is wide; 0.90 sits safely inside it.
pub const NCC_THRESHOLD: f64 = 0.90;

/// Coarse pass downscale factor (both axes); candidates within
/// COARSE_KEEP of the coarse best are re-scored at full resolution.
const COARSE_DOWN: u32 = 4;
const COARSE_KEEP: f64 = 0.08;

/// Height bucket tolerance: a template only competes for bands whose
/// height is within this fraction of its own.
const HEIGHT_TOL: f64 = 0.12;

const STORE_CAP: usize = 512;
const STORE_MAGIC: &[u8; 8] = b"P2LTPL01";

#[derive(Clone)]
pub struct Learned {
    /// Reproduces the priced row without OCR.
    pub item_key: String,
    pub count: u32,
    pub count_explicit: bool,
    strip: GrayImage,
    coarse: GrayImage,
    mean: f64,
    var: f64,
}

pub struct TemplateStore {
    entries: Vec<Learned>,
    pub dirty: bool,
}

fn stats(img: &GrayImage) -> (f64, f64) {
    let n = (img.width() * img.height()) as f64;
    let sum: f64 = img.as_raw().iter().map(|&p| f64::from(p)).sum();
    let mean = sum / n;
    let var: f64 = img.as_raw().iter().map(|&p| (f64::from(p) - mean).powi(2)).sum::<f64>() / n;
    (mean, var)
}

fn downscale(img: &GrayImage) -> GrayImage {
    imageops::resize(
        img,
        (img.width() / COARSE_DOWN).max(1),
        (img.height() / COARSE_DOWN).max(1),
        imageops::FilterType::Triangle,
    )
}

/// Normalized cross-correlation of `tpl` against `hay` at horizontal
/// offset `x0` (heights must match; the caller resizes). Returns [-1, 1].
fn ncc_at(tpl: &GrayImage, tpl_mean: f64, tpl_var: f64, hay: &GrayImage, x0: u32) -> f64 {
    let (tw, th) = (tpl.width(), tpl.height());
    let n = (tw * th) as f64;
    let mut sum = 0f64;
    let mut sum2 = 0f64;
    let mut cross = 0f64;
    let traw = tpl.as_raw();
    let hraw = hay.as_raw();
    let hw = hay.width() as usize;
    for y in 0..th as usize {
        let hrow = &hraw[y * hw + x0 as usize..y * hw + x0 as usize + tw as usize];
        let trow = &traw[y * tw as usize..(y + 1) * tw as usize];
        for (h, t) in hrow.iter().zip(trow) {
            let hv = f64::from(*h);
            sum += hv;
            sum2 += hv * hv;
            cross += hv * f64::from(*t);
        }
    }
    let hmean = sum / n;
    let hvar = sum2 / n - hmean * hmean;
    let denom = (tpl_var * hvar).sqrt();
    if denom < 1e-6 {
        return 0.0;
    }
    (cross / n - tpl_mean * hmean) / denom
}

/// Best NCC of `tpl` slid horizontally across `hay` (same height).
fn best_ncc(tpl: &GrayImage, tpl_mean: f64, tpl_var: f64, hay: &GrayImage) -> f64 {
    if hay.width() < tpl.width() || hay.height() != tpl.height() {
        return -1.0;
    }
    let mut best = -1.0f64;
    for x0 in 0..=(hay.width() - tpl.width()) {
        let s = ncc_at(tpl, tpl_mean, tpl_var, hay, x0);
        if s > best {
            best = s;
        }
    }
    best
}

impl TemplateStore {
    pub fn new() -> TemplateStore {
        TemplateStore { entries: Vec::new(), dirty: false }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Identifies a band's text-region crop. Coarse pass over downscaled
    /// strips prunes candidates; survivors re-score at full resolution.
    pub fn match_band(&self, crop: &GrayImage) -> Option<(&Learned, f64)> {
        let h = crop.height();
        let coarse_hay_cache: GrayImage = downscale(crop);
        let mut coarse: Vec<(usize, f64)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let dh = f64::from(e.strip.height()).max(1.0);
            if (f64::from(h) - dh).abs() / dh > HEIGHT_TOL {
                continue;
            }
            // Resize coarse haystack to the template's coarse height for
            // exact-height sliding.
            let hay = if coarse_hay_cache.height() == e.coarse.height() {
                coarse_hay_cache.clone()
            } else {
                imageops::resize(
                    crop,
                    (crop.width() * e.coarse.height() / h.max(1)).max(1),
                    e.coarse.height(),
                    imageops::FilterType::Triangle,
                )
            };
            let (cm, cv) = stats(&e.coarse);
            let s = best_ncc(&e.coarse, cm, cv, &hay);
            coarse.push((i, s));
        }
        let best_coarse = coarse.iter().cloned().fold(f64::MIN, |a, (_, s)| a.max(s));
        if best_coarse < NCC_THRESHOLD - COARSE_KEEP {
            return None;
        }
        let mut best: Option<(usize, f64)> = None;
        for (i, s) in coarse {
            if s + COARSE_KEEP < best_coarse {
                continue;
            }
            let e = &self.entries[i];
            let hay = if crop.height() == e.strip.height() {
                crop.clone()
            } else {
                imageops::resize(
                    crop,
                    (crop.width() * e.strip.height() / h.max(1)).max(1),
                    e.strip.height(),
                    imageops::FilterType::Triangle,
                )
            };
            let s = best_ncc(&e.strip, e.mean, e.var, &hay);
            if s >= NCC_THRESHOLD && best.is_none_or(|(_, b)| s > b) {
                best = Some((i, s));
            }
        }
        best.map(|(i, s)| (&self.entries[i], s))
    }

    /// Stores a band crop as the template for `item_key`+`count`,
    /// replacing an existing entry for the same identity and height
    /// bucket. Oldest entries are evicted past STORE_CAP.
    pub fn learn(&mut self, item_key: &str, count: u32, count_explicit: bool, crop: &GrayImage) {
        let h = crop.height();
        self.entries.retain(|e| {
            !(e.item_key == item_key
                && e.count == count
                && ((f64::from(e.strip.height()) - f64::from(h)).abs() / f64::from(h.max(1)))
                    <= HEIGHT_TOL)
        });
        let (mean, var) = stats(crop);
        if var < 25.0 {
            return; // near-flat crop carries no identity
        }
        let coarse = downscale(crop);
        self.entries.push(Learned {
            item_key: item_key.to_string(),
            count,
            count_explicit,
            strip: crop.clone(),
            coarse,
            mean,
            var,
        });
        if self.entries.len() > STORE_CAP {
            self.entries.remove(0);
        }
        self.dirty = true;
    }

    // --- persistence: tiny custom binary format, no new dependencies ---

    pub fn save(&mut self, path: &std::path::Path) -> anyhow::Result<()> {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(STORE_MAGIC);
        buf.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            let key = e.item_key.as_bytes();
            buf.extend_from_slice(&(key.len() as u32).to_le_bytes());
            buf.extend_from_slice(key);
            buf.extend_from_slice(&e.count.to_le_bytes());
            buf.push(u8::from(e.count_explicit));
            buf.extend_from_slice(&e.strip.width().to_le_bytes());
            buf.extend_from_slice(&e.strip.height().to_le_bytes());
            buf.extend_from_slice(e.strip.as_raw());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, buf)?;
        self.dirty = false;
        Ok(())
    }

    pub fn load(path: &std::path::Path) -> TemplateStore {
        let mut store = TemplateStore::new();
        let Ok(buf) = std::fs::read(path) else { return store };
        let mut p = 0usize;
        let take = |p: &mut usize, n: usize| -> Option<&[u8]> {
            let s = buf.get(*p..*p + n)?;
            *p += n;
            Some(s)
        };
        let magic = take(&mut p, 8);
        if magic != Some(STORE_MAGIC.as_slice()) {
            return store;
        }
        let Some(nb) = take(&mut p, 4) else { return store };
        let n = u32::from_le_bytes(nb.try_into().unwrap());
        for _ in 0..n {
            let Some(klen) = take(&mut p, 4) else { return store };
            let klen = u32::from_le_bytes(klen.try_into().unwrap()) as usize;
            let Some(key) = take(&mut p, klen) else { return store };
            let item_key = String::from_utf8_lossy(key).into_owned();
            let Some(cb) = take(&mut p, 4) else { return store };
            let count = u32::from_le_bytes(cb.try_into().unwrap());
            let Some(ce) = take(&mut p, 1) else { return store };
            let count_explicit = ce[0] != 0;
            let Some(wb) = take(&mut p, 4) else { return store };
            let w = u32::from_le_bytes(wb.try_into().unwrap());
            let Some(hb) = take(&mut p, 4) else { return store };
            let h = u32::from_le_bytes(hb.try_into().unwrap());
            if w == 0 || h == 0 || w > 4096 || h > 512 {
                return store;
            }
            let Some(px) = take(&mut p, (w * h) as usize) else { return store };
            let Some(strip) = GrayImage::from_raw(w, h, px.to_vec()) else { return store };
            let (mean, var) = stats(&strip);
            let coarse = downscale(&strip);
            store.entries.push(Learned {
                item_key,
                count,
                count_explicit,
                strip,
                coarse,
                mean,
                var,
            });
        }
        store
    }
}

impl Default for TemplateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn textured(w: u32, h: u32, seed: u32) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| {
            let v = (x.wrapping_mul(31).wrapping_add(y.wrapping_mul(17)).wrapping_add(seed))
                .wrapping_mul(2654435761)
                >> 24;
            image::Luma([(v as u8) / 2 + 90])
        })
    }

    #[test]
    fn exact_reencounter_matches_and_stranger_does_not() {
        let a = textured(300, 40, 1);
        let b = textured(300, 40, 999);
        let mut store = TemplateStore::new();
        store.learn("exalted orb", 3, true, &a);
        let (hit, score) = store.match_band(&a).expect("same pixels must match");
        assert_eq!(hit.item_key, "exalted orb");
        assert_eq!(hit.count, 3);
        assert!(score > 0.99, "identical strip must score ~1.0, got {score}");
        assert!(store.match_band(&b).is_none(), "unrelated texture must not match");
    }

    #[test]
    fn matches_with_brightness_shift_and_slight_offset() {
        let a = textured(300, 40, 7);
        let mut store = TemplateStore::new();
        store.learn("chaos orb", 1, false, &a);
        // Same content embedded further right in a wider band, uniformly darker.
        let mut wide = GrayImage::from_pixel(360, 40, image::Luma([100]));
        for y in 0..40 {
            for x in 0..300 {
                let p = a.get_pixel(x, y)[0].saturating_sub(18);
                wide.put_pixel(x + 40, y, image::Luma([p]));
            }
        }
        let (hit, score) = store.match_band(&wide).expect("shifted+darker must match");
        assert_eq!(hit.item_key, "chaos orb");
        assert!(score > 0.95, "NCC absorbs uniform brightness, got {score}");
    }

    #[test]
    fn height_mismatch_excludes_a_template() {
        let a = textured(300, 40, 3);
        let mut store = TemplateStore::new();
        store.learn("regal orb", 1, false, &a);
        let tall = textured(300, 80, 3);
        assert!(store.match_band(&tall).is_none(), "2x height is a different row style");
    }

    #[test]
    fn save_load_roundtrip_preserves_matching() {
        let a = textured(280, 36, 11);
        let mut store = TemplateStore::new();
        store.learn("divine orb", 2, true, &a);
        let dir = std::env::temp_dir().join(format!("khalonipoe2-tpl-test-{}", std::process::id()));
        let path = dir.join("templates.bin");
        store.save(&path).expect("save");
        let loaded = TemplateStore::load(&path);
        assert_eq!(loaded.len(), 1);
        let (hit, score) = loaded.match_band(&a).expect("roundtripped template must match");
        assert_eq!(hit.item_key, "divine orb");
        assert_eq!((hit.count, hit.count_explicit), (2, true));
        assert!(score > 0.99);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_store_loads_empty_not_panicking() {
        let dir = std::env::temp_dir().join(format!("khalonipoe2-tpl-bad-{}", std::process::id()));
        let path = dir.join("templates.bin");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, b"garbage").unwrap();
        assert!(TemplateStore::load(&path).is_empty());
        std::fs::write(&path, [STORE_MAGIC.as_slice(), &[9, 9, 9, 9]].concat()).unwrap();
        assert!(TemplateStore::load(&path).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
