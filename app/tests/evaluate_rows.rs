//! The Evaluate card's derived weapon rows: which of them may drive a trade
//! filter, and that no two rows ever write the same bound.

use khaloni_poe2::evaluate_ui::{weapon_rows, Target};
use khaloni_poe2_core::derived::WeaponStats;

#[test]
fn weapon_rows_never_share_a_search_bound() {
    let w = WeaponStats {
        phys_dps: 412.6,
        ele_dps: 220.1,
        chaos_dps: 99.2,
        total_dps: 731.9,
        aps: 1.45,
        crit_chance: 6.5,
    };
    let rows = weapon_rows(&w);
    let chaos = rows.iter().find(|r| r.label.starts_with("Chaos DPS")).expect("chaos row shown");
    assert_eq!(chaos.label, "Chaos DPS: 99.2", "a display-only line carries its value");
    assert!(chaos.target.is_none(), "the trade site has no chaos-DPS filter; the row is display-only");
    let mut bounds: Vec<_> = rows
        .iter()
        .filter_map(|r| match r.target {
            Some(Target::Weapon(b)) => Some(b),
            _ => None,
        })
        .collect();
    let n = bounds.len();
    bounds.sort_by_key(|b| format!("{b:?}"));
    bounds.dedup();
    assert_eq!(bounds.len(), n, "two rows driving one bound would fight over it");
    // Zero figures are not shown: an unarmed item has no DPS line.
    let none = weapon_rows(&WeaponStats { aps: 1.0, ..WeaponStats::default() });
    assert!(none.iter().all(|r| r.label == "Attacks per Second"));
}
