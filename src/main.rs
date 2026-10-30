// app tray sans UI : aucune console en release (la fermer tuerait l'exé) ;
// en debug la console reste pour `cargo run` et les println des tests
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod app;
mod config;
mod conj;
mod def;
mod dlog;
mod hook;
mod punct;
mod shortcut;
mod spell;
mod tray;
mod update;

use std::sync::Arc;
use std::sync::mpsc;

fn main() -> eframe::Result {
    let cfg = Arc::new(config::Config::load());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || hook::run(tx));

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_position(eframe::egui::pos2(-4000.0, -4000.0))
            // taille initiale ≈ bulle : le buffer affiché avant le premier InnerSize ne flash pas
            .with_inner_size(eframe::egui::vec2(300.0, 150.0)),
        ..Default::default()
    };
    // raccourcis Démarrer + Bureau (réécrits : suit un déplacement du dossier)
    shortcut::install();
    // tray à créer sur le thread principal, avant la boucle winit (voir tray.rs)
    tray::init();
    eframe::run_native(
        "plume",
        options,
        Box::new(move |_cc| Ok(Box::new(app::App::new(rx, cfg)))),
    )
}
