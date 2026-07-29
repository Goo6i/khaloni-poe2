//! System tray icon. Linux-only for now (StatusNotifierItem over D-Bus via
//! ksni); the Windows tray (tray-icon crate) arrives in SP3.

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

/// Windows tray (tray-icon crate) arrives in SP3; until then the app simply
/// runs without a tray on non-Linux targets.
#[cfg(not(target_os = "linux"))]
pub fn spawn(_tx: std::sync::mpsc::Sender<TrayEvent>) -> anyhow::Result<()> {
    Ok(())
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
