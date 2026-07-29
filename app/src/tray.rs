//! System tray icon: StatusNotifierItem over D-Bus (ksni) on Linux, a
//! notification-area icon (tray-icon) elsewhere. Same TrayEvent contract
//! and menu on both.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    OpenSettings,
    ToggleOverlay,
    TogglePause,
    Quit,
}

#[cfg(target_os = "linux")]
pub fn spawn(tx: std::sync::mpsc::Sender<TrayEvent>) -> anyhow::Result<()> {
    linux::spawn(tx)
}

#[cfg(not(target_os = "linux"))]
pub fn spawn(tx: std::sync::mpsc::Sender<TrayEvent>) -> anyhow::Result<()> {
    win::spawn(tx)
}

// Named `win` (not `windows`) so `use windows::Win32::...` inside keeps
// resolving to the extern crate without ambiguity.
#[cfg(not(target_os = "linux"))]
mod win {
    use std::sync::mpsc::Sender;
    use std::time::Duration;

    use anyhow::{anyhow, Context};
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    use super::TrayEvent;

    // Same embedded exalted-orb PNG the ksni half ships; tray-icon wants
    // plain RGBA, no channel shuffle needed.
    static ICON_PNG: &[u8] = include_bytes!("../assets/icons/exalted.png");

    // Menu ids double as the routing keys in drain_events.
    const ID_SETTINGS: &str = "open-settings";
    const ID_OVERLAY: &str = "toggle-overlay";
    const ID_PAUSE: &str = "pause-pricing";
    const ID_QUIT: &str = "quit";

    pub fn spawn(tx: Sender<TrayEvent>) -> anyhow::Result<()> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();

        // tray-icon on Windows hangs its hidden window off the creating
        // thread's message queue, so the icon must be built and pumped on
        // one dedicated thread (the win32 analogue of the ksni runtime
        // thread on the Linux side).
        std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || match build_tray() {
                Ok(tray) => {
                    let _ = ready_tx.send(Ok(()));
                    // Dropping the TrayIcon removes the icon; keep it alive
                    // for the pump's (= process') lifetime.
                    let _tray = tray;
                    pump(&tx);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .context("tray: failed to spawn thread")?;

        // Creation is a few Win32 calls; the timeout only guards against a
        // wedged thread so startup can't hang on it.
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(result) => result,
            Err(_) => Err(anyhow!("tray: timed out waiting for icon creation")),
        }
    }

    fn build_tray() -> anyhow::Result<tray_icon::TrayIcon> {
        let img = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
            .context("tray: bad embedded icon PNG")?
            .into_rgba8();
        let (width, height) = img.dimensions();
        let icon = Icon::from_rgba(img.into_vec(), width, height)
            .context("tray: icon rejected by tray-icon")?;

        // Menu mirrors the ksni half. The check item's visual state is
        // muda's (it auto-toggles on click); main owns the real pause flag
        // and flips it on TogglePause, same split as on Linux.
        let menu = Menu::with_items(&[
            &MenuItem::with_id(ID_SETTINGS, "Open Settings", true, None),
            &MenuItem::with_id(ID_OVERLAY, "Toggle Overlay", true, None),
            &CheckMenuItem::with_id(ID_PAUSE, "Pause Pricing", true, false, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id(ID_QUIT, "Quit", true, None),
        ])
        .context("tray: menu build failed")?;

        TrayIconBuilder::new()
            .with_tooltip("poe2-lens")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            // Left click opens settings (ksni's activate); menu stays on
            // right click only, so keep the crate from also popping it.
            .with_menu_on_left_click(false)
            .build()
            .context("tray: icon creation failed")
    }

    // Classic message pump. Menu/tray WndProcs push into the crates'
    // channels during DispatchMessageW, so draining right after each
    // dispatched message never lags and never needs a busy-wait.
    fn pump(tx: &Sender<TrayEvent>) {
        let mut msg = MSG::default();
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                drain_events(tx);
            }
        }
    }

    fn drain_events(tx: &Sender<TrayEvent>) {
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let event = match ev.id().as_ref() {
                ID_SETTINGS => TrayEvent::OpenSettings,
                ID_OVERLAY => TrayEvent::ToggleOverlay,
                ID_PAUSE => TrayEvent::TogglePause,
                ID_QUIT => TrayEvent::Quit,
                _ => continue,
            };
            let _ = tx.send(event);
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            // Left click on the icon = ksni's activate. Fire on release so
            // a click reads as one event, not one per button transition.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                let _ = tx.send(TrayEvent::OpenSettings);
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::mpsc::Sender;
    use std::sync::OnceLock;
    use std::time::Duration;

    use anyhow::{anyhow, Context};
    // ksni 0.3 is async-first; the blocking API is behind a non-default
    // feature, so we drive the async `spawn` on our own runtime thread.
    use ksni::TrayMethods;

    use super::TrayEvent;

    // Fallback pixmap for hosts without a "poe2-lens" hicolor icon installed
    // (Task 9 ships the theme icon). Exalted orb: gold, matches the palette.
    static ICON_PNG: &[u8] = include_bytes!("../assets/icons/exalted.png");

    fn icon_pixmap() -> Vec<ksni::Icon> {
        static ICON: OnceLock<Option<ksni::Icon>> = OnceLock::new();
        ICON.get_or_init(|| {
            let img = image::load_from_memory_with_format(ICON_PNG, image::ImageFormat::Png)
                .ok()?
                .into_rgba8();
            let (width, height) = img.dimensions();
            let mut data = img.into_vec();
            // SNI wants ARGB32 in network byte order; image gives RGBA.
            for px in data.chunks_exact_mut(4) {
                px.rotate_right(1);
            }
            Some(ksni::Icon {
                width: width as i32,
                height: height as i32,
                data,
            })
        })
        .iter()
        .cloned()
        .collect()
    }

    struct Tray {
        tx: Sender<TrayEvent>,
        // Mirrors main's pipeline_paused only for the checkmark; main owns
        // the real state and flips it on TogglePause.
        paused: bool,
    }

    impl ksni::Tray for Tray {
        fn id(&self) -> String {
            "poe2-lens".into()
        }

        fn title(&self) -> String {
            "poe2-lens".into()
        }

        fn icon_name(&self) -> String {
            "poe2-lens".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            icon_pixmap()
        }

        // Left click on the icon.
        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.tx.send(TrayEvent::OpenSettings);
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::{CheckmarkItem, StandardItem};
            vec![
                StandardItem {
                    label: "Open Settings".into(),
                    activate: Box::new(|t: &mut Self| {
                        let _ = t.tx.send(TrayEvent::OpenSettings);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: "Toggle Overlay".into(),
                    activate: Box::new(|t: &mut Self| {
                        let _ = t.tx.send(TrayEvent::ToggleOverlay);
                    }),
                    ..Default::default()
                }
                .into(),
                CheckmarkItem {
                    label: "Pause Pricing".into(),
                    checked: self.paused,
                    activate: Box::new(|t: &mut Self| {
                        t.paused = !t.paused;
                        let _ = t.tx.send(TrayEvent::TogglePause);
                    }),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(|t: &mut Self| {
                        let _ = t.tx.send(TrayEvent::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    pub fn spawn(tx: Sender<TrayEvent>) -> anyhow::Result<()> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();

        std::thread::Builder::new()
            .name("tray".into())
            .spawn(move || {
                // Dedicated current-thread runtime: ksni's service loop is
                // tokio::spawn'ed, so this thread must keep driving it and
                // must not depend on the app's runtime existing.
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .context("tray: failed to build tokio runtime")
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                rt.block_on(async move {
                    let tray = Tray { tx, paused: false };
                    match tray.spawn().await {
                        Ok(handle) => {
                            let _ = ready_tx.send(Ok(()));
                            // Dropping the Handle closes the service channel;
                            // keep it (and the runtime) alive forever.
                            let _handle = handle;
                            std::future::pending::<()>().await;
                        }
                        Err(e) => {
                            let _ = ready_tx.send(Err(anyhow::Error::new(e)
                                .context("tray: StatusNotifierItem registration failed")));
                        }
                    }
                });
            })
            .context("tray: failed to spawn thread")?;

        // Registration is a couple of D-Bus round-trips; the timeout only
        // guards against a wedged session bus so startup can't hang on it.
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(result) => result,
            Err(_) => Err(anyhow!("tray: timed out waiting for D-Bus registration")),
        }
    }
}
