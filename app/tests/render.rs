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
            base_type: None,
        },
        rows,
        extras,
        ..ev::Panel::default()
    };
    let r = Renderer::new().unwrap();
    let lay = ev::layout(&panel, &|s| r.evaluate_label_width(s));
    let mut pm = tiny_skia::Pixmap::new(lay.size.0 as u32 + 20, lay.size.1 as u32 + 20).unwrap();
    r.draw_evaluate(&mut pm, &panel, &lay, (10, 10), None, "", std::time::Instant::now());

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
            base_type: None,
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
        header: ev::ItemHeader { base_type: Some("Soldier Cuirass".into()), ..base.header.clone() },
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
    r.draw_evaluate(&mut pm, &panel, &lay, (ax, ay), None, "", std::time::Instant::now());

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
    // The base line under the name, and the group headings as drawn.
    assert!(text.iter().any(|s| s == "Soldier Cuirass"));
    assert!(text.iter().any(|s| s.starts_with("MODIFIERS \u{b7} ")), "{text:?}");
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

use khaloni_poe2::ornament::{FrameMetrics, Scale};
use khaloni_poe2::theme;

fn rgb_of(pm: &tiny_skia::Pixmap, x: u32, y: u32) -> (u8, u8, u8, u8) {
    let p = pm.pixel(x, y).expect("inside");
    // tiny-skia stores premultiplied; demultiply for an opaque pixel.
    (p.red(), p.green(), p.blue(), p.alpha())
}

fn bytes(c: u32) -> (u8, u8, u8) {
    (((c >> 16) & 0xff) as u8, ((c >> 8) & 0xff) as u8, (c & 0xff) as u8)
}

#[test]
fn the_frame_outer_line_lands_on_the_surface_edge_at_every_scale() {
    let r = Renderer::new().unwrap();
    for scale in [1.0f32, 1.25, 1.5, 2.0] {
        r.set_scale(scale);
        let (w, h) = ((200.0 * scale).ceil() as u32, (100.0 * scale).ceil() as u32);
        let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
        r.surface(&mut pm, 0.0, 0.0, 200.0, 100.0);
        let metrics = FrameMetrics::new(Scale { device: scale, unit: 1.0 });
        let mid = h / 2;
        let (gr, gg, gb) = bytes(theme::BORDER_GOLD);
        let (pr, pg, pb) = bytes(theme::BG_PANEL);
        // Outer line: the first column, dim gold, one hairline wide.
        assert_eq!(rgb_of(&pm, 0, mid), (gr, gg, gb, 255), "scale {scale}: outer line");
        let after_outer = metrics.line as u32;
        assert_eq!(rgb_of(&pm, after_outer, mid), (pr, pg, pb, 255), "scale {scale}: panel right after the outer line");
        // Inner line: `gap` in, gold over panel at 30 %, then panel again.
        let inner = metrics.gap as u32;
        assert_ne!(rgb_of(&pm, inner, mid), (pr, pg, pb, 255), "scale {scale}: inner line painted");
        assert_eq!(rgb_of(&pm, inner + metrics.line as u32, mid), (pr, pg, pb, 255), "scale {scale}: panel after inner line");
        // The last column is the outer line too.
        assert_eq!(rgb_of(&pm, w - 1, mid), (gr, gg, gb, 255), "scale {scale}: right outer line");
    }
}

#[test]
fn a_section_heading_paints_a_gold_diamond_before_its_text_and_a_rule_after() {
    let r = Renderer::new().unwrap();
    let mut pm = tiny_skia::Pixmap::new(400, 40).unwrap();
    pm.fill(theme::color(theme::BG_PANEL));
    r.section_heading(&mut pm, 10.0, 24.0, 380.0, "Prefixes · 3");
    let (pr, pg, pb) = bytes(theme::BG_PANEL);
    // Something gold-ish in the first 8 px (the diamond).
    let diamond = (10..18u32).any(|x| (14..26u32).any(|y| rgb_of(&pm, x, y).0 > pr + 40));
    assert!(diamond, "no diamond painted");
    // The rule runs to the right edge: painted near x=380, fading, so lighter than panel.
    let rule_px = rgb_of(&pm, 120, 19);
    assert!(rule_px.0 > pr, "rule not painted at x=120: {rule_px:?}");
    assert_eq!(rgb_of(&pm, 395, 5), (pr, pg, pb, 255), "nothing above the heading row");
}

#[test]
fn a_top_tier_badge_is_filled_gold_and_a_lower_one_is_outlined() {
    let r = Renderer::new().unwrap();
    let mut pm = tiny_skia::Pixmap::new(200, 40).unwrap();
    pm.fill(theme::color(theme::BG_PANEL));
    let w1 = r.tier_badge(&mut pm, 10.0, 10.0, "P1", theme::AFFIX_PREFIX, true);
    let w2 = r.tier_badge(&mut pm, 100.0, 10.0, "S3", theme::AFFIX_SUFFIX, false);
    assert!(w1 > 16.0 && w2 > 16.0);
    let (tr, tg, tb) = bytes(theme::TIER_TOP);
    // Inside the top badge, away from its text: the gold fill.
    assert_eq!(rgb_of(&pm, 12, 12), (tr, tg, tb, 255), "top badge fill");
    // Inside the outlined badge, away from its text and edge: still panel.
    let (pr, pg, pb) = bytes(theme::BG_PANEL);
    assert_eq!(rgb_of(&pm, 103, 13), (pr, pg, pb, 255), "outlined badge keeps the panel inside");
}

#[test]
fn the_plate_button_fills_its_edges_at_mid_height_without_a_seam() {
    let r = Renderer::new().unwrap();
    let mut pm = tiny_skia::Pixmap::new(160, 60).unwrap();
    pm.fill(theme::color(theme::BG_PANEL));
    let rect = khaloni_poe2::config::Rect { x: 10, y: 10, w: 120, h: 28 };
    r.plate_button(&mut pm, &rect, 0.0, 0.0, "Search", 0.0, true);
    let (pr, pg, pb) = bytes(theme::BG_PANEL);
    // The notch columns at the left and right edges at the vertical middle: plate, not panel.
    for x in [10u32, 11, 128, 129] {
        for y in [23u32, 24, 25] {
            let px = rgb_of(&pm, x, y);
            assert_ne!((px.0, px.1, px.2), (pr, pg, pb), "panel shows through the plate at ({x}, {y})");
        }
    }
    // The top of the plate is lighter than its bottom (the gradient runs down).
    let top = rgb_of(&pm, 60, 13);
    let bottom = rgb_of(&pm, 60, 35);
    assert!(top.0 > bottom.0, "gradient top {top:?} should be lighter than bottom {bottom:?}");
}

#[test]
fn a_one_line_popup_still_frames_cleanly() {
    // Narrower than two frame arms in height: corners meet halfway and
    // nothing is painted outside the popup's rect.
    let r = Renderer::new().unwrap();
    let popup = Popup { title: "Exalted Orb".into(), lines: vec![], expires: std::time::Instant::now() };
    let (w, h) = r.popup_size(&popup);
    let mut pm = tiny_skia::Pixmap::new((w + 20) as u32, (h + 20) as u32).unwrap();
    r.draw_popup(&mut pm, &popup, (10, 10));
    let (pr, pg, pb) = bytes(theme::BG_PANEL);
    let (gr, gg, gb) = bytes(theme::BORDER_GOLD);
    // The popup is shorter than two frame arms, so its middle row lies in
    // the corners' fade: the outer line there is somewhere between the
    // frame's dim gold and the corner's bright gold, and opaque.
    let (br, _, _) = bytes(theme::GOLD);
    let left = rgb_of(&pm, 10, 10 + h as u32 / 2);
    assert!(left.3 == 255 && left.0 >= gr && left.0 <= br, "left outer line {left:?} not gold between {gr} and {br}");
    let _ = (gg, gb);
    assert_eq!(rgb_of(&pm, 10 + w as u32 / 2, 10 + h as u32 / 2), (pr, pg, pb, 255), "opaque panel inside");
    assert_eq!(pm.pixel(5, 5).unwrap().alpha(), 0, "nothing painted outside");
    assert_eq!(pm.pixel(10 + w as u32 + 5, 10 + h as u32 / 2).unwrap().alpha(), 0, "nothing painted right of it");
}

#[test]
fn the_popup_title_is_drawn_in_the_name_face_on_the_dark_surface() {
    let r = Renderer::new().unwrap();
    let popup = sample_popup();
    let (w, h) = r.popup_size(&popup);
    let mut pm = tiny_skia::Pixmap::new(w as u32, h as u32).unwrap();
    r.draw_popup(&mut pm, &popup, (0, 0));
    let (pr, pg, pb) = bytes(theme::BG_PANEL);
    // The title row holds pixels lighter than the panel (the text).
    let lit = (12..w as u32 - 12).any(|x| (12..34u32).any(|y| rgb_of(&pm, x, y).0 > pr + 60));
    assert!(lit, "no title drawn");
    let _ = (pg, pb);
}

#[test]
fn the_nameplate_is_tinted_with_the_rarity_colour_at_its_top() {
    let r = Renderer::new().unwrap();
    let panel = listing_panel(); // rarity "Rare"
    let lay = ev::layout(&panel, &|s| r.evaluate_label_width(s));
    let mut pm = tiny_skia::Pixmap::new(lay.size.0 as u32, lay.size.1 as u32).unwrap();
    r.draw_evaluate(&mut pm, &panel, &lay, (0, 0), None, "", std::time::Instant::now());
    let top = theme::blend(theme::BG_NAMEPLATE, theme::RARITY_RARE, theme::BANNER_TINT);
    let (tr, tg, tb) = bytes(top);
    // A pixel in the banner's top row, left of the name, right of the frame.
    let y = (lay.banner.y + 1) as u32;
    let got = rgb_of(&pm, 14, y);
    assert!((got.0 as i32 - tr as i32).abs() <= 1 && (got.1 as i32 - tg as i32).abs() <= 1 && (got.2 as i32 - tb as i32).abs() <= 1,
        "banner top {got:?} is not the rare tint {:?}", (tr, tg, tb));
    // Its bottom row is the plain nameplate.
    let (nr, ng, nb) = bytes(theme::BG_NAMEPLATE);
    let bottom = rgb_of(&pm, 14, (lay.banner.y + lay.banner.h as i32 - 1) as u32);
    assert!((bottom.0 as i32 - nr as i32).abs() <= 1 && (bottom.1 as i32 - ng as i32).abs() <= 1 && (bottom.2 as i32 - nb as i32).abs() <= 1, "banner bottom {bottom:?}");
}

#[test]
fn group_headings_and_badge_pills_are_drawn_for_every_visible_group() {
    let r = Renderer::new().unwrap();
    let panel = full_panel(fixture_listing_rows());
    let lay = ev::layout(&panel, &|s| r.evaluate_label_width(s));
    let mut pm = tiny_skia::Pixmap::new(lay.size.0 as u32, lay.size.1 as u32).unwrap();
    r.draw_evaluate(&mut pm, &panel, &lay, (0, 0), None, "", std::time::Instant::now());
    let (pr, _, _) = bytes(theme::BG_PANEL);
    for (baseline, _) in &lay.group_heads {
        let y = (*baseline - 4) as u32;
        let gold = (12..20u32).any(|x| rgb_of(&pm, x, y).0 > pr + 40);
        assert!(gold, "no heading diamond at baseline {baseline}");
    }
}

#[test]
fn the_art_cache_keeps_its_source_alive_so_a_reused_address_cannot_alias() {
    // Draw art A, drop every outside reference to it, allocate art B of the
    // same size (which may land at A's address), draw B: B's pixels must
    // show, never A's.
    let r = Renderer::new().unwrap();
    let green = {
        let mut img = image::RgbaImage::new(64, 64);
        for p in img.pixels_mut() { *p = image::Rgba([0, 200, 0, 255]); }
        img
    };
    let red = {
        let mut img = image::RgbaImage::new(64, 64);
        for p in img.pixels_mut() { *p = image::Rgba([200, 0, 0, 255]); }
        img
    };
    let slot = khaloni_poe2::config::Rect { x: 0, y: 0, w: 48, h: 48 };
    let mut pm = tiny_skia::Pixmap::new(48, 48).unwrap();
    {
        let a = std::sync::Arc::new(green);
        r.draw_art(&mut pm, &a, &slot, 0.0, 0.0);
    }
    for _ in 0..16 {
        let b = std::sync::Arc::new(red.clone());
        let mut pm2 = tiny_skia::Pixmap::new(48, 48).unwrap();
        r.draw_art(&mut pm2, &b, &slot, 0.0, 0.0);
        let c = rgb_of(&pm2, 24, 24);
        assert!(c.0 > 150 && c.1 < 60, "stale art drawn: {c:?}");
    }
}

#[test]
fn art_is_drawn_inside_its_slot_when_the_panel_has_it() {
    let r = Renderer::new().unwrap();
    let mut panel = listing_panel();
    let mut img = image::RgbaImage::new(64, 64);
    for p in img.pixels_mut() { *p = image::Rgba([0, 200, 0, 255]); }
    panel.art = Some(ev::Art::new(img));
    let lay = ev::layout(&panel, &|s| r.evaluate_label_width(s));
    let slot = lay.art.expect("slot");
    let mut pm = tiny_skia::Pixmap::new(lay.size.0 as u32, lay.size.1 as u32).unwrap();
    r.draw_evaluate(&mut pm, &panel, &lay, (0, 0), None, "", std::time::Instant::now());
    let centre = rgb_of(&pm, (slot.x + 24) as u32, (slot.y + 24) as u32);
    assert!(centre.1 > 150 && centre.0 < 60, "art not composited: {centre:?}");
}

#[test]
fn an_opening_panel_is_drawn_faint_and_risen_then_settles() {
    use std::time::{Duration, Instant};
    let r = Renderer::new().unwrap();
    let mut panel = listing_panel();
    let t0 = Instant::now();
    panel.opened_at = Some(t0);
    let lay = ev::layout(&panel, &|s| r.evaluate_label_width(s));
    let size = ((lay.size.0 + 20) as u32, (lay.size.1 + 20) as u32);
    let mut early = tiny_skia::Pixmap::new(size.0, size.1).unwrap();
    r.draw_evaluate(&mut early, &panel, &lay, (10, 10), None, "", t0 + Duration::from_millis(10));
    let mut late = tiny_skia::Pixmap::new(size.0, size.1).unwrap();
    r.draw_evaluate(&mut late, &panel, &lay, (10, 10), None, "", t0 + Duration::from_secs(1));
    let mid = (10 + lay.size.0 / 2) as u32;
    let y = (10 + lay.size.1 / 2) as u32;
    assert!(early.pixel(mid, y).unwrap().alpha() < late.pixel(mid, y).unwrap().alpha(), "early frame is fainter");
    assert_eq!(late.pixel(mid, y).unwrap().alpha(), 255, "settled frame is opaque");
    // Risen: the top frame line is lower early than late.
    let top_late = (10..30u32).find(|yy| late.pixel(mid, *yy).unwrap().alpha() > 0).unwrap();
    let top_early = (10..30u32).find(|yy| early.pixel(mid, *yy).unwrap().alpha() > 0).unwrap();
    assert!(top_early > top_late, "early {top_early} should sit below late {top_late}");
}

#[test]
fn the_last_appearing_frame_lands_where_the_settled_frame_does() {
    use std::time::{Duration, Instant};
    let r = Renderer::new().unwrap();
    // 2 * 1.25 = 2.5 device px: the overlay rounds that half toward zero.
    r.set_scale(1.25);
    let settled = listing_panel();
    let lay = ev::layout(&settled, &|s| r.evaluate_label_width(s));
    let size = ((lay.size.0 as f32 * 1.25) as u32 + 20, (lay.size.1 as f32 * 1.25) as u32 + 20);
    let t0 = Instant::now();
    let opening = ev::Panel { opened_at: Some(t0), ..settled.clone() };
    let mut last = tiny_skia::Pixmap::new(size.0, size.1).unwrap();
    r.draw_evaluate(&mut last, &opening, &lay, (2, 2), None, "", t0 + theme::APPEAR - Duration::from_millis(1));
    let mut direct = tiny_skia::Pixmap::new(size.0, size.1).unwrap();
    r.draw_evaluate(&mut direct, &settled, &lay, (2, 2), None, "", t0);
    let edge = |pm: &tiny_skia::Pixmap| {
        let y = size.1 / 2;
        let x = (0..size.0).find(|x| pm.pixel(*x, y).unwrap().alpha() > 0).unwrap();
        let mid = size.0 / 2;
        let top = (0..size.1).find(|yy| pm.pixel(mid, *yy).unwrap().alpha() > 0).unwrap();
        (x, top)
    };
    assert_eq!(edge(&last), edge(&direct));
}

#[test]
fn the_three_orbs_give_their_embedded_icon_as_art_and_other_currencies_none() {
    let r = Renderer::new().unwrap();
    for name in ["Divine Orb", "Exalted Orb", "Chaos Orb"] {
        let denom = Denom::of_currency(name).unwrap_or_else(|| panic!("{name} is a denomination"));
        assert!(r.icon_image(denom).is_some_and(|img| img.width() > 0), "{name}");
    }
    assert_eq!(Denom::of_currency("Greater Exalted Orb"), None);
    assert_eq!(Denom::of_currency("Orb of Annulment"), None);
    assert!(r.icon_image(Denom::None).is_none());
}

#[test]
fn hairlines_are_one_device_pixel_tall_at_fractional_scales() {
    use khaloni_poe2::ornament::Scale;
    use khaloni_poe2::theme;
    for scale in [1.25_f32, 1.5] {
        let r = Renderer::new().unwrap();
        r.set_scale(scale);
        let want = Scale { device: scale, unit: 1.0 }.hairline();
        for y in 10..40 {
            let mut pm = tiny_skia::Pixmap::new(80, 80).unwrap();
            r.hairline(&mut pm, 0.0, y as f32, 40.0, theme::TEXT);
            let painted = (0..80).filter(|&row| pm.pixel(5, row).map_or(0, |p| p.alpha()) > 0).count() as i32;
            assert_eq!(painted, want, "hairline at y={y}, scale {scale}");
        }
    }
}
