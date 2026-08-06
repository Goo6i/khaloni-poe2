//! The Updates section of the settings window.
//!
//! Checking and installing both run on background threads and report back
//! through a channel, so a slow network never freezes the window. Install
//! is always an explicit click — see the module docs in `update`.

use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;

use crate::config::Config;
use crate::update::{self, Update};

#[derive(Debug, Clone)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Found(Update),
    Installing,
    /// Installed; takes effect on next launch.
    Installed(String),
    Failed(String),
}

pub struct UpdateUi {
    pub state: State,
    tx: Sender<State>,
    rx: Receiver<State>,
}

impl Default for UpdateUi {
    fn default() -> UpdateUi {
        let (tx, rx) = channel();
        UpdateUi { state: State::Idle, tx, rx }
    }
}

impl UpdateUi {
    /// Drains worker results; call once per frame before drawing.
    pub fn poll(&mut self) {
        while let Ok(s) = self.rx.try_recv() {
            self.state = s;
        }
    }

    fn check(&mut self) {
        self.state = State::Checking;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(match update::check() {
                Ok(Some(u)) => State::Found(u),
                Ok(None) => State::UpToDate,
                Err(e) => State::Failed(e.to_string()),
            });
        });
    }

    fn install(&mut self, u: Update) {
        self.state = State::Installing;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(match update::apply(&u) {
                Ok(_) => State::Installed(u.version),
                Err(e) => State::Failed(e.to_string()),
            });
        });
    }
}

pub fn section_updates(ui: &mut egui::Ui, cfg: &mut Config, ui_state: &mut UpdateUi) {
    ui.heading("Updates");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Installed version");
        ui.monospace(update::CURRENT);
    });
    ui.add_space(6.0);
    ui.checkbox(&mut cfg.check_updates, "Check for updates on startup");
    ui.label(
        egui::RichText::new(
            "Checks this project's GitHub releases. Updates are downloaded \
             only when you ask, verified against the release checksum, and \
             take effect the next time you start the app — never mid-game.",
        )
        .weak(),
    );

    ui.add_space(10.0);
    // A cargo build must never be replaced by a release binary.
    if update::is_dev_build() {
        ui.colored_label(
            egui::Color32::from_rgb(0xB7, 0xAB, 0x97),
            "Running from a build directory — self-update is disabled here.",
        );
        return;
    }

    let busy = matches!(ui_state.state, State::Checking | State::Installing);
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy, egui::Button::new("Check now")).clicked() {
            ui_state.check();
        }
        if let State::Found(u) = ui_state.state.clone() {
            if ui.add_enabled(!busy, egui::Button::new(format!("Install {}", u.version))).clicked() {
                ui_state.install(u);
            }
        }
    });

    ui.add_space(6.0);
    match &ui_state.state {
        State::Idle => {}
        State::Checking => {
            ui.label("Checking…");
        }
        State::UpToDate => {
            ui.label("Up to date.");
        }
        State::Found(u) => {
            ui.label(format!("{} is available.", u.version));
            ui.hyperlink_to("Release notes", &u.notes_url);
        }
        State::Installing => {
            ui.label("Downloading and verifying…");
        }
        State::Installed(v) => {
            ui.colored_label(
                egui::Color32::from_rgb(0xC9, 0xA2, 0x27),
                format!("{v} installed — quit from the tray and start it again to use it."),
            );
        }
        State::Failed(e) => {
            ui.colored_label(egui::Color32::RED, format!("Update failed: {e}"));
        }
    }
}
