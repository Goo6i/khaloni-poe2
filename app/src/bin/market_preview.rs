//! Renders the market panel to PNGs from the cached overviews, offline:
//! `market_preview --cache <dir> --league <name> --out <dir>`. Writes
//! `mechanics.png`, `group.png` (the first mechanic group opened),
//! `movers.png` and `loading.png`, at scale 1. A dev aid; not shipped.
//!
//! With `--log <Client.txt>` it also writes the "My runs" tab twice from
//! that log's runs: `my-runs.png` without any stash data, the state of an
//! overlay with no account set, and `my-runs-with-income.png` with stash
//! snapshots MADE UP here (the quantities are invented, cut along the real
//! runs so that every kind of line shows). `labels.txt` lists every string
//! each view drew, prefixed by the view's file stem and a tab.

use std::sync::Arc;

use khaloni_poe2::market_ui::{self, Action, Freshness, Panel, Tab, View};
use khaloni_poe2::render::Renderer;
use khaloni_poe2::myruns;
use khaloni_poe2_core::income::{self, Holding, Snapshot, StashAccess};
use khaloni_poe2_core::market::Floors;
use khaloni_poe2_core::runs::{Mechanic, Run};
use tiny_skia::{Color, Pixmap};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

/// Invented snapshots for the preview: one wherever the runs change
/// between showing `m` and not, so that the blocks in between are pure for
/// it or free of it. Each map adds a fifth of a divine, the exalted price
/// drifts so that revaluation shows, and the last snapshot loses a stack,
/// which leaves the last block pending.
fn invented_snapshots(runs: &[Run], m: Mechanic) -> Vec<Snapshot> {
    let mut by_end: Vec<&Run> = runs.iter().collect();
    by_end.sort_by_key(|r| r.ended);
    let recent = &by_end[by_end.len().saturating_sub(80)..];
    let Some(first) = recent.first() else { return Vec::new() };
    let mut cuts = vec![first.ended - 1];
    for pair in recent.windows(2) {
        if pair[0].saw(m) != pair[1].saw(m) {
            cuts.push(pair[0].ended);
        }
    }
    cuts.extend(recent.last().map(|r| r.ended));
    cuts.dedup();
    let last = cuts.len() - 1;
    cuts.iter()
        .enumerate()
        .map(|(i, &at)| {
            let maps = recent.iter().filter(|r| r.ended <= at).count() as u64;
            let mut items = std::collections::BTreeMap::new();
            items.insert("Divine Orb".to_string(), Holding { qty: 40 + maps / 5, price_div: Some(1.0) });
            items.insert(
                "Exalted Orb".to_string(),
                Holding { qty: 900 + 7 * maps, price_div: Some(0.0024 - 0.00001 * i as f64) },
            );
            let chaos = if i == last { 0 } else { 300 };
            items.insert("Chaos Orb".to_string(), Holding { qty: chaos, price_div: Some(0.03) });
            Snapshot { at, league: "preview".into(), items }
        })
        .collect()
}

fn labels(stem: &str, r: &Renderer, p: &Panel, out: &mut String) {
    for text in market_ui::all_text(p, &|s| r.evaluate_label_width(s)) {
        if !text.is_empty() {
            out.push_str(&format!("{stem}\t{text}\n"));
        }
    }
}

fn render(r: &Renderer, p: &Panel, path: &std::path::Path, drawn: &mut String) -> anyhow::Result<()> {
    labels(&path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), r, p, drawn);
    let lay = market_ui::layout(p, &|s| r.evaluate_label_width(s));
    let (w, h) = ((lay.w + 40) as u32, (lay.h + 40) as u32);
    let mut pm = Pixmap::new(w, h).ok_or_else(|| anyhow::anyhow!("pixmap {w}x{h}"))?;
    // A mid-dark backdrop standing in for the game, so the panel's own
    // translucent fill reads the way it does in play.
    pm.fill(Color::from_rgba8(0x2A, 0x2E, 0x33, 0xFF));
    r.draw_market(&mut pm, p, &lay, (20, 20));
    pm.save_png(path)?;
    println!("{} {}x{}", path.display(), w, h);
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cache = arg(&args, "--cache").ok_or_else(|| anyhow::anyhow!("--cache <dir> is required"))?;
    let league = arg(&args, "--league").ok_or_else(|| anyhow::anyhow!("--league <name> is required"))?;
    let out = std::path::PathBuf::from(arg(&args, "--out").ok_or_else(|| anyhow::anyhow!("--out <dir> is required"))?);
    std::fs::create_dir_all(&out)?;

    let market = khaloni_poe2::prices::market_from_cache(std::path::Path::new(&cache), &league);
    if market.source.is_empty() {
        anyhow::bail!("no cached overviews for {league} in {cache}");
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let view = Arc::new(View::build(&market, Floors::default(), false, 1.0));
    let r = Renderer::new()?;

    let mut drawn = String::new();
    let mut p = Panel::loading(&league);
    render(&r, &p, &out.join("loading.png"), &mut drawn)?;

    // Read from disk, so by the overlay's own flag these prices are stale.
    p.set_data(&league, Some(view.clone()), Freshness::at(now, &market, true, true));
    render(&r, &p, &out.join("mechanics.png"), &mut drawn)?;

    let first = view.mechanics.first().map(|g| g.category.clone()).ok_or_else(|| anyhow::anyhow!("no mechanic group"))?;
    market_ui::apply(&mut p, &Action::OpenGroup(first));
    render(&r, &p, &out.join("group.png"), &mut drawn)?;

    market_ui::apply(&mut p, &Action::SetTab(Tab::Movers));
    render(&r, &p, &out.join("movers.png"), &mut drawn)?;

    if let Some(log) = arg(&args, "--log") {
        // The overlay's own startup read of the log.
        let (tracker, _, _) = myruns::cold_read(std::path::Path::new(&log))?;
        let runs: Vec<Run> = tracker.finished().into_iter().cloned().collect();
        let now_local = tracker.last_seen().unwrap_or(0);
        market_ui::apply(&mut p, &Action::SetTab(Tab::MyRuns));

        let missing = StashAccess::from_credentials("", "");
        let bare = income::summarize(&runs, &[], now_local, missing);
        p.set_runs(myruns::Shown::Runs(Arc::new(myruns::View::build(&bare))));
        render(&r, &p, &out.join("my-runs.png"), &mut drawn)?;

        let most_seen = Mechanic::ALL
            .into_iter()
            .max_by_key(|m| runs.iter().filter(|r| r.saw(*m)).count())
            .unwrap_or(Mechanic::Ritual);
        let snapshots = invented_snapshots(&runs, most_seen);
        let with_income = income::summarize(&runs, &snapshots, now_local, StashAccess::Ready);
        p.set_runs(myruns::Shown::Runs(Arc::new(myruns::View::build(&with_income))));
        // Nobody should take this image's rates for their own.
        p.league = "PREVIEW WITH INVENTED STASH DATA".to_string();
        render(&r, &p, &out.join("my-runs-with-income.png"), &mut drawn)?;
    }
    std::fs::write(out.join("labels.txt"), drawn)?;
    Ok(())
}
