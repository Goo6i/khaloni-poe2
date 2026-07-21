use poe2_lens::pricing::Tier;
use poe2_lens::render::{Placed, Renderer};

#[test]
fn draws_nonempty_label_pixels_inside_bounds() {
    let font = std::fs::read("/usr/share/fonts/TTF/DejaVuSans.ttf").unwrap();
    let r = Renderer::new(&font).unwrap();
    let mut pm = tiny_skia::Pixmap::new(600, 200).unwrap();
    r.draw_frame(
        &mut pm,
        &[Placed { x: 20, y: 100, label: "12.5 ex (2.5 each)".into(), tier: Tier::Decent }],
        "Total: 2.1 div",
        false,
    );
    let data = pm.data();
    let painted = data.chunks_exact(4).filter(|p| p[3] != 0).count();
    assert!(painted > 500, "expected painted pixels, got {painted}");
    // Nothing outside a sane bound of the label row should be painted below it.
    let mut low_rows_painted = 0;
    for yy in 160..200 {
        for xx in 0..600 {
            if pm.pixel(xx, yy).map(|p| p.alpha()).unwrap_or(0) != 0 {
                low_rows_painted += 1;
            }
        }
    }
    assert_eq!(low_rows_painted, 0, "label bled far below its row");
}

#[test]
fn repeated_draw_frame_is_deterministic_with_glyph_cache() {
    // A second draw_frame call reuses the renderer's internal glyph cache
    // instead of re-rasterizing; the output must be pixel-identical to the
    // first call, proving the cached path draws the same as the cold path.
    let font = std::fs::read("/usr/share/fonts/TTF/DejaVuSans.ttf").unwrap();
    let r = Renderer::new(&font).unwrap();
    let labels = [
        Placed { x: 20, y: 100, label: "12.5 ex (2.5 each)".into(), tier: Tier::Decent },
        Placed { x: 20, y: 140, label: "3 ex".into(), tier: Tier::Jackpot },
    ];

    let mut first = tiny_skia::Pixmap::new(600, 200).unwrap();
    r.draw_frame(&mut first, &labels, "Total: 2.1 div", false);

    let mut second = tiny_skia::Pixmap::new(600, 200).unwrap();
    r.draw_frame(&mut second, &labels, "Total: 2.1 div", false);

    assert_eq!(first.data(), second.data(), "second draw_frame with warm glyph cache must match the first");
}
