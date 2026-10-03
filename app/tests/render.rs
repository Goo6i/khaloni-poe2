use khaloni_poe2::hover::{Popup, PopupLine};
use khaloni_poe2::pricing::{Denom, Tier};
use khaloni_poe2::render::{Placed, Renderer};

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
    let painted = data.as_chunks::<4>().0.iter().filter(|p| p[3] != 0).count();
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

/// Alpha of every pixel, for comparing renders.
fn alpha_plane(pm: &tiny_skia::Pixmap) -> Vec<u8> {
    pm.data().as_chunks::<4>().0.iter().map(|p| p[3]).collect()
}

fn sample_popup() -> Popup {
    Popup {
        title: "Exalted Orb".into(),
        lines: vec![PopupLine { text: "12 ex".into(), denom: Denom::Exalted }],
        expires: std::time::Instant::now() + std::time::Duration::from_secs(6),
    }
}

#[test]
fn scale_one_is_the_default_and_survives_a_round_trip() {
    // A fresh renderer draws at 1.0; one that was scaled and set back draws
    // the same pixels (no glyph or icon of the other scale left in a cache).
    let fresh = Renderer::new().unwrap();
    let mut a = tiny_skia::Pixmap::new(500, 200).unwrap();
    fresh.draw_popup(&mut a, &sample_popup(), (20, 20));

    let back = Renderer::new().unwrap();
    assert!(back.set_scale(1.5));
    assert!(back.set_scale(1.0));
    assert!(!back.set_scale(1.0), "an unchanged scale asks for no repaint");
    let mut b = tiny_skia::Pixmap::new(500, 200).unwrap();
    back.draw_popup(&mut b, &sample_popup(), (20, 20));
    assert_eq!(a.data(), b.data());
}

#[test]
fn a_fractional_scale_fills_the_device_pixmap_with_sharp_text() {
    let r = Renderer::new().unwrap();
    let popup = sample_popup();
    // Logical size is what layout and hit-testing use; it does not change
    // with the scale.
    let logical = r.popup_size(&popup);
    let mut one = tiny_skia::Pixmap::new(500, 200).unwrap();
    r.draw_popup(&mut one, &popup, (20, 20));

    assert!(r.set_scale(1.5));
    assert_eq!(r.popup_size(&popup), logical, "layout stays in logical pixels");
    // The caller hands in a device-size pixmap: 500x200 logical at 150%.
    let mut dev = tiny_skia::Pixmap::new(750, 300).unwrap();
    r.draw_popup(&mut dev, &popup, (20, 20));

    // Geometry: the popup starts at the scaled anchor and covers the
    // scaled rect, not the logical one.
    let painted_bounds = |pm: &tiny_skia::Pixmap| {
        let (mut x1, mut y1) = (0u32, 0u32);
        for y in 0..pm.height() {
            for x in 0..pm.width() {
                if pm.pixel(x, y).is_some_and(|p| p.alpha() != 0) {
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x1, y1)
    };
    let (w1, h1) = painted_bounds(&one);
    let (w15, h15) = painted_bounds(&dev);
    let near = |got: u32, want: f32| (got as f32 - want).abs() <= 3.0;
    assert!(near(w15, w1 as f32 * 1.5) && near(h15, h1 as f32 * 1.5), "{w1}x{h1} -> {w15}x{h15}");
    assert_eq!(dev.pixel(10, 10).map(|p| p.alpha()), Some(0), "nothing above-left of the scaled anchor");

    // Sharpness: stretching the 1x render to 750x300 (what the compositor
    // did to a logical-size buffer) is not what was drawn. Glyph edges are
    // where the two differ: text rasterized at the device size has far
    // more fully-covered and fully-empty pixels than interpolated text.
    let stretched = image::imageops::resize(
        &image::RgbaImage::from_raw(500, 200, one.data().to_vec()).unwrap(),
        750,
        300,
        image::imageops::FilterType::Triangle,
    );
    let stretched_alpha: Vec<u8> = stretched.pixels().map(|p| p.0[3]).collect();
    let native_alpha = alpha_plane(&dev);
    assert_eq!(stretched_alpha.len(), native_alpha.len());
    let differing = native_alpha.iter().zip(&stretched_alpha).filter(|(a, b)| a.abs_diff(**b) > 24).count();
    assert!(differing > 200, "the 1.5x render is just the 1x render stretched ({differing} px differ)");

    // Text colour against the panel fill: count the ink pixels at full
    // strength. Interpolation smears them; native rasterization keeps them.
    let ink = |data: &[u8]| {
        data.as_chunks::<4>().0.iter().filter(|p| p[0] > 0xD0 && p[1] > 0xC8 && p[3] == 0xFF).count()
    };
    let native_ink = ink(dev.data());
    let stretched_ink = ink(stretched.as_raw());
    assert!(
        native_ink as f32 > stretched_ink as f32 * 1.15,
        "native text should keep more full-strength ink than stretched text: {native_ink} vs {stretched_ink}"
    );
}

#[test]
fn icons_are_resized_from_the_artwork_not_from_the_1x_copy() {
    let r = Renderer::new().unwrap();
    r.set_scale(2.0);
    let mut pm = tiny_skia::Pixmap::new(600, 400).unwrap();
    r.draw_frame(
        &mut pm,
        &[Placed { x: 20, y: 100, amount: "9".into(), denom: Denom::Divine, tier: Tier::Jackpot, best: false }],
        "",
        false,
    );
    // The icon sits right of the amount at logical x~44..74, y~85..115:
    // device 88..148, 170..230. Detailed artwork gives many distinct colours.
    let mut colors = std::collections::HashSet::new();
    for xx in 70..170u32 {
        for yy in 165..235u32 {
            if let Some(p) = pm.pixel(xx, yy) {
                if p.alpha() != 0 {
                    colors.insert((p.red(), p.green(), p.blue()));
                }
            }
        }
    }
    assert!(colors.len() >= 16, "expected the scaled divine icon, got {} colours", colors.len());
}

fn extra_line(text: &str, into: &[&str], ids: &[&str]) -> khaloni_poe2_core::ee2::request::ExtraRow {
    khaloni_poe2_core::ee2::request::ExtraRow {
        text: text.into(),
        tag: "explicit",
        rolled: Some(27.0),
        lines: vec![text.into()],
        into: into.iter().map(|s| s.to_string()).collect(),
        group: "explicit",
        ids: ids.iter().map(|s| s.to_string()).collect(),
        option: None,
        value: khaloni_poe2_core::trade::FilterValue { min: Some(31.0), max: None },
        lookup: Vec::new(),
        stat_keys: Vec::new(),
    }
}

/// A modifier line no catalog lists is drawn as text only. A checkbox or a
/// value well on it would say it can be searched, and it cannot: the pixels
/// where a searchable row has them are bare panel. A line a total counted
/// is searchable now, and draws both like any row.
#[test]
fn a_line_without_a_trade_id_draws_no_checkbox_and_no_value_wells() {
    use khaloni_poe2::evaluate_ui as ev;

    let mut extras = Vec::new();
    let rows = ev::extra_rows(
        &[
            extra_line("12% increased Wombat Summoning Speed", &[], &[]),
            extra_line("Adds 27 to 36 Fire Damage", &["Total DPS", "Elemental DPS"], &["explicit.stat_709508406"]),
        ],
        &mut extras,
    );
    let panel = ev::Panel {
        header: ev::ItemHeader {
            name: "Horror Bane".into(),
            rarity: "Rare".into(),
            item_level: Some(82),
            requires_level: None,
            base: None,
        },
        rows,
        extras,
        ..ev::Panel::default()
    };
    let r = Renderer::new().unwrap();
    let lay = ev::layout(&panel, &|s| r.evaluate_label_width(s));
    let mut pm = tiny_skia::Pixmap::new(lay.size.0 as u32 + 20, lay.size.1 as u32 + 20).unwrap();
    r.draw_evaluate(&mut pm, &panel, &lay, (10, 10), None, "");

    let colours = |pm: &tiny_skia::Pixmap, rect: &khaloni_poe2::config::Rect| {
        let mut seen = std::collections::BTreeSet::new();
        for y in rect.y..rect.y + rect.h as i32 {
            for x in rect.x..rect.x + rect.w as i32 {
                let p = pm.pixel((x + 10) as u32, (y + 10) as u32).unwrap();
                seen.insert((p.red(), p.green(), p.blue(), p.alpha()));
            }
        }
        seen.len()
    };
    let (flat, drawn) = (&lay.rows[0], &lay.rows[1]);
    for (name, bare, used) in [
        ("checkbox", &flat.check, &drawn.check),
        ("min well", &flat.min_box, &drawn.min_box),
        ("max well", &flat.max_box, &drawn.max_box),
    ] {
        assert_eq!(colours(&pm, bare), 1, "the row with no trade id has something drawn where its {name} would be");
        assert!(colours(&pm, used) > 1, "the counted line's row lost its {name}");
    }
}


// --- the listings table, the hover card and the blocks under the card ----

use khaloni_poe2::evaluate_ui::{self as ev, ListingRow, SellerState};
use khaloni_poe2_core::listing;

/// The fixture's rows as the panel shows them, prices in a 459 ex / 8.4
/// chaos per divine league.
fn fixture_listing_rows() -> Vec<ListingRow> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/tests/fixtures/trade_fetch_full.json");
    let body: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut table = khaloni_poe2_core::ninja::PriceTable::default();
    table.exalted_per_divine = 459.0;
    table.chaos_per_divine = 8.4;
    listing::group(listing::parse_fetch_body(&body, 1_790_000_000, "seller3").0)
        .iter()
        .map(|g| {
            let exalted = g.view.price.as_ref().and_then(|(amount, currency)| match currency.as_str() {
                "exalted" => Some(*amount),
                "divine" => Some(amount * table.exalted_per_divine),
                _ => None,
            });
            ListingRow::priced(g, exalted, &table, 1.0)
        })
        .collect()
}

fn listing_panel() -> ev::Panel {
    ev::Panel {
        header: ev::ItemHeader {
            name: "Sol Wrap".into(),
            rarity: "Rare".into(),
            item_level: Some(81),
            requires_level: Some(65),
            base: Some(ev::BaseToggle { label: "Category: Body Armour".into(), enabled: true }),
        },
        rows: vec![
            ev::StatRow {
                label: "+60 to maximum Life".into(),
                badge: Some(ev::TierBadge { kind: ev::AffixKind::Prefix, tier: 3 }),
                score: Some(3.0),
                min: Some(60.0),
                max: None,
                enabled: true,
                target: Some(ev::Target::Stat(0)),
                hidden: false,
                group: ev::RowGroup::Explicit,
                note: None,
            },
            ev::StatRow {
                label: "+45% to Lightning Resistance".into(),
                badge: Some(ev::TierBadge { kind: ev::AffixKind::Other, tier: 1 }),
                score: None,
                min: Some(45.0),
                max: None,
                enabled: false,
                target: Some(ev::Target::Stat(1)),
                hidden: true,
                group: ev::RowGroup::Explicit,
                note: None,
            },
        ],
        show_hidden: false,
        strictness: ev::Strictness::Broad,
        status: "Broad search, bounds -10%: 20 of 1,934 shown".into(),
        search_id: Some("abc".into()),
        searching: false,
        ..ev::Panel::default()
    }
}

/// Every block filled with the wording the panel can produce, so the font
/// check covers all of it at once.
fn full_panel(rows: Vec<ListingRow>) -> ev::Panel {
    let base = listing_panel();
    let mut extras = Vec::new();
    let mut card_rows = base.rows.clone();
    card_rows.extend(ev::extra_rows(
        &[
            extra_line("+142 to maximum Life", &["Total Life"], &["explicit.stat_3299347043"]),
            extra_line("Right ring slot: Projectiles from Spells Chain +1 times", &[], &["explicit.stat_1555918911"]),
            extra_line("Desecrated Suffix", &[], &["pseudo.pseudo_number_of_unrevealed_suffix_mods"]),
            extra_line("+26% to Monster Critical Damage Bonus", &[], &[]),
        ],
        &mut extras,
    ));
    ev::Panel {
        rows: card_rows,
        extras,
        listings: rows,
        hover: Some(3),
        ninja: Some(ev::NinjaBlock {
            price: "4.6 div".into(),
            band: khaloni_poe2_core::market::band_text_ascii(12),
            direction: "rising".into(),
            volume: "120 div".into(),
            note: khaloni_poe2_core::market::THIN_MARKET.into(),
        }),
        ladder: "cheapest 2 ex, then 5, 6, 6, 8 · 20 of 1,934 matched".into(),
        closest: Some(ev::ClosestBlock {
            lines: vec![
                "3 listings within one tier: 38, 42, 45 ex".into(),
                "the nearest has T1 life, T2 fire res, T3 cold res like yours".into(),
            ],
            nearest: Some("nearest differs by: T2 life (yours T1), no cold res".into()),
        }),
        attribution: vec![ev::AttributionRow {
            label: "T1 life".into(),
            with: "40 ex".into(),
            without: "9 ex".into(),
            text: "with: seller1 40 ex; without: seller4 9 ex".into(),
        }],
        price_fixed: Some(ev::PriceFixedStrip {
            text: "likely price-fixed: 14 listings under 1 aug, next at 5 ex".into(),
            button: "ex/div only".into(),
        }),
        bulk: Some(ev::BulkBlock {
            offers: vec![ev::BulkOffer {
                have: "2 omen-of-whittling".into(),
                want: "7 exalted".into(),
                stock: "12".into(),
                seller: "sellerA#1001".into(),
                state: SellerState::Online,
            }],
            note: "3 offers, cheapest 3 ex each".into(),
        }),
        stack_value: Some("37 x 1.8 chaos = 67 chaos".into()),
        budget_text: "searches 4/30 (5 min)".into(),
        budget_low: true,
        attribute_enabled: true,
        screen_right: None,
        ..base
    }
}

/// The hover card draws every line of the listing's item, each with its
/// tier badge in the gutter: a prefix badge in the cool blue, a suffix
/// badge in the warm amber, the same two the item card's own gutter uses,
/// and the title in the rarity's colour.
#[test]
fn the_card_draws_every_line_with_its_tier_badge() {
    let r = Renderer::new().unwrap();
    let rows = fixture_listing_rows();
    let panel = ev::Panel { listings: rows.clone(), hover: Some(3), ..listing_panel() };
    let lay = ev::layout(&panel, &|s| r.evaluate_label_width(s));
    let card = lay.card.clone().expect("card");
    let (ax, ay) = (10, 10);
    let w = (ax + card.rect.x + card.rect.w as i32 + 10) as u32;
    let h = (ay + lay.size.1.max(card.rect.y + card.rect.h as i32) + 10) as u32;
    let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
    r.draw_evaluate(&mut pm, &panel, &lay, (ax, ay), None, "");

    let painted_in = |x0: i32, y0: i32, x1: i32, y1: i32, keep: &dyn Fn(u8, u8, u8) -> bool| {
        let mut n = 0;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = pm.pixel((ax + x) as u32, (ay + y) as u32).unwrap();
                // The panel fill is near-opaque (238), so only glyph
                // pixels reach full alpha; partly covered edge pixels
                // blend towards the dark fill, so the colour tests below
                // look for the hue, not the exact value.
                if p.alpha() >= 250 && keep(p.red(), p.green(), p.blue()) {
                    n += 1;
                }
            }
        }
        n
    };
    let any = |_r: u8, g: u8, b: u8| g > 0x40 || b > 0x40; // brighter than the panel fill
    let blue = |r: u8, _g: u8, b: u8| b > 0x80 && b > r + 0x20;
    let amber = |r: u8, g: u8, b: u8| r > 0x90 && g > 0x50 && r > b + 0x40;
    let gold = |r: u8, g: u8, b: u8| r > 0x90 && g > 0x60 && b < 0x60;

    let (cx, cw) = (card.rect.x, card.rect.w as i32);
    // Title in gold: Sol Wrap is rare.
    assert!(painted_in(cx, card.title_pos.1 - 20, cx + cw, card.title_pos.1 + 4, &gold) > 30, "no gold title");
    let item = &rows[3].card;
    assert_eq!(card.lines.len(), item.lines.len());
    for (baseline, line) in card.lines.iter().zip(&item.lines) {
        let (top, bottom) = (baseline - 15, baseline + 4);
        assert!(painted_in(card.text_x, top, cx + cw, bottom, &any) > 20, "line not drawn: {}", line.text);
        let gutter = |keep: &dyn Fn(u8, u8, u8) -> bool| painted_in(card.badge_x, top, card.text_x - 2, bottom, keep);
        let has = |letter: char| line.tiers.iter().any(|t| t.starts_with(letter));
        if has('P') {
            assert!(gutter(&blue) > 2, "no blue prefix badge beside: {}", line.text);
        }
        if has('S') {
            assert!(gutter(&amber) > 2, "no amber suffix badge beside: {}", line.text);
        }
        if line.tiers.is_empty() {
            assert_eq!(gutter(&any), 0, "a badge drawn on a line without a tier: {}", line.text);
        }
    }
    // The card is beside the panel, not over it: the panel's own rows are
    // untouched where the card would have covered them.
    assert!(card.rect.x >= lay.size.0);
}

/// Every string the panel can put on screen has an outline in the overlay
/// face: the font lacks the plus-minus and multiplication signs among
/// others, and a missing glyph draws as a gap that reads as a number.
#[test]
fn every_panel_label_can_be_drawn_by_the_overlay_font() {
    let r = Renderer::new().unwrap();
    let panel = full_panel(fixture_listing_rows());
    let text = ev::all_text(&panel, &|s| r.evaluate_label_width(s));
    assert!(text.len() > 60, "the full panel draws many strings, got {}", text.len());
    for s in &text {
        assert!(r.can_draw(s), "not drawable by the overlay font: {s:?}");
    }
    // The strings that exist to be checked here.
    assert!(text.iter().any(|s| s.contains("+/-12%")));
    assert!(text.iter().any(|s| s.contains("x3") || s.contains("x ")), "a folded count or a stack value with an x");
    assert!(text.iter().any(|s| s == "searches 4/30 (5 min)"));
    assert!(text.iter().any(|s| s == "thin market"));
    assert!(!text.iter().any(|s| s.contains('×') || s.contains('±')));
    // The extra rows' notes are among them, and no string claims more than
    // the listings show.
    assert!(text.iter().any(|s| s.ends_with("counted in Total Life")));
    assert!(text.iter().any(|s| s.ends_with(khaloni_poe2_core::ee2::request::NO_TRADE_STAT)));
    for s in &text {
        let lower = s.to_lowercase();
        assert!(!lower.contains("estimate") && !lower.contains("reliability"), "{s:?}");
    }
}

/// A long "differs by" list wraps inside the panel instead of widening it
/// past the screen and running off its right edge.
#[test]
fn a_long_closest_line_wraps_inside_the_panel() {
    let r = Renderer::new().unwrap();
    let measure = |s: &str| r.evaluate_label_width(s);
    let long = "no close match among the cheapest 16; nearest differs by: no reduced duration of bleeding on you, \
                no hits against you have #% reduced critical damage bonus, T1 increased armour vs T2, T2 lightning res, \
                T4 life regeneration per second, no stun threshold, no fire resistance, T3 maximum life vs T1";
    let mut panel = full_panel(fixture_listing_rows());
    panel.closest = Some(ev::ClosestBlock { lines: vec![long.into()], nearest: Some("nearest listing: 6 div by seller4, 9 d".into()) });
    let short = ev::layout(&full_panel(fixture_listing_rows()), &measure);
    let lay = ev::layout(&panel, &measure);
    assert_eq!(lay.size.0, short.size.0, "the long line does not change the panel's width");
    let g = lay.closest.as_ref().expect("the block is laid out");
    assert!(g.texts.len() > 1, "the line wrapped: {:?}", g.texts);
    assert_eq!(g.texts.len(), g.lines.len());
    assert_eq!(g.texts.join(" "), long, "wrapping loses no word");
    for (t, (x, _)) in g.texts.iter().zip(&g.lines) {
        assert!(x + measure(t) <= lay.size.0 - 12, "{t:?} runs past the panel ({} > {})", x + measure(t), lay.size.0);
    }
    let drawn = ev::all_text(&panel, &measure);
    assert!(g.texts.iter().all(|t| drawn.contains(t)), "the labels list the wrapped lines as drawn");
}
