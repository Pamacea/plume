//! Icône de notification : pause, démarrage auto, quitter.
//!
//! Créée sur le thread principal AVANT `eframe::run_native` : tray-icon repose sur
//! la boucle de messages Windows, que seule winit (thread principal) vide.
//! L'état tient dans un `thread_local!` (muda est !Send) pour ne pas étoffer `App`.

use std::sync::atomic::{AtomicBool, Ordering};

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use windows::Win32::UI::WindowsAndMessaging::{CopyIcon, IDI_APPLICATION, LoadIconW};

use crate::hook;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "plume";

struct State {
    // doit rester vivante : la destruction de l'icône retire le tray
    _tray: TrayIcon,
    pause: MenuItem,
    autostart: CheckMenuItem,
}

thread_local! {
    // muda/tray-icon sont !Send (!Sync) : l'état vit sur le seul thread principal,
    // qui crée le tray et vide ses événements — pas de static possible
    static STATE: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}
/// dernier état affiché dans le libellé « pause » (synchro hotkey ⇄ menu tray)
static SHOWN_PAUSED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    let pause = MenuItem::new("Mettre en pause", true, None);
    let autostart = CheckMenuItem::new("Démarrage auto", true, autostart_enabled(), None);
    let quit = MenuItem::new("Quitter", true, None);
    let menu = Menu::new();
    let _ = menu.append(&pause);
    let _ = menu.append(&autostart);
    let _ = menu.append(&quit);
    // icône système par défaut : pas de fichier .ico à embarquer.
    // CopyIcon car tray-icon détruit le handle au drop et l'icône LoadIconW est partagée.
    let icon = unsafe {
        let shared = LoadIconW(None, IDI_APPLICATION).expect("LoadIconW");
        Icon::from_handle(CopyIcon(shared).expect("CopyIcon").0 as isize)
    };
    let tray = TrayIconBuilder::new()
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_tooltip("plume")
        .build()
        .expect("création de l'icône de tray");
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            _tray: tray,
            pause,
            autostart,
        });
    });
    crate::dlog::dlog("icône de tray installée");
}

/// À appeler à chaque frame depuis `App::ui` : winit ne délivre pas les événements
/// de menu tout seul, ils arrivent par un canal qu'il faut vider.
pub fn poll() {
    STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        let Some(s) = slot.as_mut() else {
            return;
        };
        // le libellé suit aussi les bascules faites par la hotkey Ctrl+Alt+P
        let paused = hook::is_paused();
        if paused != SHOWN_PAUSED.swap(paused, Ordering::Relaxed) {
            s.pause.set_text(if paused {
                "Reprendre"
            } else {
                "Mettre en pause"
            });
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let id: &str = ev.id().as_ref();
            match id {
                "pause" => {
                    hook::toggle_paused();
                }
                "autostart" => {
                    // muda a déjà basculé la coche avant d'émettre l'événement
                    if !set_autostart(s.autostart.is_checked()) {
                        crate::dlog::dlog("démarrage auto : registre inaccessible");
                        s.autostart.set_checked(!s.autostart.is_checked());
                    }
                }
                "quit" => std::process::exit(0),
                _ => {}
            }
        }
    });
}

fn autostart_enabled() -> bool {
    winreg::HKCU
        .open_subkey(RUN_KEY)
        .and_then(|k| k.get_value::<String, _>(VALUE_NAME))
        .is_ok()
}

/// Écrit/supprime la valeur `plume` dans `HKCU\...\Run` ; true = l'état voulu est en place.
fn set_autostart(on: bool) -> bool {
    if on {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        // guillemets : sans eux, un chemin avec espaces est mal parsé par Run
        let cmd = format!("\"{}\"", exe.display());
        if winreg::HKCU
            .create_subkey(RUN_KEY)
            .and_then(|(k, _)| k.set_value(VALUE_NAME, &cmd))
            .is_err()
        {
            return false;
        }
    } else if let Ok(k) = winreg::HKCU.open_subkey(RUN_KEY) {
        let _ = k.delete_value(VALUE_NAME); // déjà absent : rien à supprimer
    }
    // revérification : la coche du menu doit refléter la réalité du registre
    autostart_enabled() == on
}
