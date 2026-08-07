//! Renders representative overlay elements to a PNG so the visual design can
//! be reviewed without the game running. Not shipped; a dev aid.

use khaloni_poe2::appraise_ui::{BaseToggle, EstimateView, ModRow, Panel};
use khaloni_poe2::pricing::{Denom, Tier};
use khaloni_poe2::render::{Placed, Renderer};
use tiny_skia::{Color, Pixmap};

fn main() -> anyhow::Result<()> {
    let r = Renderer::new()?;
    let mut pm = Pixmap::new(980, 760).unwrap();
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

    // Appraisal panel: grouped implicit/explicit mods + base toggle + listings.
    let panel = Panel {
        estimate: Some(EstimateView {
            amount: "5.5".into(),
            denom: Denom::Divine,
            detail: "Range: 0.62-49 div  -  from 23 listing(s)".into(),
            reliability: "Very Low".into(),
            shaky: true,
        }),
        title: "Horror Bane".into(),
        base: Some(BaseToggle { label: "Base: Expert Dualstring Bow".into(), enabled: true }),
        mods: vec![
            ModRow { label: "+25% to Lightning Resistance".into(), tier: Some(3), min: 25.0, max: None, enabled: true, filter_index: 0, tag: "implicit".into() },
            ModRow { label: "Adds 40 to 75 Physical Damage".into(), tier: Some(2), min: 40.0, max: None, enabled: true, filter_index: 1, tag: "explicit".into() },
            ModRow { label: "+180 to maximum Life".into(), tier: Some(4), min: 180.0, max: Some(200.0), enabled: false, filter_index: 2, tag: "explicit".into() },
        ],
        listings: vec!["1 div  Xyz#1234".into(), "2 div  Abc#5678".into()],
        status: "8 shown".into(),
        search_id: Some("abc".into()),
    };
    let lay = khaloni_poe2::appraise_ui::layout(&panel, &|s| r.appraisal_label_width(s));
    r.draw_appraisal(&mut pm, &panel, &lay, (360, 60), None, "");

    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/overlay-preview.png".to_string());
    pm.save_png(&out)?;
    eprintln!("wrote {out}");
    Ok(())
}
