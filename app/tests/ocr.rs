use poe2_lens::ocr::{parse_tsv, preprocess, ICON_CUT, UPSCALE};

#[test]
fn parses_real_tsv_into_lines_with_coordinates() {
    let tsv = include_str!("fixtures/sample.tsv");
    let lines = parse_tsv(tsv);
    // Three reward rows plus possible noise lines; the three Greater orbs must be present.
    let all: Vec<&str> = lines.iter().map(|l| l.unfiltered.as_str()).collect();
    for want in ["greater chaos orb", "greater exalted orb", "greater regal orb"] {
        assert!(
            all.iter().any(|l| l.contains(want)),
            "missing {want:?} in {all:?}"
        );
    }
    // Rows must be in top-to-bottom order with sane geometry.
    let ys: Vec<u32> = lines.iter().map(|l| l.y_top).collect();
    let mut sorted = ys.clone();
    sorted.sort_unstable();
    assert_eq!(ys, sorted, "lines must be y-ordered");
    assert!(lines.iter().all(|l| l.height > 0));
}

#[test]
fn filtered_tier_drops_low_confidence_words() {
    // Synthetic 2-word line: one word conf 90, one conf 10.
    let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
               5\t1\t1\t1\t1\t1\t0\t10\t50\t20\t90.0\tExalted\n\
               5\t1\t1\t1\t1\t2\t60\t10\t50\t20\t10.0\tOrbb\n";
    let lines = parse_tsv(tsv);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].filtered, "exalted");
    assert_eq!(lines[0].unfiltered, "exalted orbb");
    assert_eq!(lines[0].y_top, 10);
    assert_eq!(lines[0].height, 20);
}

#[test]
fn rejects_lines_without_a_four_letter_alpha_run() {
    // Line 1: pure 1-3 char OCR noise fragments (icon artifacts, digits).
    // Line 2: a real word ("Orb", 3 letters) plus a short fragment - still
    // under MIN_WORD_RUN=4 everywhere in the line.
    // Line 3: a real 4+ letter word, must survive.
    let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
               5\t1\t1\t1\t1\t1\t0\t10\t20\t20\t90.0\tl\n\
               5\t1\t1\t1\t1\t2\t30\t10\t20\t20\t90.0\t88\n\
               5\t1\t1\t2\t2\t1\t0\t50\t20\t20\t90.0\tOrb\n\
               5\t1\t1\t2\t2\t2\t30\t50\t20\t20\t90.0\tf\n\
               5\t1\t1\t3\t3\t1\t0\t90\t60\t20\t90.0\tExalted\n";
    let lines = parse_tsv(tsv);
    assert_eq!(lines.len(), 1, "only the line with a real 4+ letter run must survive: {lines:?}");
    assert_eq!(lines[0].unfiltered, "exalted");
}

#[test]
fn rejects_empty_and_whitespace_only_lines() {
    let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
               5\t1\t1\t1\t1\t1\t0\t10\t20\t20\t90.0\t...\n\
               5\t1\t1\t1\t1\t2\t30\t10\t20\t20\t90.0\t!!!\n";
    let lines = parse_tsv(tsv);
    assert!(lines.is_empty(), "punctuation-only lines normalize to empty and must be dropped");
}

#[test]
fn preprocess_shapes_the_image_as_specified() {
    let img = image::GrayImage::from_pixel(400, 100, image::Luma([120u8]));
    let out = preprocess(&img);
    assert_eq!(out.width(), (400.0 * (1.0 - ICON_CUT)) as u32 * UPSCALE);
    assert_eq!(out.height(), 100 * UPSCALE);
}
