use poe2_lens::ocr::{detect_bands, parse_band_tsv, parse_whole_tsv, union_ocr_lines, OcrLine, UPSCALE};

#[test]
fn parse_band_tsv_splits_confidence_tiers_and_sets_coordinates_from_band_bounds() {
    // Synthetic 2-word line: one word conf 90, one conf 10.
    let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
               5\t1\t1\t1\t1\t1\t0\t10\t50\t20\t90.0\tExalted\n\
               5\t1\t1\t1\t1\t2\t60\t10\t50\t20\t10.0\tOrbb\n";
    // Band bounds are in capture-pixel space; y_top/height must come from
    // these (* UPSCALE), not from anything in the TSV's own top/height
    // columns (which are the crop's local, already-upscaled coordinates).
    let line = parse_band_tsv(tsv, 100, 130).expect("real words must produce a line");
    assert_eq!(line.filtered, "exalted");
    assert_eq!(line.unfiltered, "exalted orbb");
    assert_eq!(line.y_top, 100 * UPSCALE);
    assert_eq!(line.height, (130 - 100) * UPSCALE);
}

#[test]
fn parse_band_tsv_rejects_bands_without_a_four_letter_alpha_run() {
    // 1-3 char OCR noise fragments (icon artifacts, digits): no run of 4.
    let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
               5\t1\t1\t1\t1\t1\t0\t10\t20\t20\t90.0\tl\n\
               5\t1\t1\t1\t1\t2\t30\t10\t20\t20\t90.0\t88\n\
               5\t1\t1\t1\t1\t3\t60\t10\t20\t20\t90.0\tf\n";
    assert!(parse_band_tsv(tsv, 0, 30).is_none());
}

#[test]
fn parse_band_tsv_rejects_empty_and_whitespace_only_output() {
    let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
               5\t1\t1\t1\t1\t1\t0\t10\t20\t20\t90.0\t...\n\
               5\t1\t1\t1\t1\t2\t30\t10\t20\t20\t90.0\t!!!\n";
    assert!(
        parse_band_tsv(tsv, 0, 30).is_none(),
        "punctuation-only output normalizes to empty and must be dropped"
    );
    assert!(parse_band_tsv("level\tpage_num\n", 0, 30).is_none(), "no word rows at all");
}

fn synthetic_region(rows: &[u8]) -> image::GrayImage {
    // A 20px-wide region; every pixel in a row gets the same brightness,
    // so the per-row mean over the sampled x-range is just that value.
    let h = rows.len() as u32;
    let mut img = image::GrayImage::new(20, h);
    for (y, &v) in rows.iter().enumerate() {
        for x in 0..20 {
            img.put_pixel(x, y as u32, image::Luma([v]));
        }
    }
    img
}

#[test]
fn detect_bands_finds_contiguous_bright_runs_at_or_above_min_height() {
    // dim, dim, [bright x14], dim x(BAND_MERGE_GAP+1), [bright x3 - too
    // short], dim. The dim gap must clear BAND_MERGE_GAP so the two runs
    // are scored independently rather than gap-merged into one (see
    // `detect_bands_merges_runs_separated_by_a_small_gap` below for that
    // behavior).
    let mut rows = vec![50u8, 50];
    rows.extend(std::iter::repeat_n(200u8, 14));
    rows.extend(std::iter::repeat_n(50u8, 14));
    rows.extend([200, 200, 200]);
    rows.push(50);

    let bands = detect_bands(&synthetic_region(&rows));
    assert_eq!(bands, vec![(2, 16)], "only the 14px run clears BAND_MIN_H; the 3px run must not");
}

#[test]
fn detect_bands_merges_runs_separated_by_a_small_gap() {
    // dim, dim, [bright x14], dim x3 (well under BAND_MERGE_GAP=13),
    // [bright x14]: two real bright runs close enough together (as
    // measured on tall skill entries: icon/text/descender strips) must
    // come back as ONE merged band spanning both, not two.
    let mut rows = vec![50u8, 50];
    rows.extend(std::iter::repeat_n(200u8, 14));
    rows.extend([50, 50, 50]);
    rows.extend(std::iter::repeat_n(200u8, 14));
    rows.push(50);

    let bands = detect_bands(&synthetic_region(&rows));
    assert_eq!(bands, vec![(2, 33)], "runs separated by <= BAND_MERGE_GAP must merge into one band");
}

#[test]
fn detect_bands_returns_nothing_for_a_uniformly_dim_region() {
    let rows = vec![100u8; 40];
    assert!(detect_bands(&synthetic_region(&rows)).is_empty());
}

#[test]
fn detect_bands_includes_a_band_that_runs_to_the_bottom_edge() {
    let mut rows = vec![50u8, 50];
    rows.extend(std::iter::repeat_n(200u8, 20));
    // No trailing dim row: the bright run ends exactly at the image edge.
    let bands = detect_bands(&synthetic_region(&rows));
    assert_eq!(bands, vec![(2, 22)]);
}

#[test]
fn parse_whole_tsv_groups_words_into_rows_by_block_par_line() {
    // Two words in (block1,par1,line1), two more in (block1,par2,line1):
    // must come back as 2 rows, each spanning its own words' min-top to
    // max(top+height).
    let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
               5\t1\t1\t1\t1\t1\t0\t10\t50\t20\t90.0\tExalted\n\
               5\t1\t1\t1\t1\t2\t60\t10\t50\t20\t10.0\tOrbb\n\
               5\t1\t1\t2\t1\t1\t0\t100\t50\t20\t90.0\tChaos\n\
               5\t1\t1\t2\t1\t2\t60\t100\t50\t20\t90.0\tOrb\n";
    let lines = parse_whole_tsv(tsv);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0].filtered, "exalted");
    assert_eq!(lines[0].unfiltered, "exalted orbb");
    assert_eq!(lines[0].y_top, 10);
    assert_eq!(lines[0].height, 20);
    assert_eq!(lines[1].filtered, "chaos orb");
    assert_eq!(lines[1].unfiltered, "chaos orb");
    assert_eq!(lines[1].y_top, 100);
}

#[test]
fn parse_whole_tsv_rejects_rows_without_a_four_letter_alpha_run() {
    let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
               5\t1\t1\t1\t1\t1\t0\t10\t20\t20\t90.0\tl\n\
               5\t1\t1\t1\t1\t2\t30\t10\t20\t20\t90.0\t88\n";
    assert!(parse_whole_tsv(tsv).is_empty());
    assert!(parse_whole_tsv("level\tpage_num\n").is_empty(), "no word rows at all");
}

fn line(filtered: &str, unfiltered: &str, y_top: u32, height: u32) -> OcrLine {
    OcrLine {
        filtered: filtered.to_string(),
        unfiltered: unfiltered.to_string(),
        y_top,
        height,
    }
}

#[test]
fn union_merges_overlapping_lines_and_keeps_the_longer_filtered_text() {
    // Overlap: band 100..150, whole 110..150 -> overlap 40, min height 40:
    // 40*2 > 40, so these are the same logical row. Whole's filtered text
    // has more real content (6 alpha chars vs 3), so it wins.
    let band = vec![line("abc", "abc", 100, 50)];
    let whole = vec![line("abcdef", "abcdef", 110, 40)];
    let merged = union_ocr_lines(band, whole);
    assert_eq!(merged.len(), 1, "{merged:?}");
    assert_eq!(merged[0].filtered, "abcdef");
    assert_eq!(merged[0].y_top, 110, "the winning candidate's own y_top must be kept");
}

#[test]
fn union_ties_keep_the_band_line() {
    let band = vec![line("abcd", "abcd", 100, 50)];
    let whole = vec![line("wxyz", "wxyz", 110, 40)];
    let merged = union_ocr_lines(band, whole);
    assert_eq!(merged.len(), 1, "{merged:?}");
    assert_eq!(merged[0].filtered, "abcd", "equal alpha-char counts must keep the band line");
    assert_eq!(merged[0].y_top, 100);
}

#[test]
fn union_keeps_non_overlapping_lines_from_both_sources_sorted_by_y() {
    let band = vec![line("band row", "band row", 500, 20)];
    let whole = vec![line("whole row", "whole row", 100, 20)];
    let merged = union_ocr_lines(band, whole);
    assert_eq!(merged.len(), 2, "{merged:?}");
    assert_eq!(merged[0].y_top, 100, "sorted by y_top ascending");
    assert_eq!(merged[0].filtered, "whole row");
    assert_eq!(merged[1].y_top, 500);
    assert_eq!(merged[1].filtered, "band row");
}

#[test]
fn union_consumes_every_overlapping_whole_line_not_just_the_biggest_overlap() {
    // Regression test for a real bug measured on spikes/ocr/samples/s5.png:
    // one oversized/garbled band (see the doc comment on union_ocr_lines)
    // overlapped BOTH a real whole-panel line and an adjacent noise-only
    // whole-panel line enough to pass the overlap test on both. An
    // earlier version of this function picked only the single
    // largest-raw-overlap partner (here, the noise line, since it simply
    // has more pixels of overlap with the oversized band) and left the
    // real line "unmatched" - which then leaked through as a spurious
    // second row a few pixels below the correct one. This must produce
    // exactly ONE merged row, not two, with the real text winning.
    let band = vec![line(
        "s s 8 bl vll llbv",
        "s s 8 bl vll llbv",
        1000,
        400,
    )];
    // Noise-only whole line, closer/larger overlap with the band.
    let whole_noise = line("o r i r u", "o r i r u", 1050, 200);
    // Real whole line, smaller overlap with the band, but the actual text.
    let whole_real = line(
        "skill level 20 animus exchange",
        "skill level 20 animus exchange",
        1300,
        90,
    );
    let merged = union_ocr_lines(band, vec![whole_noise, whole_real]);
    assert_eq!(
        merged.len(),
        1,
        "both overlapping whole-panel lines must be consumed into one row, not leak a duplicate: {merged:?}"
    );
    assert_eq!(merged[0].filtered, "skill level 20 animus exchange");
    assert_eq!(merged[0].y_top, 1300, "the winning candidate's own position, not the band's");
}

// --- optical scroll estimation ---

fn synthetic_profile(len: usize, bars: &[(usize, usize)]) -> Vec<u16> {
    let mut p = vec![130u16; len];
    for &(a, b) in bars {
        for v in p.iter_mut().take(b.min(len)).skip(a) {
            *v = 200;
        }
    }
    p
}

fn shifted(profile: &[u16], dy: i32) -> Vec<u16> {
    let n = profile.len() as i32;
    (0..n)
        .map(|i| {
            let src = i - dy;
            if src >= 0 && src < n {
                profile[src as usize]
            } else {
                130
            }
        })
        .collect()
}

#[test]
fn motion_tracking_recovers_known_shifts() {
    let base = synthetic_profile(1000, &[(100, 170), (250, 320), (400, 470), (700, 770)]);
    for dy in [-180i32, -60, -7, 7, 60, 180] {
        let cur = shifted(&base, dy);
        assert_eq!(
            poe2_lens::ocr::track_motion(&base, &cur),
            poe2_lens::ocr::Motion::Scrolled(dy),
            "shift {dy} must be recovered exactly"
        );
    }
}

#[test]
fn flat_and_static_frames_are_still() {
    let flat = vec![150u16; 1000];
    assert_eq!(
        poe2_lens::ocr::track_motion(&flat, &flat),
        poe2_lens::ocr::Motion::Still,
        "flat profiles carry no signal"
    );
    let base = synthetic_profile(1000, &[(100, 170), (400, 470)]);
    assert_eq!(
        poe2_lens::ocr::track_motion(&base, &base),
        poe2_lens::ocr::Motion::Still,
        "identical frames are not a scroll"
    );
}

#[test]
fn a_tiny_shift_is_still_not_a_scroll() {
    // Sub-3-row drift is jitter; POSITION_SNAP absorbs it downstream.
    let base = synthetic_profile(1000, &[(100, 170), (400, 470)]);
    assert_eq!(
        poe2_lens::ocr::track_motion(&base, &shifted(&base, 2)),
        poe2_lens::ocr::Motion::Still
    );
}

#[test]
fn uncorrelated_content_is_lost() {
    let a = synthetic_profile(1000, &[(100, 170), (400, 470)]);
    let b = synthetic_profile(1000, &[(37, 61), (533, 601), (804, 851)]);
    assert_eq!(
        poe2_lens::ocr::track_motion(&a, &b),
        poe2_lens::ocr::Motion::Lost,
        "a panel change is not a scroll and must not hold old positions"
    );
}

#[test]
fn mismatched_profile_lengths_are_lost() {
    let a = synthetic_profile(1000, &[(100, 170), (400, 470)]);
    let b = synthetic_profile(999, &[(100, 170), (400, 470)]);
    assert_eq!(poe2_lens::ocr::track_motion(&a, &b), poe2_lens::ocr::Motion::Lost);
}
