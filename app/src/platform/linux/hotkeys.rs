use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

pub use crate::platform::Hotkey;

/// `price_check` and `overlay` are preferred triggers from the config
/// ("F7"/"F8" by default); the portal treats them as suggestions the user
/// can override in KDE's shortcut settings. `extra` is (id, trigger) for
/// every dynamically-bound action (chat macros, resource shortcuts, panel
/// toggles); changing the set triggers one KDE re-approval.
pub async fn listen(
    tx: std::sync::mpsc::Sender<Hotkey>,
    price_check: String,
    overlay: String,
    extra: Vec<(String, String)>,
) -> anyhow::Result<()> {
    let gs = GlobalShortcuts::new().await?;
    let session = gs.create_session().await?;
    // An empty trigger is an action the caller unbound (a conflict loser,
    // or a hotkey cleared in Settings): it is not registered at all.
    let mut shortcuts = Vec::new();
    if !overlay.is_empty() {
        shortcuts.push(
            NewShortcut::new("overlay-toggle", "khaloni-poe2: overlay on/off")
                .preferred_trigger(overlay.as_str()),
        );
    }
    if !price_check.is_empty() {
        shortcuts.push(
            NewShortcut::new("price-check", "khaloni-poe2: price check hovered item")
                .preferred_trigger(price_check.as_str()),
        );
    }
    for (id, trigger) in extra.iter().filter(|(_, t)| !t.is_empty()) {
        shortcuts.push(
            NewShortcut::new(id.as_str(), "khaloni-poe2: action").preferred_trigger(trigger.as_str()),
        );
    }
    gs.bind_shortcuts(&session, &shortcuts, None).await?.response()?;
    let mut activated = gs.receive_activated().await?;
    while let Some(a) = activated.next().await {
        let hk = match a.shortcut_id() {
            "overlay-toggle" => Hotkey::OverlayToggle,
            "price-check" => Hotkey::PriceCheck,
            id => Hotkey::Extra(id.to_string()),
        };
        let _ = tx.send(hk);
    }
    Ok(())
}
