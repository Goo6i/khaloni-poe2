use poe2_lens::hover::{Popup, PopupLine};
use poe2_lens::pricing::{Denom, Tier};
use poe2_lens::render::{Placed, Renderer};

#[test]
fn draws_nonempty_label_pixels_inside_bounds() {
    let r = Renderer::new().unwrap();
    let mut pm = tiny_skia::Pixmap::new(600, 200).unwrap();
    r.draw_frame(
        &mut pm,
        &[Placed { x: 20, y: 100, amount: "12.5".into(), denom: Denom::Exalted, tier: Tier::Decent, best: false }],
        "",
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
    let r = Renderer::new().unwrap();
    let labels = [
        Placed { x: 20, y: 100, amount: "12.5".into(), denom: Denom::Exalted, tier: Tier::Decent, best: false },
        Placed { x: 20, y: 140, amount: "3".into(), denom: Denom::Chaos, tier: Tier::Jackpot, best: false },
    ];

    let mut first = tiny_skia::Pixmap::new(600, 200).unwrap();
    r.draw_frame(&mut first, &labels, "", false);

    let mut second = tiny_skia::Pixmap::new(600, 200).unwrap();
    r.draw_frame(&mut second, &labels, "", false);

    assert_eq!(first.data(), second.data(), "second draw_frame with warm glyph cache must match the first");
}

#[test]
fn jackpot_divine_row_composites_icon_pixels_beyond_the_text() {
    // A short amount ("9") leaves the text glyphs confined to a narrow
    // column near x=20; the divine icon sits a few px to the right of that
    // and spans ~24x24. A flat pill fill only ever contributes one or two
    // colors (fill + a few rounded-corner antialiasing shades) in any
    // window; real icon art is detailed enough that composited icon pixels
    // produce many distinct colors, so counting distinct colors in a
    // generous icon-sized window distinguishes "icon actually composited"
    // from "just more parchment".
    let r = Renderer::new().unwrap();
    let mut pm = tiny_skia::Pixmap::new(300, 200).unwrap();
    r.draw_frame(
        &mut pm,
        &[Placed { x: 20, y: 100, amount: "9".into(), denom: Denom::Divine, tier: Tier::Jackpot, best: false }],
        "",
        false,
    );

    let mut colors = std::collections::HashSet::new();
    for xx in 30..85u32 {
        for yy in 78..122u32 {
            if let Some(p) = pm.pixel(xx, yy) {
                if p.alpha() != 0 {
                    colors.insert((p.red(), p.green(), p.blue(), p.alpha()));
                }
            }
        }
    }
    assert!(
        colors.len() >= 8,
        "expected the divine icon's detailed artwork to produce many distinct colors beyond the amount text, got {}",
        colors.len()
    );
}

#[test]
fn draw_popup_paints_nonzero_pixels_at_the_anchor() {
    let r = Renderer::new().unwrap();
    let mut pm = tiny_skia::Pixmap::new(500, 300).unwrap();
    let popup = Popup {
        title: "Exalted Orb".into(),
        lines: vec![PopupLine { text: "12 ex".into(), denom: Denom::Exalted }],
        expires: std::time::Instant::now() + std::time::Duration::from_secs(6),
    };
    r.draw_popup(&mut pm, &popup, (20, 20));

    let mut painted = 0;
    for yy in 20..120u32 {
        for xx in 20..340u32 {
            if pm.pixel(xx, yy).map(|p| p.alpha()).unwrap_or(0) != 0 {
                painted += 1;
            }
        }
    }
    assert!(painted > 500, "expected painted popup pixels near the anchor, got {painted}");

    // Nothing painted well above/left of the anchor: the popup's top-left
    // corner is the anchor itself, not its center. A few rows of slack
    // account for the pill border stroke straddling the path (half its
    // 1.5px width sits outside the nominal rect).
    let mut outside_painted = 0;
    for yy in 0..17u32 {
        for xx in 0..500u32 {
            if pm.pixel(xx, yy).map(|p| p.alpha()).unwrap_or(0) != 0 {
                outside_painted += 1;
            }
        }
    }
    assert_eq!(outside_painted, 0, "popup bled well above its anchor");
}
