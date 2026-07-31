//! Offline reward-region detector run against a dumped frame:
//! `regiondbg <frame.png>` prints every candidate decision and the verdict.
//! Pair with KHALONI_REGION_DUMP=1, which saves the live frame this replays.

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: regiondbg <frame.png>"))?;
    // The detector's decision logging keys off this variable.
    std::env::set_var("KHALONI_DEBUG", "1");
    let img = image::open(&path)?.to_luma8();
    eprintln!("frame {}x{}", img.width(), img.height());
    match khaloni_poe2::autoregion::detect_reward_region(&img) {
        Some(r) => eprintln!("REGION {}x{} at ({},{})", r.x1 - r.x0, r.y1 - r.y0, r.x0, r.y0),
        None => eprintln!("NO REGION"),
    }
    Ok(())
}
