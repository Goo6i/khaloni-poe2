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
fn preprocess_shapes_the_image_as_specified() {
    let img = image::GrayImage::from_pixel(400, 100, image::Luma([120u8]));
    let out = preprocess(&img);
    assert_eq!(out.width(), (400.0 * (1.0 - ICON_CUT)) as u32 * UPSCALE);
    assert_eq!(out.height(), 100 * UPSCALE);
}
