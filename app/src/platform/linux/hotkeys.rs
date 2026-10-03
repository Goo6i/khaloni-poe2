use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;

pub use crate::platform::Hotkey;
use crate::platform::{HotkeyBindings, HotkeyRebind};

/// `price_check` is a preferred trigger from the config ("F7" by
/// default); the portal treats it as a suggestion the user can override
/// in KDE's shortcut settings. `extra` is (id, trigger) for
/// every dynamically-bound action (chat macros, resource shortcuts, panel
/// toggles); changing the set triggers one KDE re-approval.
///
/// Never returns `Ok` while hotkeys work: any return is the hotkeys being
/// dead, and the error says why.
pub async fn listen(
    tx: std::sync::mpsc::Sender<Hotkey>,
    price_check: String,
    extra: Vec<(String, String)>,
) -> anyhow::Result<()> {
    listen_with(tx, HotkeyBindings { price_check, extra }, HotkeyRebind::new()).await
}

/// Why one portal session stopped being listened to.
enum SessionEnd {
    Rebind(HotkeyBindings),
    /// xdg-desktop-portal was restarted: its replacement knows nothing of
    /// our session, and without a new one every hotkey is silently dead.
    PortalRestarted,
}

/// `listen`, plus new bindings at runtime through `rebind` (a settings
/// change). A portal session binds its shortcuts once, so each new set, and
/// each restart of the portal service, gets a fresh session.
pub async fn listen_with(
    tx: std::sync::mpsc::Sender<Hotkey>,
    mut bindings: HotkeyBindings,
    rebind: HotkeyRebind,
) -> anyhow::Result<()> {
    loop {
        match run_session(&tx, &bindings, &rebind).await? {
            SessionEnd::Rebind(b) => bindings = b,
            SessionEnd::PortalRestarted => {
                eprintln!("hotkeys: desktop portal restarted, binding again");
            }
        }
    }
}

async fn run_session(
    tx: &std::sync::mpsc::Sender<Hotkey>,
    bindings: &HotkeyBindings,
    rebind: &HotkeyRebind,
) -> anyhow::Result<SessionEnd> {
    let gs = GlobalShortcuts::new().await?;
    let session = gs.create_session().await?;
    // An empty trigger is an action the caller unbound (a conflict loser,
    // or a hotkey cleared in Settings): it is not registered at all.
    let mut shortcuts = Vec::new();
    if !bindings.price_check.is_empty() {
        shortcuts.push(
            NewShortcut::new("price-check", "khaloni-poe2: price check hovered item")
                .preferred_trigger(bindings.price_check.as_str()),
        );
    }
    for (id, trigger) in bindings.extra.iter().filter(|(_, t)| !t.is_empty()) {
        shortcuts.push(
            NewShortcut::new(id.as_str(), "khaloni-poe2: action").preferred_trigger(trigger.as_str()),
        );
    }
    gs.bind_shortcuts(&session, &shortcuts, None).await?.response()?;
    let mut activated = gs.receive_activated().await?;

    // The activation stream does not end when the portal service goes
    // away; it just goes quiet. The service's bus name changing hands is
    // the signal that this session died with it.
    let bus = zbus::Connection::session().await?;
    let dbus = zbus::fdo::DBusProxy::new(&bus).await?;
    let mut owner_changes = dbus
        .receive_name_owner_changed_with_args(&[(0, "org.freedesktop.portal.Desktop")])
        .await?;

    loop {
        tokio::select! {
            a = activated.next() => {
                let Some(a) = a else {
                    anyhow::bail!("the desktop portal closed the shortcut activation stream");
                };
                let hk = match a.shortcut_id() {
                    "price-check" => Hotkey::PriceCheck,
                    id => Hotkey::Extra(id.to_string()),
                };
                let _ = tx.send(hk);
            }
            b = rebind.next() => {
                // Closing releases the old set before the new one binds.
                let _ = session.close().await;
                return Ok(SessionEnd::Rebind(b));
            }
            change = owner_changes.next() => {
                let Some(change) = change else {
                    anyhow::bail!("lost the session bus while listening for hotkeys");
                };
                // The name being released is the old service exiting; the
                // new owner is the moment a session can be made again.
                if change.args().is_ok_and(|a| a.new_owner().is_some()) {
                    return Ok(SessionEnd::PortalRestarted);
                }
            }
        }
    }
}
