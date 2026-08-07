//! "Run with the game": the copyable Steam launch-options line.
//!
//! The overlay's `--launch` mode wraps the game command Steam expands from
//! `%command%`: the overlay starts first, spawns the game as its child,
//! and closes itself the moment the game process exits. This section
//! exists so nobody has to construct that line by hand.

use eframe::egui;

pub fn section_launch(ui: &mut egui::Ui) {
    ui.heading("Run with the game");
    ui.add_space(6.0);
    ui.label(
        "Steam can start the overlay together with Path of Exile 2 and close it \
         the moment the game exits.",
    );
    ui.add_space(4.0);
    ui.label(
        "In Steam: right-click Path of Exile 2 → Properties → General → Launch \
         Options, and paste this line:",
    );
    ui.add_space(8.0);
    let line = launch_line();
    ui.horizontal(|ui| {
        ui.code(&line);
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(line.clone());
        }
    });
    ui.add_space(8.0);
    ui.label(
        "Starting the overlay by hand still works exactly as before; if one is \
         already running when the game starts, the launcher leaves it alone.",
    );
}

/// The exact launch-options line for this installed binary. Quoted so a
/// path with spaces survives Steam's shell.
fn launch_line() -> String {
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "khaloni-poe2".to_string());
    format!("\"{exe}\" --launch %command%")
}
