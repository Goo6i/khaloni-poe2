//! Expedition Island Rumour recognizer (CV + OCR), the app-side half of
//! the port. The pure geometry (line boxes, anchor location, region crop)
//! lives in `poe2_lens_core::rumour_scan`; this module adds the parts that
//! need `image`/`tesseract`: the bright-parchment panel finder and the
//! full recognizer that OCRs the tooltip and resolves each line to a rumour.
//!
//! Method mirrors the danielmtv2/poe2-expedition-overlay approach proven in
//! the Python spike (8/10 recall, 0 false positives on 5 real 4K frames).

#[cfg(ocr)]
use std::collections::HashSet;

#[cfg(ocr)]
use image::imageops;
use image::GrayImage;
use poe2_lens_core::rumour::RumourEntry;
#[cfg(ocr)]
use poe2_lens_core::rumour::RumourIndex;
#[cfg(ocr)]
use poe2_lens_core::rumour_scan::{parse_rumour_tsv, RumourLine};
use poe2_lens_core::rumour_scan::Rect;

#[cfg(ocr)]
use crate::ocr::OcrEngine;

/// OCR passes unioned per scan, as (upscale, page-segmentation mode).
/// Multiple scales because tesseract groups/drops lines differently per
/// scale; both PSM 6 (uniform block, the fixture-proven mode) and PSM 11
/// (sparse text) because PSM 11 reads the game's stylized cursive rumour
/// names (e.g. "Nothin' to drink") that PSM 6 garbles, while PSM 6 anchors
/// the clean cases. Every line still clears the strict match threshold, so
/// unioning only raises recall, never false positives.
pub const OCR_PASSES: [(f32, u32); 5] = [(1.0, 6), (1.5, 6), (2.0, 6), (1.0, 11), (2.0, 11)];
/// Padding applied around the detected panel before OCR. `find_panel`'s box
/// hugs the parchment and can clip a rumour line at the top/bottom edge;
/// the Y padding recovers those. X padding stays tight so cross-screen text
/// never enters the crop (measured on the 5 real fixtures).
pub const CROP_PAD_X: u32 = 16;
pub const CROP_PAD_Y_UP: u32 = 40;
pub const CROP_PAD_Y_DN: u32 = 80;

/// One recognized rumour with its on-screen geometry, in full-frame pixels.
#[derive(Debug, Clone)]
pub struct RumourHit {
    pub entry: RumourEntry,
    /// The matched text line's box (badge anchor fallback).
    pub line: Rect,
    /// Raw OCR text that matched (for logging/debug).
    pub raw: String,
    /// The tooltip panel box: rating badges hang off its right edge.
    pub panel: Rect,
}

/// Recognize every Island Rumour in a full frame. Locate the tooltip by its
/// bright parchment panel (a cheap CV pass, no OCR on idle frames), OCR the
/// padded panel at several scales, and union the matches (first per rumour).
///
/// Panel-first rather than the Python spike's anchor-first: the port runs
/// leptess, whose full-frame line grouping merges the "UNCHARTED WATERS"
/// title with far-apart UI text into one wide line, which blows the crop
/// column up to full width and pulls in cross-screen false positives. The
/// panel box is tight and reliable on every real fixture, and OCRing only
/// it is also far cheaper than a full-frame anchor pre-scan every poll.
#[cfg(ocr)]
pub fn recognize(engine: &mut OcrEngine, gray: &GrayImage, index: &RumourIndex) -> Vec<RumourHit> {
    let Some(panel) = find_panel(gray) else {
        return Vec::new();
    };
    let cx0 = panel.x0.saturating_sub(CROP_PAD_X);
    let cy0 = panel.y0.saturating_sub(CROP_PAD_Y_UP);
    let cx1 = (panel.x1 + CROP_PAD_X).min(gray.width());
    let cy1 = (panel.y1 + CROP_PAD_Y_DN).min(gray.height());
    if cx1 <= cx0 || cy1 <= cy0 {
        return Vec::new();
    }
    let crop = imageops::crop_imm(gray, cx0, cy0, cx1 - cx0, cy1 - cy0).to_image();

    let mut hits: Vec<RumourHit> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (scale, psm) in OCR_PASSES {
        let mut lines = ocr_scaled(engine, &crop, scale, psm);
        lines.sort_by_key(RumourLine::yc);
        for ln in lines {
            if let Some(entry) = index.match_line(&ln.text) {
                if seen.insert(entry.rumour.clone()) {
                    hits.push(RumourHit {
                        entry: entry.clone(),
                        line: Rect {
                            x0: cx0 + ln.x0,
                            y0: cy0 + ln.y0,
                            x1: cx0 + ln.x1,
                            y1: cy0 + ln.y1,
                        },
                        raw: ln.text,
                        panel,
                    });
                }
            }
        }
    }
    hits
}

/// OCR `img` at `scale` and `psm`, returning line boxes mapped back to
/// `img`'s own pixel space. `scale` > 1 upsamples so tesseract reads small
/// text better; `scale` < 1 downsamples for a cheap pre-scan.
#[cfg(ocr)]
fn ocr_scaled(engine: &mut OcrEngine, img: &GrayImage, scale: f32, psm: u32) -> Vec<RumourLine> {
    let native = (scale - 1.0).abs() < f32::EPSILON;
    let scaled;
    let target = if native {
        img
    } else {
        let nw = ((img.width() as f32 * scale) as u32).max(1);
        let nh = ((img.height() as f32 * scale) as u32).max(1);
        scaled = imageops::resize(img, nw, nh, imageops::FilterType::Lanczos3);
        &scaled
    };
    let Some(tsv) = engine.tsv_of_psm(target, psm) else {
        return Vec::new();
    };
    let mut lines = parse_rumour_tsv(&tsv);
    if !native {
        for l in &mut lines {
            l.x0 = (l.x0 as f32 / scale) as u32;
            l.y0 = (l.y0 as f32 / scale) as u32;
            l.x1 = (l.x1 as f32 / scale) as u32;
            l.y1 = (l.y1 as f32 / scale) as u32;
        }
    }
    lines
}

/// Downscale factor for the panel search: morphology and labeling run on a
/// 1/N subsample so the poll loop stays cheap (Python spike: 4).
const PANEL_STEP: u32 = 4;
/// Brightness a subsampled pixel must exceed to count as parchment.
const PANEL_THRESH: u8 = 150;
/// Morphological-closing iterations to bridge the dark text holes inside
/// the parchment so it labels as one solid blob (Python spike: 2).
const CLOSE_ITERS: u32 = 2;
/// Accept only blobs whose full-resolution bounds and fill ratio match a
/// tooltip panel (Python spike ranges; panel is ~620x390 at 4K).
const MIN_W: u32 = 350;
const MAX_W: u32 = 900;
const MIN_H: u32 = 250;
const MAX_H: u32 = 1000;
const MIN_FILL: f64 = 0.6;

/// Locate the bright parchment "Uncharted Waters" tooltip anywhere on the
/// frame. Subsample, threshold, close the text holes, label connected
/// components, and return the largest panel-shaped bright blob in
/// full-resolution pixels, or `None` if none qualifies.
pub fn find_panel(gray: &GrayImage) -> Option<Rect> {
    let step = PANEL_STEP;
    let (gw, gh) = (gray.width(), gray.height());
    // Subsample to a small mask (Python `gray[::step, ::step] > thresh`).
    let sw = gw.div_ceil(step);
    let sh = gh.div_ceil(step);
    if sw == 0 || sh == 0 {
        return None;
    }
    let mut mask = vec![false; (sw * sh) as usize];
    for sy in 0..sh {
        for sx in 0..sw {
            let p = gray.get_pixel(sx * step, sy * step).0[0];
            mask[(sy * sw + sx) as usize] = p > PANEL_THRESH;
        }
    }
    // Close the dark text holes so the parchment labels as one blob.
    let closed = binary_close(&mask, sw, sh, CLOSE_ITERS);
    // Largest panel-shaped connected component.
    let mut best: Option<(u32, Rect)> = None; // (pixel count, full-res box)
    for comp in connected_components(&closed, sw, sh) {
        let bw = (comp.maxx - comp.minx + 1) * step;
        let bh = (comp.maxy - comp.miny + 1) * step;
        let bbox_area = (comp.maxx - comp.minx + 1) * (comp.maxy - comp.miny + 1);
        let fill = f64::from(comp.count) / f64::from(bbox_area.max(1));
        if (MIN_W..MAX_W).contains(&bw)
            && (MIN_H..MAX_H).contains(&bh)
            && fill > MIN_FILL
            && best.is_none_or(|(c, _)| comp.count > c)
        {
            best = Some((
                comp.count,
                Rect {
                    x0: comp.minx * step,
                    y0: comp.miny * step,
                    x1: (comp.maxx + 1) * step,
                    y1: (comp.maxy + 1) * step,
                },
            ));
        }
    }
    best.map(|(_, r)| r)
}

/// One connected component's pixel count and inclusive small-space bounds.
struct Component {
    count: u32,
    minx: u32,
    miny: u32,
    maxx: u32,
    maxy: u32,
}

/// 4-connectivity connected components over a boolean mask (matches
/// scipy.ndimage.label's default orthogonal structure).
fn connected_components(mask: &[bool], w: u32, h: u32) -> Vec<Component> {
    let mut seen = vec![false; mask.len()];
    let mut out = Vec::new();
    let idx = |x: u32, y: u32| (y * w + x) as usize;
    for sy in 0..h {
        for sx in 0..w {
            let start = idx(sx, sy);
            if !mask[start] || seen[start] {
                continue;
            }
            let mut stack = vec![(sx, sy)];
            seen[start] = true;
            let mut c = Component { count: 0, minx: sx, miny: sy, maxx: sx, maxy: sy };
            while let Some((x, y)) = stack.pop() {
                c.count += 1;
                c.minx = c.minx.min(x);
                c.miny = c.miny.min(y);
                c.maxx = c.maxx.max(x);
                c.maxy = c.maxy.max(y);
                let mut push = |nx: u32, ny: u32, stack: &mut Vec<(u32, u32)>| {
                    let n = idx(nx, ny);
                    if mask[n] && !seen[n] {
                        seen[n] = true;
                        stack.push((nx, ny));
                    }
                };
                if x > 0 {
                    push(x - 1, y, &mut stack);
                }
                if x + 1 < w {
                    push(x + 1, y, &mut stack);
                }
                if y > 0 {
                    push(x, y - 1, &mut stack);
                }
                if y + 1 < h {
                    push(x, y + 1, &mut stack);
                }
            }
            out.push(c);
        }
    }
    out
}

/// Morphological closing: `iters` of 3x3 (8-connectivity) dilation then the
/// same number of erosions, matching scipy.ndimage.binary_closing.
fn binary_close(mask: &[bool], w: u32, h: u32, iters: u32) -> Vec<bool> {
    let mut m = mask.to_vec();
    for _ in 0..iters {
        m = morph(&m, w, h, true);
    }
    for _ in 0..iters {
        m = morph(&m, w, h, false);
    }
    m
}

/// One 3x3 morphology pass. `dilate`: true if ANY neighbor is set;
/// erode: true only if ALL neighbors (in-bounds) are set. Erosion treats
/// out-of-bounds as unset (scipy border_value=0), so edge pixels erode.
fn morph(mask: &[bool], w: u32, h: u32, dilate: bool) -> Vec<bool> {
    let idx = |x: u32, y: u32| (y * w + x) as usize;
    let mut out = vec![false; mask.len()];
    for y in 0..h {
        for x in 0..w {
            let mut any = false;
            let mut all = true;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    let set = nx >= 0
                        && ny >= 0
                        && (nx as u32) < w
                        && (ny as u32) < h
                        && mask[idx(nx as u32, ny as u32)];
                    any |= set;
                    all &= set;
                }
            }
            out[idx(x, y)] = if dilate { any } else { all };
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Black frame with one filled white rectangle.
    fn frame_with_rect(w: u32, h: u32, rx0: u32, ry0: u32, rx1: u32, ry1: u32) -> GrayImage {
        let mut img = GrayImage::new(w, h);
        for y in ry0..ry1 {
            for x in rx0..rx1 {
                img.put_pixel(x, y, image::Luma([255]));
            }
        }
        img
    }

    #[test]
    fn find_panel_locates_a_panel_sized_bright_box() {
        // 600x400 bright box: within the panel size gates.
        let img = frame_with_rect(1000, 800, 200, 100, 800, 500);
        let r = find_panel(&img).expect("panel found");
        // Bounds snap to the 4px subsample grid; the box is grid-aligned here.
        assert_eq!(r, Rect { x0: 200, y0: 100, x1: 800, y1: 500 });
    }

    #[test]
    fn find_panel_rejects_a_too_small_bright_blob() {
        // 100x100 bright box: below MIN_W/MIN_H, not a panel.
        let img = frame_with_rect(1000, 800, 200, 100, 300, 200);
        assert!(find_panel(&img).is_none(), "small blob is not a panel");
    }

    #[test]
    fn find_panel_returns_none_on_a_dark_frame() {
        let img = GrayImage::new(1000, 800);
        assert!(find_panel(&img).is_none(), "nothing bright to find");
    }
}
