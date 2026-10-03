//! Learned-template identification: once OCR has confidently identified a
//! band, its text-region pixels become a template; later encounters of the
//! same reward are identified by normalized cross-correlation against the
//! stored strips in ~1 ms, bypassing tesseract entirely. Templates are the
//! game's own rendering, so matching is exact-by-construction across
//! sessions (same font, size, and antialiasing), and NCC's normalization
//! absorbs brightness differences between areas.
//!
//! Correlation over a whole strip is an identity test for the NAME only.
//! A stack count is one or two glyphs in a strip of twenty: "3x Exalted
//! Orb" against the same strip with the digit changed scores 0.99, with
//! the digit erased 0.985, against "13x" 0.987 - all far over any usable
//! whole-strip threshold, so a whole-strip hit would price a row at the
//! count it was learned with for as long as the template lives. A hit
//! therefore also has to hold glyph by glyph (`blocks_agree`), and every
//! distinct crop a template claims is checked against OCR once before it
//! is trusted unattended (`VerifyTicket`).

use image::{imageops, GrayImage};

/// Minimum whole-strip correlation for a template hit. Same-content strips
/// across frames measure > 0.97 (see the corpus test); unrelated rewards
/// measure < 0.6. The gap is wide; 0.90 sits safely inside it. This tells
/// names apart, not counts: see `BLOCK_NCC_MIN`.
pub const NCC_THRESHOLD: f64 = 0.90;

/// Coarse pass downscale factor (both axes); candidates within
/// COARSE_KEEP of the coarse best are re-scored at full resolution.
const COARSE_DOWN: u32 = 4;
const COARSE_KEEP: f64 = 0.08;

/// Height bucket tolerance: a template only competes for bands whose
/// height is within this fraction of its own.
const HEIGHT_TOL: f64 = 0.12;

/// A matched strip is re-scored in windows this wide, as a fraction of
/// the strip height (about two glyphs), every half window.
const BLOCK_WIDTH_FRAC: f64 = 0.35;
/// The windows leave out this fraction of the strip's rows at the top and
/// at the bottom: the crop's padding and the bar's border lines, which are
/// identical on every row of the panel and would prop up the correlation
/// of a window whose glyphs differ.
const BLOCK_ROW_MARGIN_FRAC: f64 = 0.125;
/// Minimum correlation of every inked window. Measured on panel_choice
/// (tests/template_counts.rs): the same strip under a brightness or gain
/// change stays above 0.99 in every window; a changed digit leaves one at
/// 0.64 ("3x" to "1x") or 0.78 (the 3 closed into an 8, the nearest
/// look-alike), an erased or an added digit at 0 - against whole-strip
/// scores of 0.986 to 0.991 for all four.
pub const BLOCK_NCC_MIN: f64 = 0.93;
/// Pixel variance at which a window holds ink, and the variance under
/// which it is blank bar. Measured on the same crops: bar texture alone
/// stays under 250, a window that a glyph merely touches reaches 440, and
/// one holding a whole glyph starts at 1800. Between the two values a
/// window is scored by correlation like any inked one.
const BLOCK_INK_VAR: f64 = 600.0;
const BLOCK_BLANK_VAR: f64 = 300.0;

/// How long one OCR confirmation of a crop stands before the next hit on
/// it asks again. An unchanged panel's OCR is memoised by the scan cache,
/// so asking again costs a lookup; it matters when the vocabulary or the
/// price table changed what the same text resolves to.
const VERIFY_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// Confirmed crops remembered per template (the same reward under a few
/// backgrounds or brightness levels).
const VERIFIED_CAP: usize = 8;

const STORE_CAP: usize = 512;
/// Bumped from 01: stores written before the per-glyph check can hold
/// strips learned at one count and matched at another, and nothing in the
/// file tells those apart, so an older file loads as empty.
const STORE_MAGIC: &[u8; 8] = b"P2LTPL02";

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
    /// Identity within this process, for `VerifyTicket`.
    id: u64,
    /// Content keys of crops OCR has confirmed this template for, and
    /// when. Never persisted: a store loaded from disk earns its trust
    /// again, which is also what retires a bad strip from an old session.
    verified: Vec<(u64, std::time::Instant)>,
}

/// A band identified from a template.
#[derive(Debug, Clone, PartialEq)]
pub struct TemplateHit {
    pub item_key: String,
    pub count: u32,
    pub count_explicit: bool,
    pub score: f64,
    /// Present when OCR has not yet confirmed this template for this exact
    /// crop (or did so too long ago). The hit may be shown, but the caller
    /// must get an OCR read of the band and hand it to
    /// `TemplateStore::confirm` with this ticket.
    pub verify: Option<VerifyTicket>,
}

/// Names the template and crop content a pending OCR check is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyTicket {
    template: u64,
    content: u64,
}

pub struct TemplateStore {
    entries: Vec<Learned>,
    next_id: u64,
    pub dirty: bool,
}

/// FNV-1a over a crop's size and pixels.
fn content_key(img: &GrayImage) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut step = |b: u8| {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    for v in [img.width(), img.height()] {
        v.to_le_bytes().into_iter().for_each(&mut step);
    }
    img.as_raw().iter().copied().for_each(&mut step);
    h
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

/// Best NCC of `tpl` slid horizontally across `hay` (same height), and
/// the offset it was found at.
fn best_ncc(tpl: &GrayImage, tpl_mean: f64, tpl_var: f64, hay: &GrayImage) -> (f64, u32) {
    if hay.width() < tpl.width() || hay.height() != tpl.height() {
        return (-1.0, 0);
    }
    let mut best = (-1.0f64, 0u32);
    for x0 in 0..=(hay.width() - tpl.width()) {
        let s = ncc_at(tpl, tpl_mean, tpl_var, hay, x0);
        if s > best.0 {
            best = (s, x0);
        }
    }
    best
}

/// Mean and variance of the `w`-wide window of `img` starting at `x`.
fn window_stats(img: &GrayImage, x: u32, w: u32) -> (f64, f64) {
    let (iw, h) = (img.width() as usize, img.height() as usize);
    let raw = img.as_raw();
    let n = (w as usize * h) as f64;
    let (mut sum, mut sum2) = (0f64, 0f64);
    for y in 0..h {
        for &p in &raw[y * iw + x as usize..y * iw + (x + w) as usize] {
            let v = f64::from(p);
            sum += v;
            sum2 += v * v;
        }
    }
    let mean = sum / n;
    (mean, sum2 / n - mean * mean)
}

/// Lowest per-window correlation between `tpl` and `hay` aligned at `x0`,
/// over windows about two glyphs wide. A window that is blank on both
/// sides agrees; one that holds ink on one side and blank bar on the other
/// scores zero (an erased or an added digit); the rest score their own
/// NCC. Whole-strip correlation averages a changed digit away; its own
/// window cannot.
fn blocks_agree(tpl: &GrayImage, hay: &GrayImage, x0: u32) -> f64 {
    let (tw, th) = (tpl.width(), tpl.height());
    let margin = (f64::from(th) * BLOCK_ROW_MARGIN_FRAC).round() as u32;
    let rows = th.saturating_sub(2 * margin);
    if rows == 0 || tw == 0 {
        return 0.0;
    }
    let tpl = imageops::crop_imm(tpl, 0, margin, tw, rows).to_image();
    let hay = imageops::crop_imm(hay, x0, margin, tw, rows).to_image();
    let bw = ((f64::from(th) * BLOCK_WIDTH_FRAC).round() as u32).clamp(4.min(tw), tw);
    let stride = (bw / 2).max(1);
    let mut worst = 1.0f64;
    let mut x = 0u32;
    loop {
        let bx = x.min(tw - bw);
        let (tm, tv) = window_stats(&tpl, bx, bw);
        let (_, hv) = window_stats(&hay, bx, bw);
        let score = if tv.max(hv) < BLOCK_INK_VAR {
            1.0
        } else if tv.min(hv) < BLOCK_BLANK_VAR {
            0.0
        } else {
            let block = imageops::crop_imm(&tpl, bx, 0, bw, rows).to_image();
            ncc_at(&block, tm, tv, &hay, bx)
        };
        worst = worst.min(score);
        if bx == tw - bw {
            break;
        }
        x += stride;
    }
    worst
}

impl TemplateStore {
    pub fn new() -> TemplateStore {
        TemplateStore { entries: Vec::new(), next_id: 0, dirty: false }
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
            let (s, _) = best_ncc(&e.coarse, cm, cv, &hay);
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
            let (s, x0) = best_ncc(&e.strip, e.mean, e.var, &hay);
            if s >= NCC_THRESHOLD
                && best.is_none_or(|(_, b)| s > b)
                && blocks_agree(&e.strip, &hay, x0) >= BLOCK_NCC_MIN
            {
                best = Some((i, s));
            }
        }
        best.map(|(i, s)| (&self.entries[i], s))
    }

    /// The best whole-strip correlation any same-height template reaches
    /// on `crop`, with that alignment's worst per-glyph window score,
    /// thresholds not applied. For diagnostics and for the tests that pin
    /// the two thresholds to measurements.
    pub fn scores(&self, crop: &GrayImage) -> Option<(f64, f64)> {
        self.entries
            .iter()
            .filter(|e| e.strip.height() == crop.height())
            .map(|e| {
                let (s, x0) = best_ncc(&e.strip, e.mean, e.var, crop);
                (s, e, x0)
            })
            .filter(|(s, ..)| *s > -1.0)
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(s, e, x0)| (s, blocks_agree(&e.strip, crop, x0)))
    }

    /// `match_band` for the scan loop: the identified row, plus a ticket
    /// when the hit still owes an OCR confirmation for this crop.
    pub fn lookup(&self, crop: &GrayImage) -> Option<TemplateHit> {
        let (hit, score) = self.match_band(crop)?;
        let content = content_key(crop);
        let confirmed = hit
            .verified
            .iter()
            .any(|&(key, when)| key == content && when.elapsed() < VERIFY_TTL);
        Some(TemplateHit {
            item_key: hit.item_key.clone(),
            count: hit.count,
            count_explicit: hit.count_explicit,
            score,
            verify: (!confirmed).then_some(VerifyTicket { template: hit.id, content }),
        })
    }

    /// Settles a ticket with what OCR read off the same band: the row's
    /// item key and count, or None when OCR produced no row for it (the
    /// check stays owed). Agreement marks the crop confirmed and returns
    /// true. Disagreement removes the template, so the caller's usual
    /// `learn` of the OCR row replaces it, and returns false: the OCR row
    /// is the one to show.
    pub fn confirm(&mut self, ticket: VerifyTicket, ocr: Option<(&str, u32)>) -> bool {
        let Some(pos) = self.entries.iter().position(|e| e.id == ticket.template) else {
            return false;
        };
        let Some((item_key, count)) = ocr else {
            return true;
        };
        let e = &mut self.entries[pos];
        if e.item_key == item_key && e.count == count {
            e.verified.retain(|&(key, _)| key != ticket.content);
            e.verified.push((ticket.content, std::time::Instant::now()));
            if e.verified.len() > VERIFIED_CAP {
                e.verified.remove(0);
            }
            true
        } else {
            self.entries.remove(pos);
            self.dirty = true;
            false
        }
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
        // The strip was just read by OCR as exactly this row: that is the
        // confirmation for this content.
        let verified = vec![(content_key(crop), std::time::Instant::now())];
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(Learned {
            item_key: item_key.to_string(),
            count,
            count_explicit,
            strip: crop.clone(),
            coarse,
            mean,
            var,
            id,
            verified,
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
            let id = store.next_id;
            store.next_id += 1;
            store.entries.push(Learned {
                item_key,
                count,
                count_explicit,
                strip,
                coarse,
                mean,
                var,
                id,
                verified: Vec::new(),
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
