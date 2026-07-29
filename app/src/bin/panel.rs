//! Standalone launcher for the control-panel web UI, for developing and
//! testing the panel without the full overlay. Loads the real reference data
//! (cached/fetched), then serves the panel and blocks.
//! Run: `cargo run --bin panel`.

use poe2_lens::webserver::{self, Ctx};

fn main() -> anyhow::Result<()> {
    let cache = directories::ProjectDirs::from("", "", "poe2-lens")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    eprintln!("loading reference data (cache: {})...", cache.display());
    let r = webserver::reference_data(&cache);
    eprintln!(
        "reference: {} affixes, {} items, {} uniques, {} keystones",
        r.affixes.len(), r.items.len(), r.uniques.len(), r.keystones.len()
    );

    let ctx = Ctx {
        affixes: r.affixes, items: r.items, uniques: r.uniques, keystones: r.keystones, categories: r.categories, leveling: r.leveling,
        index_html: webserver::INDEX_HTML.to_string(), csrf_token: String::new(), port: 0,
    };
    let port = webserver::start(ctx)?;
    eprintln!("control panel: http://127.0.0.1:{port}");
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
