//! Prints the map runs of a game log as JSON, offline:
//! `runs_report --log <Client.txt>`.
//!
//! The file goes through `myruns::cold_read`, the read the overlay does at
//! startup (the last 40 MB, the cut-off first line dropped). The run still
//! open at the end of the file is in progress and is not reported.

use khaloni_poe2_core::runs::Totals;
use serde_json::json;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let log = args
        .iter()
        .position(|a| a == "--log")
        .and_then(|i| args.get(i + 1))
        .ok_or_else(|| anyhow::anyhow!("--log <Client.txt> is required"))?;
    let (mut tracker, _, rest) = khaloni_poe2::myruns::cold_read(std::path::Path::new(log))?;
    // The overlay would get the end of an unterminated last line from the
    // tail; a file that is not growing has it already.
    if !rest.is_empty() {
        tracker.feed(&rest);
    }
    let runs = tracker.finished();
    let totals = Totals::of(runs.iter().copied());
    let report = json!({
        "runs": runs.iter().map(|r| json!({
            "area": r.area,
            "seed": r.seed,
            "started": r.started,
            "ended": r.ended,
            "seconds": r.seconds(),
            "portals": r.portals,
            "mechanics": r.mechanics.iter().map(|m| m.name()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "totals": {
            "maps": totals.maps,
            "maps_unknown_duration": totals.maps_unknown_duration,
            "map_hours": totals.map_hours(),
        },
    });
    println!("{report}");
    Ok(())
}
