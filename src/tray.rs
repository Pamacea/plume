//! Icône de notification : pause, démarrage auto, mise à jour, quitter.
//!
//! Créée sur le thread principal AVANT `eframe::run_native` : tray-icon repose sur
//! la boucle de messages Windows, que seule winit (thread principal) vide.
//! L'état tient dans un `thread_local!` (muda est !Send) pour ne pas étoffer `App`.

use std::sync::atomic::{AtomicBool, Ordering};

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{CopyIcon, IDI_APPLICATION, LoadIconW};
use windows::core::PCWSTR;

use crate::hook;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "plume";

struct State {
    // doit rester vivante : la destruction de l'icône retire le tray
    _tray: TrayIcon,
    pause: CheckMenuItem,
    force: CheckMenuItem,
    autostart: CheckMenuItem,
}

thread_local! {
    // muda/tray-icon sont !Send (!Sync) : l'état vit sur le seul thread principal,
    // qui crée le tray et vide ses événements — pas de static possible
    static STATE: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}
/// dernier état affiché par la coche « Pause » (synchro hotkey ⇄ menu tray)
static SHOWN_PAUSED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    // ids explicites : sans eux muda génère des ids numériques ("1","2"…) et
    // les matchs de `poll` ne répondent jamais
    let pause = CheckMenuItem::with_id("pause", "Pause", true, hook::is_paused(), None);
    let force = CheckMenuItem::with_id("force", "Mode forcé", true, hook::is_force(), None);
    let autostart = CheckMenuItem::with_id(
        "autostart",
        "Démarrage auto",
        true,
        autostart_enabled(),
        None,
    );
    let update = MenuItem::with_id("update", "Mettre à jour", true, None);
    let quit = MenuItem::with_id("quit", "Quitter", true, None);
    let menu = Menu::new();
    let _ = menu.append(&pause);
    let _ = menu.append(&force);
    let _ = menu.append(&autostart);
    let _ = menu.append(&update);
    let _ = menu.append(&quit);
    // icône embarquée dans l'exé par build.rs (winresource, ressource ID 1),
    // repli sur l'icône système si absente. CopyIcon car tray-icon détruit le
    // handle au drop et LoadIconW renvoie une icône partagée.
    let icon = unsafe {
        let own = GetModuleHandleW(None).ok().and_then(|m| {
            // MAKEINTRESOURCE(1) : ressource ID 1 de l'exé (winresource)
            let id1 = PCWSTR(std::ptr::without_provenance(1));
            LoadIconW(Some(HINSTANCE(m.0)), id1).ok()
        });
        let shared = own
            .or_else(|| LoadIconW(None, IDI_APPLICATION).ok())
            .expect("LoadIconW");
        crate::dlog::dlog(if own.is_some() {
            "icône embarquée de l'exé"
        } else {
            "icône système (ressource 1 absente)"
        });
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
            force,
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
        // la coche suit aussi les bascules faites par la hotkey Ctrl+Alt+P
        let paused = hook::is_paused();
        if paused != SHOWN_PAUSED.swap(paused, Ordering::Relaxed) {
            s.pause.set_checked(paused);
        }
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let id: &str = ev.id().as_ref();
            match id {
                "pause" => {
                    // muda a déjà basculé la coche avant d'émettre l'événement
                    hook::toggle_paused();
                    s.pause.set_checked(hook::is_paused());
                }
                "force" => {
                    // muda a déjà basculé la coche avant d'émettre l'événement
                    hook::set_force(s.force.is_checked());
                }
                "autostart" => {
                    // muda a déjà basculé la coche avant d'émettre l'événement
                    if !set_autostart(s.autostart.is_checked()) {
                        crate::dlog::dlog("démarrage auto : registre inaccessible");
                        s.autostart.set_checked(!s.autostart.is_checked());
                    }
                }
                "update" => crate::update::launch(),
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
