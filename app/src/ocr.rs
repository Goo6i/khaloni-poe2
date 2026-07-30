use std::collections::BTreeMap;

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

/// Merge bright runs separated by gaps at or below this into one entry
/// band. Measured on real captures: intra-entry gaps (icon/text/descender
/// strips of tall skill entries) are 2-11 px; gaps between entries are
/// 16-18 px in both tall and choice panel styles.
pub const BAND_MERGE_GAP: u32 = 13;
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
#[cfg(ocr)]
const BAND_OCR_SCALE: u32 = 3;
const MIN_CONF: f32 = 40.0;
/// A line's unfiltered text must contain a run of at least this many
/// consecutive alphabetic characters to be kept as a row. Guards against
/// 1-3 character OCR noise fragments (icon artifacts, stray glyphs,
/// misread punctuation) that would otherwise surface as spurious rows
/// downstream; every real panel line (an item name, "Support:", "Skill
/// Level N:") comfortably clears this bar.
pub const MIN_WORD_RUN: usize = 4;

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
/// right edge). Raw bright runs are collected without a height filter,
/// then runs separated by small gaps are merged into one entry band
/// (tall skill entries render as icon strip + text strip + descender
/// strip: measured intra-entry gaps are 2-11 px, gaps between entries
/// 16-18 px, so BAND_MERGE_GAP = 13 separates the populations), and only
/// then is BAND_MIN_H applied. Returns (y0, y1) pairs in capture-pixel
/// space, top to bottom.
/// Per-row mean brightness over the band-detection x-window, in
/// capture-pixel rows. Shared by band detection and optical scroll
/// estimation (the profile of a scrolled frame is the previous frame's
/// profile shifted vertically).
pub fn row_profile(gray: &GrayImage) -> Vec<u16> {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let x0 = ((w as f32) * ICON_CUT) as usize;
    let x1 = (((w as f32) * (1.0 - RIGHT_TRIM)) as usize).clamp(x0 + 1, w);
    let raw = gray.as_raw();
    (0..h)
        .map(|y| {
            let row = &raw[y * w + x0..y * w + x1];
            (row.iter().map(|&p| u64::from(p)).sum::<u64>() / row.len() as u64) as u16
        })
        .collect()
}

/// One frame's vertical motion relative to the previous frame, judged
/// from their row profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// Near-identical to the previous frame (or drifted by <= 2 profile
    /// rows, which POSITION_SNAP absorbs downstream). Safe to scan.
    Still,
    /// Content shifted vertically by this many profile rows.
    Scrolled(i32),
    /// The frame differs substantially and no shift explains it: a flick
    /// faster than the search range, a panel switch, or a large content
    /// change mid-scroll. Positions held from before are untrustworthy.
    Lost,
}

/// Normalized-SAD ceiling (x1024 fixed point) under which two profiles
/// count as "the same content": accepts a candidate shift, and separates
/// Still from Lost at dy=0. Measured on the live corpus for
/// estimate_scroll and carried over unchanged.
const SAME_MAX: u64 = 12 * 1024;

/// Classifies the vertical motion between two frames' row profiles:
/// minimizes normalized SAD over dy candidates, requiring at least 30%
/// overlap. Sub-millisecond for ~1000-row profiles, so it can run on
/// every captured frame.
pub fn track_motion(prev: &[u16], cur: &[u16]) -> Motion {
    const MAX_DY: i32 = 240;
    if prev.len() != cur.len() || prev.len() < 100 {
        return Motion::Lost;
    }
    let n = prev.len() as i32;
    // Flat profiles (no structure) match at every shift; call the frame
    // Still and let band detection decide what is actually on screen.
    let (min, max) = cur.iter().fold((u16::MAX, 0u16), |(a, b), &v| (a.min(v), b.max(v)));
    if max - min < 20 {
        return Motion::Still;
    }
    // Convention: positive dy = content moved DOWN by dy rows (cur[i]
    // matches prev[i - dy]), which is the direction slot y values shift.
    let sad_at = |dy: i32| -> Option<u64> {
        let (p0, c0) = if dy >= 0 { (0usize, dy as usize) } else { ((-dy) as usize, 0usize) };
        let overlap = (n - dy.abs()) as usize;
        if overlap * 10 < prev.len() * 3 {
            return None;
        }
        let mut sum = 0u64;
        for i in 0..overlap {
            sum += u64::from(prev[p0 + i].abs_diff(cur[c0 + i]));
        }
        // Scaled normalization: plain integer division floors away
        // the one-pixel edge signal that disambiguates neighboring
        // offsets (measured: a 1 px misalignment scores 0.68/row, which
        // floored to 0 and tied three offsets at zero).
        Some(sum * 1024 / overlap as u64)
    };
    let Some(base) = sad_at(0) else {
        return Motion::Lost;
    };
    let mut best = (0i32, base);
    for dy in (-MAX_DY..=MAX_DY).filter(|&d| d != 0) {
        if let Some(s) = sad_at(dy) {
            // Ties (uniform background regions) resolve toward the
            // smallest displacement.
            if s < best.1 || (s == best.1 && dy.abs() < best.0.abs()) {
                best = (dy, s);
            }
        }
    }
    // A shift only counts when it beats "no movement" decisively AND is
    // a near-exact overlay of the previous frame (a true scroll is the
    // same pixels displaced; unrelated content merely resembles it).
    if best.0 != 0 && best.1 * 3 < base && best.1 <= SAME_MAX {
        if best.0.abs() > 2 {
            Motion::Scrolled(best.0)
        } else {
            Motion::Still
        }
    } else if base <= SAME_MAX {
        Motion::Still
    } else {
        Motion::Lost
    }
}

pub fn detect_bands(gray: &GrayImage) -> Vec<(u32, u32)> {
    let profile = row_profile(gray);
    detect_bands_from_profile(&profile)
}

pub fn detect_bands_from_profile(profile: &[u16]) -> Vec<(u32, u32)> {
    let mut bands = Vec::new();
    let mut band_start: Option<u32> = None;
    for (y, &mean) in profile.iter().enumerate() {
        let bright = mean >= u16::from(BAND_BRIGHTNESS);
        match (bright, band_start) {
            (true, None) => band_start = Some(y as u32),
            (false, Some(y0)) => {
                bands.push((y0, y as u32));
                band_start = None;
            }
            _ => {}
        }
    }
    if let Some(y0) = band_start {
        bands.push((y0, profile.len() as u32));
    }

    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (y0, y1) in bands {
        match merged.last_mut() {
            Some((_, last_y1)) if y0 - *last_y1 <= BAND_MERGE_GAP => *last_y1 = y1,
            _ => merged.push((y0, y1)),
        }
    }
    merged.retain(|(y0, y1)| y1 - y0 >= BAND_MIN_H);
    merged
}

/// Runs one band's crop/upscale/tesseract pass; `None` on any failure
/// (crop out of range, tesseract error, or the result fails the
/// empty/alpha-run guards), so one bad band never kills the others.
/// The exact text-region crop OCR sees for a band; shared with the
/// template engine so learned strips and later encounters are
/// pixel-compatible by construction.
pub fn band_crop(gray: &GrayImage, y0: u32, y1: u32) -> Option<GrayImage> {
    let (w, h) = (gray.width(), gray.height());
    let x0 = ((w as f32) * ICON_CUT) as u32;
    let x1 = (((w as f32) * (1.0 - RIGHT_TRIM)) as u32).clamp(x0 + 1, w);
    let cy0 = y0.saturating_sub(BAND_PAD);
    let cy1 = (y1 + BAND_PAD).min(h);
    if x1 <= x0 || cy1 <= cy0 {
        return None;
    }
    Some(imageops::crop_imm(gray, x0, cy0, x1 - x0, cy1 - cy0).to_image())
}

#[cfg(ocr)]
fn ocr_one_band(engine: &mut OcrEngine, gray: &GrayImage, y0: u32, y1: u32) -> Option<OcrLine> {
    let crop = band_crop(gray, y0, y1)?;
    let up = imageops::resize(
        &crop,
        crop.width() * BAND_OCR_SCALE,
        crop.height() * BAND_OCR_SCALE,
        imageops::FilterType::Lanczos3,
    );

    let tsv = engine.tsv(&up).ok()?;
    parse_band_tsv(&tsv, y0, y1)
}

/// Runs the persistent engine over all detected bands sequentially (a
/// strip takes ~5-30 ms in-process, so sequential still finishes well
/// under one old CLI spawn) and returns a single y-ordered Vec<OcrLine>.
#[cfg(ocr)]
pub fn ocr_bands(engine: &mut OcrEngine, gray: &GrayImage, bands: &[(u32, u32)]) -> Vec<OcrLine> {
    let mut lines: Vec<OcrLine> = bands
        .iter()
        .filter_map(|&(y0, y1)| ocr_one_band(engine, gray, y0, y1))
        .collect();
    lines.sort_by_key(|l| l.y_top);
    lines
}


/// Persistent in-process tesseract instance. Spawning the tesseract CLI
/// pays ~130 ms of model load per invocation (measured on the reference
/// machine: 151 ms spawned vs 75 ms in-process for a whole-panel pass,
/// and the gap dominates entirely on small band strips); RuneHelper, the
/// closest comparable tool, holds a persistent TessBaseAPI for the same
/// reason. One engine per OCR worker thread; scans run sequentially on
/// it, which at in-process speeds still beats concurrent process spawns.
#[cfg(ocr)]
pub struct OcrEngine {
    lt: leptess::LepTess,
}

#[cfg(ocr)]
impl OcrEngine {
    pub fn new() -> anyhow::Result<OcrEngine> {
        // Portable tessdata resolution: when TESSDATA_PREFIX is unset and
        // eng.traineddata sits next to the executable (the Windows release
        // zip layout), point tesseract there; otherwise the system default
        // (distro tessdata on Linux) applies.
        let datapath: Option<String> = if std::env::var_os("TESSDATA_PREFIX").is_none() {
            std::env::current_exe().ok().and_then(|exe| {
                let dir = exe.parent()?;
                dir.join("eng.traineddata")
                    .exists()
                    .then(|| dir.to_string_lossy().into_owned())
            })
        } else {
            None
        };
        let mut lt = leptess::LepTess::new(datapath.as_deref(), "eng")
            .map_err(|e| anyhow::anyhow!("tesseract init failed: {e}"))?;
        // Route tesseract's tprintf chatter to the bit bucket. On live
        // frames the sparse-text pass regularly finds sub-3px noise specks
        // and prints "Image too small to scale!!" / "Line cannot be
        // recognized!!" per speck; debug_file is tesseract's own switch for
        // exactly this, and (being a global param) it also swallows the
        // cosmetic ObjectCache LEAK warnings its static destructor prints
        // at exit while worker threads still hold engines. Untestable from
        // cargo (C-level stderr, only triggered by noisy real frames), so
        // best-effort and verified live.
        #[cfg(unix)]
        let sink = "/dev/null";
        #[cfg(windows)]
        let sink = "nul";
        let _ = lt.set_variable(leptess::Variable::DebugFile, sink);
        Ok(OcrEngine { lt })
    }

    /// One TSV pass over `img` at page-segmentation mode 6 (uniform
    /// block, the mode every accuracy measurement in this file used).
    fn tsv(&mut self, img: &GrayImage) -> anyhow::Result<String> {
        let mut png: Vec<u8> = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
        self.lt
            .set_image_from_mem(&png)
            .map_err(|e| anyhow::anyhow!("set_image: {e}"))?;
        self.lt
            .set_variable(leptess::Variable::TesseditPagesegMode, "6")
            .map_err(|e| anyhow::anyhow!("set psm: {e}"))?;
        self.lt
            .get_tsv_text(0)
            .map_err(|e| anyhow::anyhow!("tsv: {e}"))
    }

    /// Public raw-TSV pass (psm 6) over an arbitrary gray image, for the
    /// rumour recognizer which does its own line grouping. `None` on any
    /// tesseract failure, matching the "degrade, never crash" contract.
    pub fn tsv_of(&mut self, img: &GrayImage) -> Option<String> {
        self.tsv(img).ok()
    }

    /// Like `tsv_of` but at a caller-chosen page-segmentation mode. PSM 11
    /// (sparse text) reads the stylized cursive rumour names that the
    /// uniform-block PSM 6 garbles; the rumour recognizer unions both.
    pub fn tsv_of_psm(&mut self, img: &GrayImage, psm: u32) -> Option<String> {
        let mut png: Vec<u8> = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .ok()?;
        self.lt.set_image_from_mem(&png).ok()?;
        self.lt
            .set_variable(leptess::Variable::TesseditPagesegMode, &psm.to_string())
            .ok()?;
        self.lt.get_tsv_text(0).ok()
    }
}


/// Builds one OcrLine from a single band's raw tesseract TSV output. Public
/// so the parsing logic (confidence split, MIN_WORD_RUN guard, normalize)
/// is directly unit-testable without invoking a real tesseract process. A
/// band crop is, by construction, one reward row's worth of text - even
/// though `--psm 6` doesn't strictly guarantee tesseract reports it all as
/// a single TSV line/paragraph, every real word (TSV level 5) in the crop
/// belongs to the same logical row, so this collects all of them
/// regardless of block/par/line grouping - no multi-row grouping needed,
/// unlike `parse_whole_tsv` below (which OCRs the whole multi-row panel in
/// one pass and so must group words back into rows by block/par/line).
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

    let unfiltered = khaloni_poe2_core::matcher::normalize(&unfiltered.join(" "));
    if unfiltered.trim().is_empty() || !has_alpha_run(&unfiltered, MIN_WORD_RUN) {
        return None;
    }
    Some(OcrLine {
        filtered: khaloni_poe2_core::matcher::normalize(&filtered.join(" ")),
        unfiltered,
        y_top: y0 * UPSCALE,
        height: (y1 - y0) * UPSCALE,
    })
}

// --- Whole-panel OCR: the milestone-0 recipe, restored ---
//
// Per-band strip OCR (above) was built to fix the "choice" panel style,
// where a single whole-panel Otsu binarization drowns the small bright
// reward-row area inside a much larger mid-gray map background and
// tesseract returns 0 lines. It does fix that. But live sweeps across
// other real captures (tall Runeshape panels: support/spirit rows, one
// oddly-short entry, in spikes/ocr/samples/s1.png..s5.png) show the
// reverse failure: a handful of odd-height bands come back garbled from
// their own small, low-context crop, while the OLD whole-panel psm-6 TSV
// pass - deleted when band OCR replaced it - read those exact panels at
// the proven 39/40 level, because tesseract gets a full paragraph of
// context instead of one isolated strip. The two pipelines fail on
// disjoint panel styles (measured, not assumed): band OCR wins on
// "choice" panels, whole-panel OCR wins on tall panels. `ocr_scan` below
// runs both and unions the results instead of picking one.
//
// Everything from here down is that whole-panel pipeline, restored
// verbatim from before the band-OCR switch (commit 13cd4fc): crop the
// icon column at WHOLE_ICON_CUT, upscale by UPSCALE, min-max normalize,
// run tesseract psm 6 over the whole region, and group words back into
// per-(block,par,line) rows. Same coordinate convention as band OCR: no y
// crop, so a word's TSV `top` is already capture-y * UPSCALE, the same
// preprocessed-pixel space OcrLine::y_top uses everywhere else.

/// Left crop fraction for the whole-panel pass. Kept separate from the
/// band pipeline's ICON_CUT (0.30): band detection's per-row brightness
/// average is more sensitive to a partial icon still being inside the
/// sampled strip than a single whole-panel OCR pass ever was, so the two
/// were split when band OCR was introduced. 0.25 is the original
/// milestone-0 value this restores.
#[cfg(ocr)]
const WHOLE_ICON_CUT: f32 = 0.25;

/// Crops the icon column, upscales by UPSCALE, and min-max normalizes.
/// Input must already be grayscale; caller converts from the capture
/// format. Mirrors `ocr_one_band`'s crop/upscale, but over the whole
/// region (no y crop) and with an extra contrast-normalize step the band
/// pipeline doesn't need (a single band crop is already a tight, mostly
/// on-brightness strip; the whole panel's much wider brightness range
/// benefits from being stretched to full contrast before tesseract sees
/// it).
#[cfg(ocr)]
fn whole_preprocess(region: &GrayImage) -> GrayImage {
    let cut = (region.width() as f32 * WHOLE_ICON_CUT) as u32;
    let text = imageops::crop_imm(region, cut, 0, region.width() - cut, region.height()).to_image();
    let up = imageops::resize(
        &text,
        text.width() * UPSCALE,
        text.height() * UPSCALE,
        imageops::FilterType::Lanczos3,
    );
    whole_normalize(up)
}

#[cfg(ocr)]
fn whole_normalize(mut img: GrayImage) -> GrayImage {
    let (mut lo, mut hi) = (255u8, 0u8);
    for p in img.pixels() {
        lo = lo.min(p.0[0]);
        hi = hi.max(p.0[0]);
    }
    if hi > lo {
        let span = (hi - lo) as f32;
        for p in img.pixels_mut() {
            p.0[0] = (((p.0[0] - lo) as f32 / span) * 255.0) as u8;
        }
    }
    img
}

/// Runs tesseract psm 6 over a preprocessed whole-panel image and groups
/// the result into rows. `None` on any tesseract failure (missing binary,
/// non-zero exit), same "one bad pass never kills the pipeline" contract
/// as `ocr_one_band`.
#[cfg(ocr)]
fn run_whole_tesseract(engine: &mut OcrEngine, pre: &GrayImage) -> anyhow::Result<Vec<OcrLine>> {
    Ok(parse_whole_tsv(&engine.tsv(pre)?))
}

/// Builds OcrLines from a whole-panel tesseract TSV pass. Public so the
/// grouping logic is directly unit-testable without a real tesseract
/// process, same as `parse_band_tsv`. Unlike a band crop (one row per
/// crop, by construction), a whole-panel pass sees every row in one TSV
/// dump, so words are grouped back into rows by tesseract's own
/// (block, par, line) key.
pub fn parse_whole_tsv(tsv: &str) -> Vec<OcrLine> {
    // (filtered words, all words, min top, max bottom) accumulated per
    // tesseract (block, par, line) key.
    type RowAcc = (Vec<String>, Vec<String>, u32, u32);
    let mut acc: BTreeMap<(u32, u32, u32), RowAcc> = BTreeMap::new();
    for row in tsv.lines().skip(1) {
        let f: Vec<&str> = row.split('\t').collect();
        if f.len() < 12 || f[0] != "5" {
            continue;
        }
        let (Ok(block), Ok(par), Ok(line)) = (f[2].parse(), f[3].parse(), f[4].parse()) else {
            continue;
        };
        let (Ok(top), Ok(height)) = (f[7].parse::<u32>(), f[9].parse::<u32>()) else {
            continue;
        };
        let conf: f32 = f[10].parse().unwrap_or(-1.0);
        let word = f[11].trim();
        if word.is_empty() {
            continue;
        }
        let e = acc
            .entry((block, par, line))
            .or_insert_with(|| (Vec::new(), Vec::new(), u32::MAX, 0));
        if conf >= MIN_CONF {
            e.0.push(word.to_string());
        }
        e.1.push(word.to_string());
        e.2 = e.2.min(top);
        e.3 = e.3.max(top + height);
    }
    let mut lines: Vec<OcrLine> = acc
        .into_values()
        .filter_map(|(fw, uw, top, bottom)| {
            let unfiltered = khaloni_poe2_core::matcher::normalize(&uw.join(" "));
            if unfiltered.trim().is_empty() || !has_alpha_run(&unfiltered, MIN_WORD_RUN) {
                return None;
            }
            Some(OcrLine {
                filtered: khaloni_poe2_core::matcher::normalize(&fw.join(" ")),
                unfiltered,
                y_top: top,
                height: bottom.saturating_sub(top),
            })
        })
        .collect();
    lines.sort_by_key(|l| l.y_top);
    lines
}

/// Runs the whole-panel OCR pipeline end to end: preprocess, tesseract,
/// parse. `Vec::new()` on any tesseract failure rather than propagating an
/// error, matching `ocr_bands`' "OCR problems degrade to fewer rows, never
/// a crash" contract.
#[cfg(ocr)]
pub fn ocr_whole_panel(engine: &mut OcrEngine, gray: &GrayImage) -> Vec<OcrLine> {
    let pre = whole_preprocess(gray);
    run_whole_tesseract(engine, &pre).unwrap_or_default()
}

/// How much real text a line's `filtered` field recovered: total ASCII
/// alphabetic characters, ignoring digits/spaces/punctuation. Used by
/// `union_ocr_lines` to pick the better of two candidates for the same
/// row.
///
/// A plain word count (`split_whitespace().count()`) was tried first,
/// matching the "longer filtered text... more words recovered" brief
/// literally. Measured against spikes/ocr/samples/s5.png (a tall panel
/// with 6 skill-gem rows) it actively picks the WRONG winner: one band's
/// crop garbled "Skill Level 20: Animus Exchange" into 6 short
/// space-separated noise tokens ("s s 8 bl vll llbv"), which by raw word
/// count (6) beat the whole-panel pass's correct "skill level 20 animus
/// exchange" (5 tokens) - the real row's text lost to noise and the row
/// silently dropped (gem_row never matched the noise). Counting
/// alphabetic characters instead scores the noise fragment at 11 and the
/// real text at 24: short junk fragments no longer outscore genuine
/// words just by being numerous.
fn recovered_alpha_chars(line: &OcrLine) -> usize {
    line.filtered.chars().filter(char::is_ascii_alphabetic).count()
}

/// Merges the band-OCR and whole-panel-OCR passes into one logical set of
/// rows. See the evidence block above `ocr_whole_panel`: the two pipelines
/// fail on disjoint panel styles, so rather than choosing one per panel
/// (which would need to detect the style first - itself another
/// error-prone classifier), both always run, and their outputs are
/// unioned here.
///
/// A band line and a whole-panel line are the same logical row when their
/// y-ranges (`y_top`..`y_top+height`) overlap by more than half the
/// shorter of the two heights - loose enough to survive the small
/// per-pass boundary differences (band detection's own y0/y1 vs.
/// tesseract's own reported line bounds) without conflating two distinct,
/// merely-adjacent rows.
///
/// A band line can satisfy that overlap test against MORE than one
/// whole-panel line: the odd-height/garbled bands this union exists to
/// paper over (see `ocr_whole_panel`'s evidence block) are often taller
/// than a single real text line, so they straddle two of tesseract's own
/// (block,par,line) groups from the whole-panel pass - one real, one
/// noise. Measured on s5.png: a 435px-tall band spanning "Skill Level 20:
/// Runic Reprieve" overlapped both the whole-panel pass's real text line
/// AND an adjacent noise-only line above it enough to pass the test on
/// both. An earlier version of this function picked only the
/// single largest-raw-overlap whole-panel partner per band line; on that
/// fixture the larger-overlap partner happened to be the noise line, so
/// the real whole-panel line was left "unmatched" and leaked through as a
/// spurious duplicate row a few pixels below the correct one. Fixed by
/// consuming EVERY whole-panel line that clears the overlap test for a
/// given band (never just the best-overlapping one), so nothing from a
/// matched region can leak through as a stray duplicate.
///
/// Once a band's full set of overlapping whole-panel lines is collected,
/// exactly one candidate survives to represent the row: the one with the
/// most `recovered_alpha_chars` among the band line and all of its
/// matched whole-panel lines (see that function's doc comment for why
/// alpha-char count, not word count). Ties keep the band line
/// (deterministic; band OCR is the pass this project has leaned on the
/// longest). Lines with no overlapping counterpart in the other pass are
/// kept as-is - most rows only ever come from one pipeline on any given
/// panel style. The result is sorted by y_top.
pub fn union_ocr_lines(band_lines: Vec<OcrLine>, whole_lines: Vec<OcrLine>) -> Vec<OcrLine> {
    let mut whole_used = vec![false; whole_lines.len()];
    let mut merged: Vec<OcrLine> = Vec::with_capacity(band_lines.len() + whole_lines.len());

    for b in band_lines {
        let b_y1 = b.y_top + b.height;
        let matched: Vec<usize> = whole_lines
            .iter()
            .enumerate()
            .filter(|(i, w)| {
                if whole_used[*i] {
                    return false;
                }
                let w_y1 = w.y_top + w.height;
                let overlap_start = b.y_top.max(w.y_top);
                let overlap_end = b_y1.min(w_y1);
                if overlap_end <= overlap_start {
                    return false;
                }
                let overlap = overlap_end - overlap_start;
                let min_h = b.height.min(w.height);
                overlap * 2 > min_h
            })
            .map(|(i, _)| i)
            .collect();

        if matched.is_empty() {
            merged.push(b);
            continue;
        }
        let mut winner = b;
        let mut winner_score = recovered_alpha_chars(&winner);
        for i in matched {
            whole_used[i] = true;
            let score = recovered_alpha_chars(&whole_lines[i]);
            if score > winner_score {
                winner_score = score;
                winner = whole_lines[i].clone();
            }
        }
        merged.push(winner);
    }
    for (i, w) in whole_lines.into_iter().enumerate() {
        if !whole_used[i] {
            merged.push(w);
        }
    }

    merged.sort_by_key(|l| l.y_top);
    merged
}

/// Top-level OCR entry point: runs the band pipeline and the whole-panel
/// pipeline concurrently (they're independent tesseract processes over
/// the same source image, so there's no reason to serialize them) and
/// unions the results. See `union_ocr_lines` and the evidence block above
/// `ocr_whole_panel` for why both passes run unconditionally rather than
/// picking one.
#[cfg(ocr)]
pub fn ocr_scan(engine: &mut OcrEngine, gray: &GrayImage) -> Vec<OcrLine> {
    ocr_scan_gated(engine, gray, &mut WholePanelGate::always())
}

/// Rate limiter for the whole-panel pass on band-less frames. Bright
/// noise scenes (measured live: full-screen fire effects during combat)
/// open the brightness gate and cost 1-2 s of tesseract per frame with
/// zero yield, collapsing scan cadence exactly during fights. When bar
/// structure exists the whole-panel pass always runs (the union needs
/// it); without bars it runs at most once per interval, which still
/// catches an under-threshold panel within a second.
#[cfg(ocr)]
pub struct WholePanelGate {
    last: Option<std::time::Instant>,
    interval: std::time::Duration,
}

#[cfg(ocr)]
impl WholePanelGate {
    pub fn new(interval: std::time::Duration) -> WholePanelGate {
        WholePanelGate { last: None, interval }
    }
    /// A gate that never limits (headless/scanimg one-shot use).
    pub fn always() -> WholePanelGate {
        WholePanelGate { last: None, interval: std::time::Duration::ZERO }
    }
    fn allow(&mut self) -> bool {
        let now = std::time::Instant::now();
        match self.last {
            Some(t) if now.duration_since(t) < self.interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

#[cfg(ocr)]
pub fn ocr_scan_gated(
    engine: &mut OcrEngine,
    gray: &GrayImage,
    whole_gate: &mut WholePanelGate,
) -> Vec<OcrLine> {
    let bands = detect_bands(gray);
    let band_lines = ocr_bands(engine, gray, &bands);
    let whole_lines = if !bands.is_empty() || whole_gate.allow() {
        ocr_whole_panel(engine, gray)
    } else {
        Vec::new()
    };
    union_ocr_lines(band_lines, whole_lines)
}
