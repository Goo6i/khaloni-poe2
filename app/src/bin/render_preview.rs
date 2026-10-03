//! Renders representative overlay elements to a PNG so the visual design can
//! be reviewed without the game running. Not shipped; a dev aid.

use khaloni_poe2::evaluate_ui as ev;
use khaloni_poe2::pricing::{Denom, Tier};
use khaloni_poe2::render::{Placed, Renderer};
use tiny_skia::{Color, Pixmap};

/// A mod line with a tier badge, a roll score, and a filter behind it.
fn mod_row(label: &str, kind: ev::AffixKind, tier: u8, score: f32, min: f64, i: usize) -> ev::StatRow {
    ev::StatRow {
        label: label.into(),
        badge: Some(ev::TierBadge { kind, tier }),
        score: Some(score),
        min: Some(min),
        max: None,
        enabled: true,
        target: Some(ev::Target::Stat(i)),
        hidden: false,
        group: ev::RowGroup::Explicit,
        note: None,
    }
}

/// A computed weapon figure: searchable as an equipment_filters minimum,
/// so it gets a checkbox and a min box but never a max box.
fn weapon_row(label: &str, min: f64, bound: ev::EquipKey, enabled: bool) -> ev::StatRow {
    ev::StatRow {
        label: label.into(),
        badge: None,
        score: None,
        min: Some(min),
        max: None,
        enabled,
        target: Some(ev::Target::Equipment(bound)),
        hidden: false,
        group: ev::RowGroup::Property,
        note: None,
    }
}

/// A display-only property line: nothing searchable behind it, so no
/// checkbox and no boxes.
fn derived(label: &str) -> ev::StatRow {
    ev::StatRow {
        label: label.into(),
        badge: None,
        score: None,
        min: None,
        max: None,
        enabled: false,
        target: None,
        hidden: false,
        group: ev::RowGroup::Property,
        note: None,
    }
}

/// A pseudo-total row: an ordinary stat filter, collapsed by default.
fn pseudo_row(label: &str, min: f64, i: usize) -> ev::StatRow {
    ev::StatRow {
        label: label.into(),
        badge: None,
        score: None,
        min: Some(min),
        max: None,
        enabled: false,
        target: Some(ev::Target::Stat(i)),
        hidden: true,
        group: ev::RowGroup::Explicit,
        note: None,
    }
}

fn main() -> anyhow::Result<()> {
    let r = Renderer::new()?;

    // Evaluate item card: a rare weapon as the game would show it, plus the
    // tiering and scoring gutters and the filter column the game does not.
    let ep = ev::Panel {
        header: ev::ItemHeader {
            name: "Horror Bane".into(),
            rarity: "Rare".into(),
            item_level: Some(81),
            requires_level: Some(67),
            base: Some(ev::BaseToggle { label: "Expert Dualstring Bow".into(), enabled: true }),
        },
        rows: vec![
            weapon_row("Physical DPS", 412.6, ev::EquipKey::Pdps, true),
            weapon_row("Total DPS", 731.9, ev::EquipKey::Dps, false),
            weapon_row("Critical Hit Chance", 11.5, ev::EquipKey::Crit, false),
            mod_row("Adds 40 to 75 Physical Damage", ev::AffixKind::Prefix, 2, 4.6, 40.0, 0),
            mod_row("+180 to maximum Life", ev::AffixKind::Prefix, 9, 0.8, 180.0, 1),
            mod_row("24% increased Critical Damage Bonus", ev::AffixKind::Suffix, 1, 4.0, 24.0, 2),
            mod_row("+23 to Accuracy Rating", ev::AffixKind::Suffix, 5, 1.7, 23.0, 3),
            ev::StatRow {
                enabled: false,
                ..mod_row("+31% to Lightning Resistance", ev::AffixKind::Suffix, 3, 2.9, 31.0, 4)
            },
            ev::StatRow {
                hidden: true,
                ..mod_row("+12 to Dexterity", ev::AffixKind::Suffix, 7, 1.1, 12.0, 5)
            },
            pseudo_row("Total Elemental Resistance", 31.0, 6),
            pseudo_row("Total Attributes", 12.0, 7),
        ],
        show_hidden: false,
        strictness: ev::Strictness::Broad,
        status: "Broad search, bounds -10%: 2 of 23 shown".into(),
        search_id: Some("k9f2".into()),
        searching: false,
        ..ev::Panel::default()
    };

    // What the search found, under the first card: two listings and the
    // ladder they make. `panel_preview` renders every block from the
    // fixture; this is the one-glance version.
    let card = ev::Card { name: String::new(), base: String::new(), rarity: String::new(), figures: Vec::new(), lines: Vec::new() };
    let listing = |price: &str, age: &str, seller: &str, state: ev::SellerState| ev::ListingRow {
        price: price.into(),
        raw: String::new(),
        age: age.into(),
        seller: seller.into(),
        state,
        mine: false,
        times: 1,
        stack: None,
        ilvl: Some(81),
        quality: None,
        gem_level: None,
        corrupted: false,
        instant_buyout: true,
        card: card.clone(),
    };
    let ep = ev::Panel {
        listings: vec![
            listing("4.8 div", "2 h", "Vaalstep#3311", ev::SellerState::Online),
            listing("6.2 div", "3 d", "Emberlark#8074", ev::SellerState::Offline),
        ],
        ladder: "cheapest 4.8 div, then 6.2 · 2 of 23 matched".into(),
        budget_text: "searches 4/30 (5 min)".into(),
        ..ep
    };

    // Second state of the same card: a magic item, hidden rows expanded, a
    // max box being typed into, and nothing found yet.
    let ep2 = ev::Panel {
        header: ev::ItemHeader {
            name: "Kraken Grip Sapphire Ring".into(),
            rarity: "Magic".into(),
            item_level: Some(74),
            requires_level: None,
            base: None,
        },
        show_hidden: true,
        strictness: ev::Strictness::Quick,
        status: "searching...".into(),
        rows: vec![
            derived("Energy Shield: 46"),
            mod_row("+35% to Cold Resistance", ev::AffixKind::Suffix, 2, 3.4, 35.0, 0),
            ev::StatRow {
                badge: Some(ev::TierBadge { kind: ev::AffixKind::Other, tier: 1 }),
                ..mod_row("+18 to maximum Mana", ev::AffixKind::Prefix, 6, 1.2, 18.0, 1)
            },
            ev::StatRow { hidden: true, ..mod_row("+9 to Intelligence", ev::AffixKind::Suffix, 8, 0.4, 9.0, 2) },
        ],
        search_id: None,
        searching: true,
        ..ev::Panel::default()
    };

    // Lay the cards out from their own measured sizes, so the canvas fits
    // whatever the panels turn out to be rather than a hardcoded guess that
    // silently clips when a label grows.
    let elay = ev::layout(&ep, &|s| r.evaluate_label_width(s));
    let elay2 = ev::layout(&ep2, &|s| r.evaluate_label_width(s));
    let (x1, y) = (360, 60);
    let x2 = x1 + elay.size.0 + 40;
    let w = (x2 + elay2.size.0 + 40) as u32;
    let h = (y + elay.size.1.max(elay2.size.1) + 60).max(400) as u32;
    let mut pm = Pixmap::new(w, h).unwrap();
    // Simulate the game behind the overlay: a muted dark backdrop.
    pm.fill(Color::from_rgba8(0x22, 0x1c, 0x16, 0xFF));

    // Price labels, one per tier + a best pick, with currency icons.
    let labels = vec![
        Placed { x: 60, y: 70, amount: "3".into(), denom: Denom::Chaos, tier: Tier::Junk, best: false },
        Placed { x: 60, y: 130, amount: "12".into(), denom: Denom::Exalted, tier: Tier::Decent, best: false },
        Placed { x: 60, y: 190, amount: "2.4".into(), denom: Denom::Divine, tier: Tier::Jackpot, best: true },
        Placed { x: 60, y: 250, amount: "?".into(), denom: Denom::None, tier: Tier::Unknown, best: false },
    ];
    r.draw_frame(&mut pm, &labels, "1 div = 487 ex", false);

    r.draw_evaluate(&mut pm, &ep, &elay, (x1, y), None, "");
    r.draw_evaluate(&mut pm, &ep2, &elay2, (x2, y), Some((1, ev::Field::Max)), "42");

    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/overlay-preview.png".to_string());
    pm.save_png(&out)?;
    eprintln!("wrote {out}");
    Ok(())
}
