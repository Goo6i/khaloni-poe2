use poe2_lens::ocr::{detect_bands, parse_band_tsv, UPSCALE};

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
    // dim, dim, [bright x14], dim, dim, [bright x3 - too short], dim
    let mut rows = vec![50u8, 50];
    rows.extend(std::iter::repeat_n(200u8, 14));
    rows.extend([50, 50]);
    rows.extend([200, 200, 200]);
    rows.push(50);

    let bands = detect_bands(&synthetic_region(&rows));
    assert_eq!(bands, vec![(2, 16)], "only the 14px run clears BAND_MIN_H; the 3px run must not");
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
