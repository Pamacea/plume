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
mod settings;
mod shortcut;
mod spell;
mod theme;
mod tray;
mod update;

use std::sync::Arc;
use std::sync::mpsc;

fn main() -> eframe::Result {
    // sans console en release, une panique serait muette : elle finit dans plume.log
    std::panic::set_hook(Box::new(|info| dlog::dlog(&format!("PANIC: {info}"))));
    let cfg = Arc::new(config::Config::load());
    // le thread du hook enregistre le hotkey de pause dès son démarrage :
    // les réglages doivent être publiés avant lui
    settings::init(&settings::load());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || hook::run(tx));

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            // la bulle ne doit JAMAIS prendre l'avant-plan : avec active(false),
            // winit la révèle en SW_SHOWNOACTIVATE (sinon SW_SHOW, qui active la
            // fenêtre au nez de l'application où l'utilisateur est en train de taper)
            .with_active(false)
            // pas de bouton dans la barre des tâches : eframe remontre la fenêtre
            // après le premier rendu, App::ui la remasque (voir `hidden_sent`)
            .with_taskbar(false)
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
