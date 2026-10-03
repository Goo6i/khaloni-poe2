//! Expedition Island Rumour recognizer (CV + OCR), the app-side half of
//! the port. The pure geometry (line boxes, anchor location, region crop)
//! lives in `khaloni_poe2_core::rumour_scan`; this module adds the parts that
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
use khaloni_poe2_core::rumour::RumourEntry;
#[cfg(ocr)]
use khaloni_poe2_core::rumour::RumourIndex;
#[cfg(ocr)]
use khaloni_poe2_core::rumour_scan::{has_tooltip_chrome, parse_rumour_tsv, RumourLine};
use khaloni_poe2_core::rumour_scan::Rect;

#[cfg(ocr)]
use crate::ocr::OcrEngine;
use crate::ocr::UiScale;

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
/// All three are 4K measurements and scale with the frame (`UiScale`).
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
    RumourScanner::default().recognize(engine, gray, index)
}

/// The padded tooltip crop `recognize` reads, as (crop, x offset, y
/// offset, panel box), or None when no tooltip-shaped panel is on screen.
#[cfg(ocr)]
fn panel_crop(gray: &GrayImage) -> Option<(GrayImage, u32, u32, Rect)> {
    let panel = find_panel(gray)?;
    let scale = UiScale::from_frame_height(gray.height());
    let cx0 = panel.x0.saturating_sub(scale.px(CROP_PAD_X));
    let cy0 = panel.y0.saturating_sub(scale.px(CROP_PAD_Y_UP));
    let cx1 = (panel.x1 + scale.px(CROP_PAD_X)).min(gray.width());
    let cy1 = (panel.y1 + scale.px(CROP_PAD_Y_DN)).min(gray.height());
    if cx1 <= cx0 || cy1 <= cy0 {
        return None;
    }
    Some((imageops::crop_imm(gray, cx0, cy0, cx1 - cx0, cy1 - cy0).to_image(), cx0, cy0, panel))
}

/// The parchment a recognition read, kept to tell whether a later frame
/// still shows the same tooltip. Only the panel interior is kept: the crop
/// padding around it shows the game world, which animates on every frame
/// (water, particles, the day cycle) while the tooltip stays put.
#[cfg(ocr)]
struct SeenPanel {
    /// Where the interior sits in the frame.
    at: Rect,
    pixels: GrayImage,
    hits: Vec<RumourHit>,
}

/// How far inside the detected panel box its interior starts, in 4K pixels.
/// The box snaps to the panel search's subsample grid and can take in up
/// to one grid step of the world past the parchment's true edge; two steps
/// keep that out.
#[cfg(ocr)]
const INTERIOR_INSET: u32 = 2 * PANEL_STEP;
/// A pixel counts as changed when it moved by more than this many grey
/// levels: well above the compositor's rounding noise, well below the
/// contrast of ink on parchment.
#[cfg(ocr)]
const PIXEL_NOISE: u8 = 32;
/// Changed pixels tolerated per million before the tooltip counts as
/// changed. One rumour name is thousands of ink pixels (about 1% of the
/// interior at 4K), so 0.1% cannot hide a changed or missing rumour.
#[cfg(ocr)]
const CHANGED_PER_MILLION: u64 = 1_000;

#[cfg(ocr)]
fn panel_interior(gray: &GrayImage, panel: Rect) -> Option<Rect> {
    let inset = UiScale::from_frame_height(gray.height()).px(INTERIOR_INSET);
    let at = Rect {
        x0: panel.x0 + inset,
        y0: panel.y0 + inset,
        x1: panel.x1.min(gray.width()).saturating_sub(inset),
        y1: panel.y1.min(gray.height()).saturating_sub(inset),
    };
    (at.x1 > at.x0 && at.y1 > at.y0).then_some(at)
}

#[cfg(ocr)]
impl SeenPanel {
    /// Whether `gray` shows the same interior at the same place. The
    /// comparison is at fixed frame coordinates, so a tooltip that moved
    /// by even a few pixels reads as changed; the new panel box only has
    /// to cover the old interior, since bright world pixels touching the
    /// parchment can shift the box's edges by a grid step.
    fn unchanged_in(&self, gray: &GrayImage, panel: Rect) -> bool {
        let at = self.at;
        if at.x0 < panel.x0 || at.y0 < panel.y0 || at.x1 > panel.x1 || at.y1 > panel.y1 {
            return false;
        }
        if at.x1 > gray.width() || at.y1 > gray.height() {
            return false;
        }
        let area = u64::from(at.width()) * u64::from(at.height());
        let allowed = area * CHANGED_PER_MILLION / 1_000_000;
        let mut changed = 0u64;
        for (dy, row) in self.pixels.rows().enumerate() {
            let y = at.y0 + dy as u32;
            for (dx, old) in row.enumerate() {
                let new = gray.get_pixel(at.x0 + dx as u32, y).0[0];
                if new.abs_diff(old.0[0]) > PIXEL_NOISE {
                    changed += 1;
                    if changed > allowed {
                        return false;
                    }
                }
            }
        }
        true
    }
}

/// The tooltip's three rumour rows, as (top, bottom) offsets from the
/// panel's top edge in 4K pixels, below the "Island Rumours" banner and
/// clear of the separators between rows. Measured on the five fixtures,
/// where the names' ink spans 164-208, 240-290 and 324-372.
#[cfg(ocr)]
const ROW_SLOTS: [(u32, u32); MAX_RUMOURS] = [(152, 228), (232, 312), (316, 384)];
/// Pixels darker than this are ink on the parchment.
#[cfg(ocr)]
const INK_MAX: u8 = 80;
/// Ink pixels (4K) that make a row slot filled. On the five fixtures a
/// filled slot holds 1,472 to 2,750 ink pixels and an empty one at most 6,
/// so the line sits far from both.
#[cfg(ocr)]
const FILLED_SLOT_INK: u32 = 500;
/// Columns left out of the slot scan at either side, clear of the
/// parchment's darker border (4K pixels).
#[cfg(ocr)]
const SLOT_SIDE: u32 = 40;

/// How many of the tooltip's row slots carry a name, from the ink in each.
/// An empty slot is plain parchment, so a tooltip showing one or two
/// rumours can stop reading once those resolve instead of running every
/// pass hunting for rows that are not there.
#[cfg(ocr)]
fn filled_slots(gray: &GrayImage, panel: Rect) -> usize {
    let scale = UiScale::from_frame_height(gray.height());
    let x0 = panel.x0 + scale.px(SLOT_SIDE);
    let x1 = panel.x1.min(gray.width()).saturating_sub(scale.px(SLOT_SIDE));
    let floor = (FILLED_SLOT_INK as f32 * scale.factor() * scale.factor()) as u32;
    ROW_SLOTS
        .iter()
        .filter(|(top, bottom)| {
            let y0 = panel.y0 + scale.px(*top);
            let y1 = (panel.y0 + scale.px(*bottom)).min(gray.height());
            let mut ink = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    ink += u32::from(gray.get_pixel(x, y).0[0] < INK_MAX);
                }
            }
            ink > floor.max(1)
        })
        .count()
}

/// A tooltip lists at most this many rumours: its three row slots sit at
/// fixed heights between the header and the "REQUIRES" line on every
/// fixture, whether one, two or all three are filled.
#[cfg(ocr)]
const MAX_RUMOURS: usize = 3;

/// `recognize` with memory, for the worker that polls full frames. Any
/// parchment-bright blob of tooltip size sends a frame to OCR - the
/// tooltip itself for as long as it is held open, but also sunlit ground
/// and spell effects - and the full recipe is five tesseract passes.
/// Three savings, none of which can change what a tooltip resolves to:
/// a tooltip whose parchment is unchanged in place returns the previous
/// hits without OCR, however the world around it moves; a pass that
/// leaves the scan with no rumour and read no tooltip chrome ends it (every
/// pass reads chrome on every real tooltip, see `has_tooltip_chrome`); and
/// the passes stop once every filled row slot has resolved. A pass whose own
/// lines all resolved is NOT a stopping point: measured on rumour-2, the
/// first pass reads two rumours cleanly and drops the third line
/// altogether, which only the sparse-text passes recover.
#[cfg(ocr)]
#[derive(Default)]
pub struct RumourScanner {
    last: Option<SeenPanel>,
    /// Tesseract passes run so far: the observable the tests and the
    /// worker's trace line read.
    pub ocr_passes: usize,
}

#[cfg(ocr)]
impl RumourScanner {
    pub fn recognize(
        &mut self,
        engine: &mut OcrEngine,
        gray: &GrayImage,
        index: &RumourIndex,
    ) -> Vec<RumourHit> {
        let Some((crop, cx0, cy0, panel)) = panel_crop(gray) else {
            self.last = None;
            return Vec::new();
        };
        if let Some(seen) = &self.last {
            if seen.unchanged_in(gray, panel) {
                return seen.hits.clone();
            }
        }
        // A tooltip names one to three rumours. When the ink shows no
        // filled slot at all the geometry did not fit this panel, so the
        // count is not trusted and every slot is assumed filled.
        let wanted = match filled_slots(gray, panel) {
            0 => MAX_RUMOURS,
            n => n,
        };
        // Tesseract reads text at the size it has at 4K; smaller frames
        // are enlarged to match before the per-pass scales apply.
        let base = 1.0 / UiScale::from_frame_height(gray.height()).factor().min(1.0);

        let mut hits: Vec<RumourHit> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for (scale, psm) in OCR_PASSES {
            self.ocr_passes += 1;
            let mut lines = ocr_scaled(engine, &crop, scale * base, psm);
            lines.sort_by_key(RumourLine::yc);
            let chrome = has_tooltip_chrome(&lines);
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
            if hits.len() >= wanted || (hits.is_empty() && !chrome) {
                break;
            }
        }
        self.last = panel_interior(gray, panel).map(|at| SeenPanel {
            at,
            pixels: imageops::crop_imm(gray, at.x0, at.y0, at.width(), at.height()).to_image(),
            hits: hits.clone(),
        });
        hits
    }
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

/// Downscale factor for the panel search at 4K: morphology and labeling
/// run on a 1/N subsample so the poll loop stays cheap (Python spike: 4).
/// Scaled with the frame, so the mask has the same grid relative to the
/// interface at every resolution and CLOSE_ITERS - which counts mask
/// pixels - bridges the same text holes: a fixed step of 4 on a 1080p
/// frame doubles the closing reach relative to the tooltip and welds it
/// to whatever bright scenery sits beside it.
const PANEL_STEP: u32 = 4;
/// Brightness a subsampled pixel must exceed to count as parchment.
const PANEL_THRESH: u8 = 150;
/// Morphological-closing iterations to bridge the dark text holes inside
/// the parchment so it labels as one solid blob (Python spike: 2).
const CLOSE_ITERS: u32 = 2;
/// Accept only blobs whose full-resolution bounds and fill ratio match a
/// tooltip panel (Python spike ranges; panel is ~620x390 at 4K). 4K
/// pixels, scaled with the frame: at 1080p the tooltip is ~310x195, under
/// the unscaled floor.
const MIN_W: u32 = 350;
const MAX_W: u32 = 900;
const MIN_H: u32 = 250;
const MAX_H: u32 = 1000;
const MIN_FILL: f64 = 0.6;

/// One bright blob from the parchment sweep: full-resolution bounds plus
/// the mask-space mass and fill the size/shape gates are judged on. Public
/// so `autoregion` can reuse the sweep with its own reward-panel gates
/// (the reward panel is ~990x1030 at 4K, outside `find_panel`'s
/// tooltip-sized MAX_W/MAX_H).
#[derive(Debug, Clone, Copy)]
pub struct PanelCandidate {
    /// Blob bounding box in full-resolution pixels (subsample-grid
    /// aligned; x1/y1 can overshoot the frame edge by up to one grid
    /// step, callers cropping must clamp).
    pub rect: Rect,
    /// Blob pixel count in the subsampled mask — `find_panel`'s selection
    /// key (heaviest gate-passing blob wins).
    pub count: u32,
    /// count / mask bounding-box area: how solid the blob is.
    pub fill: f64,
}

/// The component sweep behind `find_panel`: subsample, threshold, close
/// the text holes, label connected components, and return EVERY bright
/// blob ungated. `find_panel` (rumour tooltip) and
/// `autoregion::detect_reward_region` (reward panel) apply their own
/// size/fill gates on top.
pub fn panel_candidates(gray: &GrayImage) -> Vec<PanelCandidate> {
    let step = UiScale::from_frame_height(gray.height()).px(PANEL_STEP);
    let (gw, gh) = (gray.width(), gray.height());
    // Subsample to a small mask (Python `gray[::step, ::step] > thresh`).
    let sw = gw.div_ceil(step);
    let sh = gh.div_ceil(step);
    if sw == 0 || sh == 0 {
        return Vec::new();
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
    connected_components(&closed, sw, sh)
        .into_iter()
        .map(|comp| {
            let bbox_area = (comp.maxx - comp.minx + 1) * (comp.maxy - comp.miny + 1);
            PanelCandidate {
                rect: Rect {
                    x0: comp.minx * step,
                    y0: comp.miny * step,
                    x1: (comp.maxx + 1) * step,
                    y1: (comp.maxy + 1) * step,
                },
                count: comp.count,
                fill: f64::from(comp.count) / f64::from(bbox_area.max(1)),
            }
        })
        .collect()
}

/// Locate the bright parchment "Uncharted Waters" tooltip anywhere on the
/// frame: the largest panel-shaped bright blob from the candidate sweep,
/// in full-resolution pixels, or `None` if none qualifies.
pub fn find_panel(gray: &GrayImage) -> Option<Rect> {
    let scale = UiScale::from_frame_height(gray.height());
    let (min_w, max_w) = (scale.px(MIN_W), scale.px(MAX_W));
    let (min_h, max_h) = (scale.px(MIN_H), scale.px(MAX_H));
    let mut best: Option<(u32, Rect)> = None; // (pixel count, full-res box)
    for cand in panel_candidates(gray) {
        // rect is subsample-grid aligned, so width/height here equal the
        // (maxx - minx + 1) * step the gates were originally tuned on.
        let bw = cand.rect.width();
        let bh = cand.rect.height();
        if (min_w..max_w).contains(&bw)
            && (min_h..max_h).contains(&bh)
            && cand.fill > MIN_FILL
            && best.is_none_or(|(c, _)| cand.count > c)
        {
            best = Some((cand.count, cand.rect));
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
        // A 4K frame with a 620x392 bright box: the real tooltip's size.
        let img = frame_with_rect(3840, 2160, 2000, 400, 2620, 792);
        let r = find_panel(&img).expect("panel found");
        // Bounds snap to the 4px subsample grid; the box is grid-aligned here.
        assert_eq!(r, Rect { x0: 2000, y0: 400, x1: 2620, y1: 792 });
    }

    #[test]
    fn find_panel_locates_the_same_panel_on_smaller_frames() {
        // The interface scales with frame height: the tooltip is 2/3 the
        // size at 1440p and half at 1080p, under the 4K size floor.
        let r = find_panel(&frame_with_rect(2560, 1440, 1332, 267, 1746, 528)).expect("1440p");
        assert_eq!(r, Rect { x0: 1332, y0: 267, x1: 1746, y1: 528 });
        let r = find_panel(&frame_with_rect(1920, 1080, 1000, 200, 1310, 396)).expect("1080p");
        assert_eq!(r, Rect { x0: 1000, y0: 200, x1: 1310, y1: 396 });
    }

    #[test]
    fn find_panel_rejects_a_too_small_bright_blob() {
        // 100x100 bright box on a 4K frame: below the size floor.
        let img = frame_with_rect(3840, 2160, 200, 100, 300, 200);
        assert!(find_panel(&img).is_none(), "small blob is not a panel");
        // A box that would be a tooltip at 4K is scenery-sized at 1080p.
        let img = frame_with_rect(1920, 1080, 200, 100, 820, 492);
        assert!(find_panel(&img).is_none(), "4K-sized box on a 1080p frame is not a tooltip");
    }

    #[test]
    fn find_panel_returns_none_on_a_dark_frame() {
        let img = GrayImage::new(1920, 1080);
        assert!(find_panel(&img).is_none(), "nothing bright to find");
    }
}
