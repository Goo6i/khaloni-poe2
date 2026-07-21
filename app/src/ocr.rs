use std::{process::Command, sync::atomic::AtomicU64};

use image::{imageops, GrayImage};

// --- Band-detection constants and the evidence behind them ---
//
// Live tracing against the "choice" Runeshape panel (4 bright reward rows
// stacked over a large mid-gray parchment map background, e.g. the reward
// choice after finishing a favour) showed whole-panel psm-6 tesseract
// returning 0 lines: a single global Otsu binarization drowns the small
// bright-row area inside the much larger mid-gray map, so nothing on the
// panel binarizes into readable text at all. A hover tooltip elsewhere on
// screen coincidentally rebalanced the captured frame's brightness
// histogram enough to fix the binarization by accident - which is the only
// reason labels ever appeared, and only while hovering.
//
// Measured brightness on real captures: the white reward-row bars read
// ~200+ mean; the parchment map background reads ~130-156 mean.
// BAND_BRIGHTNESS=175 sits cleanly between the two, with real margin on
// both sides. Per-row detection avoids the whole-panel binarization problem
// entirely, since each band is cropped and OCR'd independently, with
// nothing but its own bright bar in frame. This also matches how the two
// existing third-party tools for this exact panel work (Denzeriko/
// RuneHelper, Barragek0/RuneshapePriceChecker: both crop each reward row
// individually before OCR) and official tesseract guidance to feed it
// single, well-isolated lines rather than whole busy screenshots.
//
// Fixture: app/tests/fixtures/panel_choice.png (a real failing capture,
// 4 real bands: "Unique Jewellery", "1x Greater Jeweller's Orb",
// "1x Cyclonic Alloy", "3x Exalted Orb", over the map background below).
//
// PSM AND SCALE, measured against that fixture directly (not assumed): the
// brief's starting recipe was psm 7 (single line) at the existing 3x
// upscale. Run against the real 4 bands, psm 7 misread all four (one band
// came back as a single garbage glyph, "m", with nothing else) - the wide
// mostly-blank crop plus a leftover sliver of the adjacent icon apparently
// defeats psm 7's single-line assumption. psm 4 ("assume a single column of
// text of variable sizes") read all 4 bands correctly at the existing 3x
// scale except one specific digit: the leading "1x" count on one row came
// back as "Ix" (capital I) at 31.9% confidence - a real character-level
// ambiguity in this font at that resolution, not a segmentation problem.
// Bumping ONLY the OCR crop's own upscale to 4x (BAND_OCR_SCALE, kept
// separate from UPSCALE, the coordinate-contract constant `y_top`/`height`
// are still computed from - see OcrLine's field docs) resolved that digit
// cleanly too: psm 4 at 4x reads all 4 bands of the fixture exactly right.
// psm 4 at 4x is what's actually used below; psm 7 is not used anywhere.

/// Mean brightness (capture-pixel space, 0-255) a row must clear to count
/// as part of a band. See the evidence block above for the measured gap
/// this sits in.
pub const BAND_BRIGHTNESS: u8 = 175;
/// A run of bright rows shorter than this (capture px) is discarded as
/// noise rather than a real reward bar.
pub const BAND_MIN_H: u32 = 12;
/// Extra capture-px padding added above/below a detected band before
/// cropping, so a row's text isn't clipped right at the brightness edge.
pub const BAND_PAD: u32 = 4;
/// Left crop fraction of the region width: skips the icon column. Left at
/// a stop wider than the milestone-0 whole-panel crop (0.25) because band
/// detection's per-row brightness average is more sensitive to a partial
/// icon still being inside the sampled strip than a single whole-panel OCR
/// pass was.
pub const ICON_CUT: f32 = 0.30;
/// Right crop fraction of the region width, trimmed off the rightmost edge
/// (drop-shadow/border artifacts).
pub const RIGHT_TRIM: f32 = 0.02;
/// Coordinate-contract scale factor: `OcrLine::y_top`/`height` are set to
/// the band's own capture-space bounds times this, exactly as the old
/// whole-panel path set them from a 3x-upscaled crop, so coord/stabilize's
/// already-calibrated preprocessed-pixel constants stay valid unchanged.
/// Deliberately independent of BAND_OCR_SCALE (the crop's own actual
/// upscale for OCR quality) - the two used to be the same constant when
/// there was only one whole-panel crop; band OCR needs more resolution
/// than the coordinate contract does, so they're split.
pub const UPSCALE: u32 = 3;
/// Actual upscale applied to a band's crop before tesseract sees it. See
/// the evidence block above: 3x (matching UPSCALE) leaves one real digit
/// ambiguous ("1x" read as "Ix"); 4x resolves it. Kept separate from
/// UPSCALE so this can be tuned for OCR accuracy without touching the
/// coordinate contract the rest of the pipeline depends on.
const BAND_OCR_SCALE: u32 = 4;
const MIN_CONF: f32 = 40.0;
/// A line's unfiltered text must contain a run of at least this many
/// consecutive alphabetic characters to be kept as a row. Guards against
/// 1-3 character OCR noise fragments (icon artifacts, stray glyphs,
/// misread punctuation) that would otherwise surface as spurious rows
/// downstream; every real panel line (an item name, "Support:", "Skill
/// Level N:") comfortably clears this bar.
pub const MIN_WORD_RUN: usize = 4;
/// Whitelisted characters for per-band tesseract: item names, counts
/// ("Nx"), and the apostrophe/colon/hyphen that show up in real panel text
/// ("Jeweller's", "Skill Level 20:", multi-word names). Excluding
/// everything else (icon-adjacent glyphs, stray punctuation) measurably
/// improves per-band accuracy over the unrestricted default.
const BAND_WHITELIST: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789:'x- ";

/// True when `text` contains a run of at least `min_run` consecutive ASCII
/// alphabetic characters.
fn has_alpha_run(text: &str, min_run: usize) -> bool {
    let mut run = 0usize;
    for c in text.chars() {
        if c.is_ascii_alphabetic() {
            run += 1;
            if run >= min_run {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrLine {
    /// Normalized text from words with confidence >= 40 (fuzzy-match tier).
    pub filtered: String,
    /// Normalized text from all words (substring-match tier).
    pub unfiltered: String,
    /// Top of the line in preprocessed-image pixels (capture-space y0 *
    /// UPSCALE): the coordinate contract pricing/coord/render already
    /// expect is unchanged even though the crop is now per-band.
    pub y_top: u32,
    pub height: u32,
}

/// Scans a captured gray region (capture-pixel space) for bright reward
/// rows: for each row, the mean brightness over x in
/// [ICON_CUT, 1-RIGHT_TRIM] of the width (skipping the icon column and the
/// right edge); a band is a contiguous run of rows at or above
/// BAND_BRIGHTNESS, at least BAND_MIN_H tall. Returns (y0, y1) pairs in
/// capture-pixel space, top to bottom.
pub fn detect_bands(gray: &GrayImage) -> Vec<(u32, u32)> {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let x0 = ((w as f32) * ICON_CUT) as usize;
    let x1 = (((w as f32) * (1.0 - RIGHT_TRIM)) as usize).clamp(x0 + 1, w);
    let raw = gray.as_raw();

    let mut bands = Vec::new();
    let mut band_start: Option<u32> = None;
    for y in 0..h {
        let row = &raw[y * w + x0..y * w + x1];
        let mean = row.iter().map(|&p| u64::from(p)).sum::<u64>() / row.len() as u64;
        let bright = mean >= u64::from(BAND_BRIGHTNESS);
        match (bright, band_start) {
            (true, None) => band_start = Some(y as u32),
            (false, Some(y0)) => {
                let y1 = y as u32;
                if y1 - y0 >= BAND_MIN_H {
                    bands.push((y0, y1));
                }
                band_start = None;
            }
            _ => {}
        }
    }
    if let Some(y0) = band_start {
        let y1 = h as u32;
        if y1 - y0 >= BAND_MIN_H {
            bands.push((y0, y1));
        }
    }
    bands
}

/// Runs one band's crop/upscale/tesseract pass; `None` on any failure
/// (crop out of range, tesseract error, or the result fails the
/// empty/alpha-run guards), so one bad band never kills the others.
fn ocr_one_band(cmd: &str, gray: &GrayImage, y0: u32, y1: u32) -> Option<OcrLine> {
    let (w, h) = (gray.width(), gray.height());
    let x0 = ((w as f32) * ICON_CUT) as u32;
    let x1 = (((w as f32) * (1.0 - RIGHT_TRIM)) as u32).clamp(x0 + 1, w);
    let cy0 = y0.saturating_sub(BAND_PAD);
    let cy1 = (y1 + BAND_PAD).min(h);
    if x1 <= x0 || cy1 <= cy0 {
        return None;
    }

    let crop = imageops::crop_imm(gray, x0, cy0, x1 - x0, cy1 - cy0).to_image();
    let up = imageops::resize(
        &crop,
        crop.width() * BAND_OCR_SCALE,
        crop.height() * BAND_OCR_SCALE,
        imageops::FilterType::Lanczos3,
    );

    let tsv = run_band_tesseract(cmd, &up).ok()?;
    parse_band_tsv(&tsv, y0, y1)
}

/// Runs tesseract on all detected bands concurrently (one process per
/// band; bands are tiny single-line crops, so total wall time is close to
/// a single strip rather than scaling with band count) and returns a
/// single y-ordered Vec<OcrLine>.
pub fn ocr_bands(cmd: &str, gray: &GrayImage, bands: &[(u32, u32)]) -> Vec<OcrLine> {
    let mut lines: Vec<OcrLine> = std::thread::scope(|scope| {
        let handles: Vec<_> = bands
            .iter()
            .map(|&(y0, y1)| scope.spawn(move || ocr_one_band(cmd, gray, y0, y1)))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
    });
    lines.sort_by_key(|l| l.y_top);
    lines
}

fn run_band_tesseract(cmd: &str, img: &GrayImage) -> anyhow::Result<String> {
    let tmp = tempfile_path();
    img.save(&tmp)?;
    let out = Command::new(cmd)
        .args([
            tmp.to_str().unwrap(),
            "-",
            "--psm",
            "4",
            "-c",
            &format!("tessedit_char_whitelist={BAND_WHITELIST}"),
            "-l",
            "eng",
            "tsv",
        ])
        .output();
    let _ = std::fs::remove_file(&tmp);
    let out = out?;
    if !out.status.success() {
        anyhow::bail!(
            "tesseract failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn tempfile_path() -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let c = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("poe2-lens-ocr-{}-{n}-{c}.png", std::process::id()))
}

/// Builds one OcrLine from a single band's raw tesseract TSV output. Public
/// so the parsing logic (confidence split, MIN_WORD_RUN guard, normalize)
/// is directly unit-testable without invoking a real tesseract process. A
/// band crop is, by construction, one reward row's worth of text - even
/// though `--psm 4` (unlike `--psm 7`) doesn't strictly guarantee tesseract
/// reports it all as a single TSV line/paragraph, every real word (TSV
/// level 5) in the crop belongs to the same logical row, so this collects
/// all of them regardless of block/par/line grouping - no multi-row
/// grouping needed, unlike the old whole-panel parser this replaces.
/// `y0`/`y1` are the band's bounds in ORIGINAL capture-pixel space (not the
/// crop's own local coordinates); `y_top`/`height` are set from them
/// (`* UPSCALE`) rather than from anything tesseract itself reports, so the
/// preprocessed-space coordinate contract downstream (pricing/coord/render)
/// is unchanged by moving to per-band crops.
pub fn parse_band_tsv(tsv: &str, y0: u32, y1: u32) -> Option<OcrLine> {
    let mut filtered = Vec::new();
    let mut unfiltered = Vec::new();
    for row in tsv.lines().skip(1) {
        let f: Vec<&str> = row.split('\t').collect();
        if f.len() < 12 || f[0] != "5" {
            continue;
        }
        let conf: f32 = f[10].parse().unwrap_or(-1.0);
        let word = f[11].trim();
        if word.is_empty() {
            continue;
        }
        if conf >= MIN_CONF {
            filtered.push(word.to_string());
        }
        unfiltered.push(word.to_string());
    }

    let unfiltered = poe2_lens_core::matcher::normalize(&unfiltered.join(" "));
    if unfiltered.trim().is_empty() || !has_alpha_run(&unfiltered, MIN_WORD_RUN) {
        return None;
    }
    Some(OcrLine {
        filtered: poe2_lens_core::matcher::normalize(&filtered.join(" ")),
        unfiltered,
        y_top: y0 * UPSCALE,
        height: (y1 - y0) * UPSCALE,
    })
}
