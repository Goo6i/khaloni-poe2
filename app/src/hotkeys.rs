use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hotkey {
    ScanToggle,
    Hide,
    PriceCheck,
}

pub async fn listen(tx: std::sync::mpsc::Sender<Hotkey>) -> anyhow::Result<()> {
    let gs = GlobalShortcuts::new().await?;
    let session = gs.create_session().await?;
    let shortcuts = vec![
        NewShortcut::new("scan-toggle", "poe2-lens: start/stop scanning").preferred_trigger("F5"),
        NewShortcut::new("overlay-hide", "poe2-lens: hide/show overlay").preferred_trigger("F8"),
        NewShortcut::new("price-check", "poe2-lens: price check hovered item").preferred_trigger("F7"),
    ];
    gs.bind_shortcuts(&session, &shortcuts, None).await?.response()?;
    let mut activated = gs.receive_activated().await?;
    while let Some(a) = activated.next().await {
        let hk = match a.shortcut_id() {
            "scan-toggle" => Hotkey::ScanToggle,
            "overlay-hide" => Hotkey::Hide,
            "price-check" => Hotkey::PriceCheck,
            _ => continue,
        };
        let _ = tx.send(hk);
    }
    Ok(())
}
