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

    // La bulle ne doit jamais voler le focus : sans ça, l'application cible perdrait
    // son curseur dès l'apparition de la popup.
    std::thread::spawn(no_activate::worker);

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

mod no_activate {
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW,
    };
    use windows::core::w;

    pub fn worker() {
        // la fenêtre n'existe qu'après le démarrage d'eframe : on attend
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            unsafe {
                let Ok(hwnd) = FindWindowW(None, w!("plume")) else {
                    continue;
                };
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                let _ = SetWindowLongPtrW(
                    hwnd,
                    GWL_EXSTYLE,
                    ex | (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize,
                );
                return;
            }
        }
    }
}
