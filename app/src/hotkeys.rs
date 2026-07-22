use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hotkey {
    /// Master switch: hides the overlay and pauses the whole pipeline.
    /// Everything else (panel detection, focus pause, price freshness) is
    /// automatic, so this is the only state key a user needs.
    OverlayToggle,
    PriceCheck,
}

/// `price_check` and `overlay` are preferred triggers from the config
/// ("F7"/"F8" by default); the portal treats them as suggestions the user
/// can override in KDE's shortcut settings.
pub async fn listen(
    tx: std::sync::mpsc::Sender<Hotkey>,
    price_check: String,
    overlay: String,
) -> anyhow::Result<()> {
    let gs = GlobalShortcuts::new().await?;
    let session = gs.create_session().await?;
    let shortcuts = vec![
        NewShortcut::new("overlay-toggle", "poe2-lens: overlay on/off")
            .preferred_trigger(overlay.as_str()),
        NewShortcut::new("price-check", "poe2-lens: price check hovered item")
            .preferred_trigger(price_check.as_str()),
    ];
    gs.bind_shortcuts(&session, &shortcuts, None).await?.response()?;
    let mut activated = gs.receive_activated().await?;
    while let Some(a) = activated.next().await {
        let hk = match a.shortcut_id() {
            "overlay-toggle" => Hotkey::OverlayToggle,
            "price-check" => Hotkey::PriceCheck,
            _ => continue,
        };
        let _ = tx.send(hk);
    }
    Ok(())
}
