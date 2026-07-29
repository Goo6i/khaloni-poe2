//! Rumour tooltip recognizer geometry: turning OCR TSV into line boxes,
//! locating the tooltip by its anchor phrases, and cropping the rumour
//! region. Pure (no image/tesseract deps) so it unit-tests without a real
//! OCR run; the CV panel-find and the tesseract passes live in the app.
//!
//! Mirrors the proven danielmtv2/poe2-expedition-overlay method verified in
//! the Python spike (8/10 recall, 0 false positives on 5 real 4K frames):
//! anchor-locate "UNCHARTED WATERS" (top) + "REQUIRES"/"CONSUMES" (bottom),
//! crop the region between them within the tooltip column, then multiscale
//! OCR + fuzzy-resolve each line (resolve lives in `rumour::RumourIndex`).

use crate::matcher::normalize;

/// Minimum tesseract word confidence to keep a word (Python spike: 30).
/// Junk low-confidence reads would otherwise poison the line's fuzzy match.
const MIN_CONF: f32 = 30.0;

/// One OCR text line with its bounding box in the OCR'd image's pixel
/// space. Unlike `ocr::OcrLine` (reward panel, y-only), rumours need x too:
/// anchors are located by column and rating badges hang off the right edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RumourLine {
    pub text: String,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl RumourLine {
    pub fn yc(&self) -> u32 {
        (self.y0 + self.y1) / 2
    }
    pub fn xc(&self) -> u32 {
        (self.x0 + self.x1) / 2
    }
}

/// Group tesseract TSV word rows (level 5) into text lines by their
/// (block, par, line) key, joining words in reading order and unioning
/// their word boxes. Standard tesseract TSV column layout: block=2, par=3,
/// line=4, left=6, top=7, width=8, height=9, conf=10, text=11.
pub fn parse_rumour_tsv(tsv: &str) -> Vec<RumourLine> {
    use std::collections::BTreeMap;
    // Per (block, par, line): (words, x0, y0, x1, y1).
    type Acc = (Vec<String>, u32, u32, u32, u32);
    let mut acc: BTreeMap<(u32, u32, u32), Acc> = BTreeMap::new();
    for row in tsv.lines().skip(1) {
        let f: Vec<&str> = row.split('\t').collect();
        if f.len() < 12 || f[0] != "5" {
            continue;
        }
        let (Ok(block), Ok(par), Ok(line)) = (f[2].parse(), f[3].parse(), f[4].parse()) else {
            continue;
        };
        let (Ok(left), Ok(top), Ok(width), Ok(height)) = (
            f[6].parse::<u32>(),
            f[7].parse::<u32>(),
            f[8].parse::<u32>(),
            f[9].parse::<u32>(),
        ) else {
            continue;
        };
        let conf: f32 = f[10].parse().unwrap_or(-1.0);
        let word = f[11].trim();
        if word.is_empty() || conf < MIN_CONF {
            continue;
        }
        let e = acc
            .entry((block, par, line))
            .or_insert_with(|| (Vec::new(), u32::MAX, u32::MAX, 0, 0));
        e.0.push(word.to_string());
        e.1 = e.1.min(left);
        e.2 = e.2.min(top);
        e.3 = e.3.max(left + width);
        e.4 = e.4.max(top + height);
    }
    acc.into_values()
        .filter(|(words, ..)| !words.is_empty())
        .map(|(words, x0, y0, x1, y1)| RumourLine {
            text: words.join(" "),
            x0,
            y0,
            x1,
            y1,
        })
        .collect()
}

/// Fuzzy score (0..1) that `line` contains `phrase`, using the same
/// normalization the rumour resolver uses. A best-window (partial) ratio
/// mirroring rapidfuzz's partial_ratio: the anchor may sit inside a longer
/// line (e.g. "REQUIRES 3 Logbooks"), so the phrase is slid across the
/// line and the best-matching window wins.
fn anchor_score(line: &str, phrase: &str) -> f64 {
    let l: Vec<char> = normalize(line).chars().collect();
    let p = normalize(phrase);
    let plen = p.chars().count();
    if plen == 0 {
        return 0.0;
    }
    if l.len() <= plen {
        return strsim::normalized_levenshtein(&l.iter().collect::<String>(), &p);
    }
    (0..=l.len() - plen)
        .map(|s| {
            let window: String = l[s..s + plen].iter().collect();
            strsim::normalized_levenshtein(&window, &p)
        })
        .fold(0.0, f64::max)
}

/// Anchor phrases that bracket the rumour list inside the tooltip.
pub const ANCHOR_TOP: &str = "UNCHARTED WATERS";
pub const ANCHOR_BOTTOM: [&str; 2] = ["CONSUMES", "REQUIRES"];
/// Minimum anchor similarity to accept a tooltip (0.60, mirroring the
/// Python spike's rapidfuzz ratio >= 60).
pub const ANCHOR_MIN: f64 = 0.60;

/// Locate the tooltip by its anchors: the best "UNCHARTED WATERS" line
/// (top), then the best "CONSUMES"/"REQUIRES" line strictly below it
/// (bottom). Returns indices into `lines`. `None` if no top anchor clears
/// `ANCHOR_MIN`; the bottom is optional (some tooltips truncate it).
pub fn locate_anchors(lines: &[RumourLine]) -> Option<(usize, Option<usize>)> {
    let (top, ts) = best_anchor(lines, ANCHOR_TOP, None)?;
    if ts < ANCHOR_MIN {
        return None;
    }
    let top_yc = lines[top].yc();
    let mut bottom: Option<(usize, f64)> = None;
    for phrase in ANCHOR_BOTTOM {
        if let Some((i, s)) = best_anchor(lines, phrase, Some(top_yc)) {
            if bottom.is_none_or(|(_, b)| s > b) {
                bottom = Some((i, s));
            }
        }
    }
    let bottom = bottom.filter(|&(_, s)| s >= ANCHOR_MIN).map(|(i, _)| i);
    Some((top, bottom))
}

/// Best-scoring line for `phrase`; if `after_yc` is set, only lines whose
/// vertical center is strictly below it are considered.
fn best_anchor(lines: &[RumourLine], phrase: &str, after_yc: Option<u32>) -> Option<(usize, f64)> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, ln)| after_yc.is_none_or(|y| ln.yc() > y))
        .map(|(i, ln)| (i, anchor_score(&ln.text, phrase)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

/// An axis-aligned box in image-pixel space (inclusive-left, exclusive-right).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Rect {
    pub fn width(&self) -> u32 {
        self.x1.saturating_sub(self.x0)
    }
    pub fn height(&self) -> u32 {
        self.y1.saturating_sub(self.y0)
    }
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
}

/// The minimum half-width of the crop column, in title-line pixels: the
/// rumour lines can run wider than the title, so the column never narrows
/// below this (mirrors the Python spike's 260px floor).
const COL_HALF_MIN: f64 = 260.0;
/// Fraction of the column half-width actually cropped, trimming the tooltip
/// border (Python spike: 0.95).
const COL_HALF_FRAC: f64 = 0.95;
/// When the bottom anchor is missing, crop this many title-line-heights
/// below the title to cover the rumour list (Python spike: 6).
const FALLBACK_LINES: u32 = 6;

/// The rumour-list crop between the anchors, within the tooltip column,
/// clamped to the frame. `y0` sits just under the title; `y1` is the bottom
/// anchor's baseline, or `FALLBACK_LINES` title-heights down when there is
/// no bottom anchor. The column is centered on the title and at least
/// `COL_HALF_MIN` wide so wide rumour names are not clipped.
pub fn crop_region(top: &RumourLine, bottom: Option<&RumourLine>, frame_w: u32, frame_h: u32) -> Rect {
    let line_h = (top.y1 - top.y0).max(1);
    let y0 = top.y1;
    let y1 = bottom.map_or(top.y1 + FALLBACK_LINES * line_h, |b| b.y1);
    let col_c = f64::from(top.x0 + top.x1) / 2.0;
    let col_half = f64::from((top.x1 - top.x0).max(COL_HALF_MIN as u32)) * COL_HALF_FRAC;
    let x0 = (col_c - col_half).max(0.0) as u32;
    let x1 = ((col_c + col_half) as u32).min(frame_w);
    Rect {
        x0: x0.min(frame_w),
        y0: y0.min(frame_h),
        x1,
        y1: y1.min(frame_h),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, y0: u32) -> RumourLine {
        RumourLine { text: text.to_string(), x0: 100, y0, x1: 300, y1: y0 + 20 }
    }

    #[test]
    fn locate_anchors_finds_top_and_bottom_below_it() {
        let lines = vec![
            line("some chrome", 10),
            line("UNCHARTED WATERS", 40),
            line("Fallen Stars", 70),
            line("REQUIRES 3 Logbooks", 120),
        ];
        let (top, bottom) = locate_anchors(&lines).expect("anchors present");
        assert_eq!(top, 1, "top anchor is the UNCHARTED WATERS line");
        assert_eq!(bottom, Some(3), "bottom anchor is REQUIRES below the top");
    }

    #[test]
    fn locate_anchors_tolerates_ocr_slips_in_the_title() {
        let lines = vec![line("UNCHARJED WATER5", 40), line("CONSUMES a Logbook", 90)];
        let (top, bottom) = locate_anchors(&lines).expect("fuzzy top anchor");
        assert_eq!(top, 0);
        assert_eq!(bottom, Some(1));
    }

    #[test]
    fn locate_anchors_returns_none_without_a_title() {
        let lines = vec![line("Fallen Stars", 40), line("REQUIRES 3 Logbooks", 90)];
        assert!(locate_anchors(&lines).is_none(), "no UNCHARTED WATERS = no tooltip");
    }

    #[test]
    fn locate_anchors_ignores_a_bottom_anchor_above_the_title() {
        // A stray "REQUIRES" above the title must not be chosen as bottom.
        let lines = vec![line("REQUIRES nothing", 10), line("UNCHARTED WATERS", 40)];
        let (top, bottom) = locate_anchors(&lines).expect("top present");
        assert_eq!(top, 1);
        assert_eq!(bottom, None, "no valid bottom below the title");
    }

    fn box_line(x0: u32, y0: u32, x1: u32, y1: u32) -> RumourLine {
        RumourLine { text: String::new(), x0, y0, x1, y1 }
    }

    #[test]
    fn crop_region_spans_between_anchors_within_the_column() {
        // Title box 100..300 x, 40..64 y (200 wide, 24 tall). Bottom at y1=200.
        let top = box_line(100, 40, 300, 64);
        let bottom = box_line(120, 176, 280, 200);
        let r = crop_region(&top, Some(&bottom), 1000, 800);
        // col_c=200, col_half=max(200,260)*0.95=247 -> x0=max(0,-47)=0, x1=447.
        // y0 just below title (64), y1 = bottom baseline (200).
        assert_eq!(r, Rect { x0: 0, y0: 64, x1: 447, y1: 200 });
    }

    #[test]
    fn crop_region_falls_back_below_the_title_without_a_bottom_anchor() {
        let top = box_line(100, 40, 300, 64);
        let r = crop_region(&top, None, 1000, 800);
        // y1 = title.y1 + 6*line_h = 64 + 6*24 = 208.
        assert_eq!(r.y0, 64);
        assert_eq!(r.y1, 208);
    }

    #[test]
    fn crop_region_clamps_to_the_frame() {
        let top = box_line(100, 40, 300, 64);
        let bottom = box_line(120, 176, 280, 200);
        // Frame narrower/shorter than the computed box: x1 and y1 clamp.
        let r = crop_region(&top, Some(&bottom), 300, 150);
        assert_eq!(r.x1, 300, "right edge clamped to frame width");
        assert_eq!(r.y1, 150, "bottom clamped to frame height");
        assert_eq!(r.x0, 0);
    }

    #[test]
    fn parse_rumour_tsv_groups_words_into_line_boxes() {
        // Header row (skipped) + two words on line 1, one word on line 2.
        let tsv = "level\tpage\tblock\tpar\tline\tword\tleft\ttop\twidth\theight\tconf\ttext\n\
            5\t1\t1\t1\t1\t1\t100\t50\t80\t20\t95\tUNCHARTED\n\
            5\t1\t1\t1\t1\t2\t190\t52\t70\t18\t93\tWATERS\n\
            5\t1\t1\t1\t2\t1\t100\t90\t60\t20\t90\tFallen\n";
        let lines = parse_rumour_tsv(tsv);
        assert_eq!(lines.len(), 2, "two logical lines");
        assert_eq!(lines[0].text, "UNCHARTED WATERS", "words joined in order");
        // Union of the two word boxes.
        assert_eq!(
            (lines[0].x0, lines[0].y0, lines[0].x1, lines[0].y1),
            (100, 50, 260, 70)
        );
        assert_eq!(lines[1].text, "Fallen");
    }

    #[test]
    fn parse_rumour_tsv_drops_low_confidence_words() {
        // Two words on one line: one confident, one OCR junk (conf 10).
        let tsv = "l\tp\tb\ta\tn\tw\tleft\ttop\twidth\theight\tconf\ttext\n\
            5\t1\t1\t1\t1\t1\t100\t50\t80\t20\t92\tEndless\n\
            5\t1\t1\t1\t1\t2\t190\t52\t70\t18\t10\t~~\n\
            5\t1\t1\t1\t1\t3\t270\t51\t60\t19\t88\tCliffs\n";
        let lines = parse_rumour_tsv(tsv);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Endless Cliffs", "conf<30 junk word dropped");
    }
}
