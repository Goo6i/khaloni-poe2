use std::{collections::BTreeMap, process::Command};

use image::{imageops, GrayImage};

pub const UPSCALE: u32 = 3;
pub const ICON_CUT: f32 = 0.25;
const MIN_CONF: f32 = 40.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrLine {
    /// Normalized text from words with confidence >= 40 (fuzzy-match tier).
    pub filtered: String,
    /// Normalized text from all words (substring-match tier).
    pub unfiltered: String,
    /// Top of the line in preprocessed-image pixels.
    pub y_top: u32,
    pub height: u32,
}

/// The milestone-0 recipe: crop the icon column, upscale, min-max normalize.
/// Input must already be grayscale; caller converts from the capture format.
pub fn preprocess(region: &GrayImage) -> GrayImage {
    let cut = (region.width() as f32 * ICON_CUT) as u32;
    let text = imageops::crop_imm(region, cut, 0, region.width() - cut, region.height()).to_image();
    let up = imageops::resize(
        &text,
        text.width() * UPSCALE,
        text.height() * UPSCALE,
        imageops::FilterType::Lanczos3,
    );
    normalize(up)
}

fn normalize(mut img: GrayImage) -> GrayImage {
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

/// Runs the system tesseract on a preprocessed image, returns y-ordered lines.
pub fn run_tesseract(cmd: &str, pre: &GrayImage) -> anyhow::Result<Vec<OcrLine>> {
    let mut tmp = tempfile_path();
    pre.save(&tmp)?;
    let out = Command::new(cmd)
        .args([tmp.to_str().unwrap(), "-", "--psm", "6", "-l", "eng", "tsv"])
        .output()?;
    let _ = std::fs::remove_file(&mut tmp);
    if !out.status.success() {
        anyhow::bail!(
            "tesseract failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(parse_tsv(&String::from_utf8_lossy(&out.stdout)))
}

fn tempfile_path() -> std::path::PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("poe2-lens-ocr-{}-{n}.png", std::process::id()))
}

pub fn parse_tsv(tsv: &str) -> Vec<OcrLine> {
    // key: (block, par, line) -> (filtered words, all words, min top, max bottom)
    let mut acc: BTreeMap<(u32, u32, u32), (Vec<String>, Vec<String>, u32, u32)> = BTreeMap::new();
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
            let unfiltered = poe2_lens_core::matcher::normalize(&uw.join(" "));
            if unfiltered.is_empty() {
                return None;
            }
            Some(OcrLine {
                filtered: poe2_lens_core::matcher::normalize(&fw.join(" ")),
                unfiltered,
                y_top: top,
                height: bottom.saturating_sub(top),
            })
        })
        .collect();
    lines.sort_by_key(|l| l.y_top);
    lines
}
