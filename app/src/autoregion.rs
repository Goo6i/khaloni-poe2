//! Zero-calibration reward-panel detection (the Detector half of
//! docs/notes/specs/2026-07-31-auto-region-design.md): find the reward
//! panel's capture-space region on a full frame with no user calibration.
//!
//! Two stages, both pure image math (no OCR, so this compiles and runs on
//! every target, ocr cfg or not, and is cheap enough for the 700 ms
//! full-frame cadence inside the rumour worker):
//!
//! 1. Blob sweep: `rumours::panel_candidates` — the same step-4
//!    subsampled threshold/close/label pass the rumour recognizer's
//!    tooltip finder uses — yields every bright parchment-like blob.
//!    This module applies its own size gates because the reward panel
//!    (~990x1030 at 4K, measured on tests/fixtures/panel_choice.png) is
//!    larger than the rumour tooltip (~620x390) that `find_panel`'s
//!    MAX_W/MAX_H were tuned for.
//!
//! 2. Band validation: `ocr::row_profile` + `detect_bands_from_profile`
//!    over each candidate's crop. A reward panel shows discrete bright
//!    reward-row bars; a candidate is accepted iff at least
//!    MIN_REWARD_BANDS bands (each >= ocr::BAND_MIN_H tall, which
//!    detect_bands already enforces) qualify as reward-style.
//!
//! Discriminating feature (measured, not assumed): the expedition rumour
//! tooltip is the same bright parchment and passes the blob sweep AND
//! plain band detection — all 5 real rumour fixtures produce 3-4 bands at
//! BAND_BRIGHTNESS=175. What separates the two panel styles is the bands'
//! own brightness: rumour-frame band means measure 178..=203 (bright-ish
//! text rows on already-bright parchment), while the reward panel's white
//! reward bars measure 221..=227 (on the panel_choice composite). A band
//! only counts as reward-style when its mean clears
//! REWARD_BAND_MEAN = 205 — the midpoint of the measured live gap
//! (rumour bands top out at 200; live reward bars measure 210-216,
//! dimmer than the 221-227 the original fixture suggested),
//! with real margin to both sides. That single feature rejects all 5
//! rumour fixtures (0 qualifying bands each) while the reward fixture
//! keeps all 4 of its bands; no band-coverage cap was needed on top (one
//! was considered, but tall reward panels — unfixtured as of this design
//! — may legitimately have high band coverage, so a cap would risk
//! rejecting real panels for no measured gain).

use image::{imageops, GrayImage};
use khaloni_poe2_core::rumour_scan::Rect;

use crate::ocr;
use crate::rumours;

/// Minimum candidate size (full-res px). Loose on purpose: the reward
/// panel is ~990x1030 at 4K but proportionally smaller on window-sized
/// Windows captures (~500x515 at 1080p); band validation, not size, is
/// the discriminator. The floor only skips blobs too small to hold two
/// BAND_MIN_H bands plus gaps with room to spare.
const MIN_W: u32 = 260;
const MIN_H: u32 = 260;
/// A candidate wider/taller than this fraction (in tenths) of the frame
/// is scenery, not a panel: full-bright frames (loading screens, menus)
/// label as one giant blob and must not become a "region".
const MAX_FRAME_TENTHS: u32 = 9;
/// Blob solidity floor. The reward fixture's blob fills 0.66 of its box
/// (the mid-gray map dips under the sweep threshold in places); sprawling
/// UI noise measures 0.13-0.46 on the rumour fixtures. 0.3 keeps real
/// margin below the fixture while skipping the cheapest junk before the
/// per-candidate crop+profile work.
const MIN_FILL: f64 = 0.3;
/// See the module doc: a band's profile mean must clear this to count as
/// a reward-style bar (midpoint of the measured rumour-max 200 /
/// reward-min 221 gap). Local to this module by design — ocr.rs's
/// BAND_BRIGHTNESS=175 stays the OCR pipeline's own row gate.
const REWARD_BAND_MEAN: u16 = 205;
/// A reward panel always shows multiple reward rows; one lone bright bar
/// (a hover tooltip's title, a stray HUD element) is not a panel.
const MIN_REWARD_BANDS: usize = 2;

/// Padding applied to the accepted blob box, in the spirit of rumours'
/// CROP_PADs. Measured on the panel_choice composite: the blob's bottom
/// edge clips 46 px of the panel (the map background fades under the
/// sweep threshold there), so PAD_Y_DN recovers it; the other sides only
/// need slack for the step-4 subsample grid snap.
const PAD_X: u32 = 8;
const PAD_Y_UP: u32 = 16;
const PAD_Y_DN: u32 = 48;

/// Capture-space reward-panel region, or None when no panel is on screen.
///
/// Of all candidates that pass the gates and band validation, the one
/// with the largest area wins (mirrors find_panel's "heaviest blob"
/// tie-break; on any real frame at most one reward panel exists, so this
/// only matters for freak double-validations).
pub fn detect_reward_region(gray: &GrayImage) -> Option<Rect> {
    let dbg = std::env::var("KHALONI_DEBUG").is_ok();
    let (gw, gh) = (gray.width(), gray.height());
    let mut best: Option<(u64, Rect)> = None; // (area, padded box)
    for cand in rumours::panel_candidates(gray) {
        let (w, h) = (cand.rect.width(), cand.rect.height());
        if w < MIN_W || h < MIN_H || cand.fill < MIN_FILL {
            continue;
        }
        if w * 10 > gw * MAX_FRAME_TENTHS || h * 10 > gh * MAX_FRAME_TENTHS {
            if dbg {
                eprintln!("autoregion: candidate {:?} rejected: frame-sized", cand.rect);
            }
            continue;
        }
        // The blob box is subsample-grid aligned and can overshoot the
        // frame edge by up to PANEL_STEP - 1: clamp before cropping.
        let x1 = cand.rect.x1.min(gw);
        let y1 = cand.rect.y1.min(gh);
        if x1 <= cand.rect.x0 || y1 <= cand.rect.y0 {
            continue;
        }
        let crop =
            imageops::crop_imm(gray, cand.rect.x0, cand.rect.y0, x1 - cand.rect.x0, y1 - cand.rect.y0)
                .to_image();
        let profile = ocr::row_profile(&crop);
        let bands = ocr::detect_bands_from_profile(&profile);
        let reward_bands =
            bands.iter().filter(|&&(y0, y1)| band_mean(&profile, y0, y1) >= REWARD_BAND_MEAN).count();
        if dbg {
            let means: Vec<u16> = bands.iter().map(|&(y0, y1)| band_mean(&profile, y0, y1)).collect();
            eprintln!(
                "autoregion: candidate {:?} fill={:.2} bands={} band-means={means:?} reward-style={} -> {}",
                cand.rect,
                cand.fill,
                bands.len(),
                reward_bands,
                if reward_bands >= MIN_REWARD_BANDS { "ACCEPT" } else { "reject" },
            );
        }
        if reward_bands < MIN_REWARD_BANDS {
            continue;
        }
        let area = u64::from(w) * u64::from(h);
        if best.is_none_or(|(a, _)| area > a) {
            best = Some((
                area,
                Rect {
                    x0: cand.rect.x0.saturating_sub(PAD_X),
                    y0: cand.rect.y0.saturating_sub(PAD_Y_UP),
                    x1: (x1 + PAD_X).min(gw),
                    y1: (y1 + PAD_Y_DN).min(gh),
                },
            ));
        }
    }
    best.map(|(_, r)| r)
}

/// Integer mean of `profile[y0..y1)`, matching row_profile's own integer
/// row means. Caller guarantees a non-empty in-range band (detect_bands
/// only emits bands >= BAND_MIN_H within the profile).
fn band_mean(profile: &[u16], y0: u32, y1: u32) -> u16 {
    let band = &profile[y0 as usize..y1 as usize];
    (band.iter().map(|&v| u64::from(v)).sum::<u64>() / band.len() as u64) as u16
}
