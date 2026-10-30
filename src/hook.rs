//! Hook clavier global (WH_KEYBOARD_LL) : reconstitue les caractères tapés
//! via ToUnicodeEx et avale ↑↓/Entrée/Échap quand la bulle est ouverte.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use eframe::egui;
use windows::Win32::Foundation::{CloseHandle, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetKeyboardLayout, INPUT, INPUT_0, INPUT_KEYBOARD,
    KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOD_ALT, MOD_CONTROL,
    MOD_NOREPEAT, RegisterHotKey, SendInput, ToUnicodeEx, UnregisterHotKey, VIRTUAL_KEY, VK_BACK,
    VK_CAPITAL, VK_CONTROL, VK_DOWN, VK_END, VK_ESCAPE, VK_F11, VK_F12, VK_F24, VK_HOME,
    VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_RCONTROL,
    VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
    KBDLLHOOKSTRUCT, KillTimer, MSG, MSLLHOOKSTRUCT, SetTimer, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_HOTKEY, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_SYSKEYDOWN, WM_TIMER, WindowFromPoint,
};
use windows::core::PWSTR;

#[derive(Debug, Clone, Copy)]
pub enum KeyMsg {
    Char(char),
    Backspace,
    Up,
    Down,
    Return,
    Escape,
    /// une autre touche (flèches latérales, tab…) : ferme la bulle sans remplacer
    Other,
    /// raccourci clavier (Ctrl+V…) : le presse-papiers a désynchronisé le buffer
    Invalidate,
    /// définition prête (payload dans `def::DEF`) : ouvre la bulle de définition
    Def,
    /// clic gauche enfoncé : ferme la bulle de définition (et rien d'autre)
    Click,
}

static TX: OnceLock<Sender<KeyMsg>> = OnceLock::new();
/// contexte egui à réveiller : le hook doit réveiller le rendu pour réveiller
/// l'app à chaque frappe utile (sinon elle attendrait le filet de 250 ms)
static REPINT: OnceLock<egui::Context> = OnceLock::new();
static POPUP_OPEN: AtomicBool = AtomicBool::new(false);
/// hwnd de l'app devant laquelle la bulle s'est ouverte (0 = aucune) : dès que
/// l'utilisateur passe à une autre app, la suggestion est périmée — sans ça,
/// une bulle laissée ouverte + mode forcé = clavier bloqué PARTOUT
static POPUP_HWND: AtomicIsize = AtomicIsize::new(0);
/// instant (ms epoch) d'ouverture de la suggestion : la fenêtre n'est à
/// l'écran qu'à la frame suivante, un clic reçu avant ne doit pas la fermer
static POPUP_SINCE: AtomicU64 = AtomicU64::new(0);
static INJECT_TEST: AtomicBool = AtomicBool::new(false);
/// pause globale : en true, hook_proc laisse passer toutes les frappes sans traiter
static PAUSED: AtomicBool = AtomicBool::new(false);
/// pédale F24 : un appui armé (true) attend un éventuel 2e appui pendant 250 ms.
/// Aucune serrure : le hook et la boucle de messages vivent sur le MÊME thread —
/// pas de mutex, donc pas de deadlock possible dans le callback.
static F24_PENDING: AtomicBool = AtomicBool::new(false);
/// id du timer WM_TIMER qui déclenche F12 après la fenêtre (0 = aucun)
static F24_TIMER: AtomicUsize = AtomicUsize::new(0);
/// marque nos propres injections SendInput (`dwExtraInfo`) : le hook les ignore.
/// Indispensable depuis la redirection Alt+Tab → Alt+Esc : sans elle, notre Esc
/// injecté serait classé Escape et avalé par la règle « bulle ouverte ».
pub(crate) const MAGIC: usize = 0x504C_554D; // « PLUM »
/// dernière touche morte vue (vk, scan) en attente de composition : l'état de
/// ToUnicodeEx est PARTAGÉ avec l'app (constaté : son rendu suit nos appels) →
/// on le réarme après chaque composition pour qu'elle compose aussi
static DEAD: Mutex<Option<(u32, u32)>> = Mutex::new(None);
/// noms de fichiers (minuscules) des apps exclues, chargés une fois dans `run`
static EXCLUDED: OnceLock<Vec<String>> = OnceLock::new();

/// identifiant du hotkey Ctrl+Alt+P (RegisterHotKey / WM_HOTKEY)
const HOTKEY_PAUSE: i32 = 1;

pub fn set_popup_open(v: bool) {
    POPUP_OPEN.store(v, Ordering::Relaxed);
    // mémorise la fenêtre au moment de l'ouverture : la bulle n'est valable
    // que devant cette app (voir hook_proc)
    let hwnd = if v {
        unsafe { GetForegroundWindow().0 as isize }
    } else {
        0
    };
    POPUP_HWND.store(hwnd, Ordering::Relaxed);
    POPUP_SINCE.store(if v { now_ms() } else { 0 }, Ordering::Relaxed);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// La bulle a-t-elle eu le temps d'apparaître ? `Visible(true)` n'est appliqué
/// qu'après la frame en cours.
/// shortcut: marge fixe 400 ms ; un clic raté est rattrapé à la frappe suivante
fn popup_drawn() -> bool {
    now_ms().saturating_sub(POPUP_SINCE.load(Ordering::Relaxed)) > 400
}

pub fn set_paused(v: bool) {
    PAUSED.store(v, Ordering::Relaxed);
}

/// Enregistré une seule fois au premier pass `App::ui` (OnceLock).
pub fn set_repaint_ctx(ctx: egui::Context) {
    REPINT.set(ctx).ok();
}

/// Réveille le rendu — uniquement quand un message part vraiment :
/// frappes ignorées et pause ne réveillent rien.
fn repint() {
    if let Some(c) = REPINT.get() {
        c.request_repaint();
    }
}

pub fn is_paused() -> bool {
    PAUSED.load(Ordering::Relaxed)
}

/// Envoie un message depuis un worker (définition) et réveille le rendu.
pub(crate) fn notify(msg: KeyMsg) {
    if let Some(tx) = TX.get()
        && tx.send(msg).is_ok()
    {
        repint();
    }
}

/// Bascule la pause (hotkey Ctrl+Alt+P et menu tray) et referme la bulle ouverte :
/// sans Invalidate, une bulle resterait affichée pendant que le hook ne la pilote plus.
pub fn toggle_paused() -> bool {
    let v = !is_paused();
    set_paused(v);
    crate::dlog::dlog(if v { "PAUSE activée" } else { "PAUSE levée" });
    if let Some(tx) = TX.get()
        && tx.send(KeyMsg::Invalidate).is_ok()
    {
        repint();
    }
    v
}

pub fn run(tx: Sender<KeyMsg>) {
    TX.set(tx).ok();
    EXCLUDED.set(load_exclusions()).ok();
    // shortcut: PLUME_INJECT_TEST accepte les frappes synthétisées (SendInput) pour test
    // automatisé — jamais actif en fonctionnement normal, ça casserait la boucle anti-retrigger
    INJECT_TEST.store(
        std::env::var("PLUME_INJECT_TEST").is_ok(),
        Ordering::Relaxed,
    );
    unsafe {
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), None, 0)
            .expect("SetWindowsHookExW a échoué");
        crate::dlog::dlog("hook installé");
        // souris sur le même thread : les deux hooks partagent la boucle
        // GetMessageW ci-dessous (obligatoire pour les hooks low-level)
        let hookm = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0)
            .expect("SetWindowsHookExW (souris) a échoué");
        crate::dlog::dlog("hook souris installé");
        // Ctrl+Alt+P : hwnd = None → WM_HOTKEY est posté dans la file de CE thread,
        // donc reçu par la boucle GetMessageW ci-dessous (pas de fenêtre dédiée).
        if let Err(e) = RegisterHotKey(
            None,
            HOTKEY_PAUSE,
            MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
            u32::from(b'P'),
        ) {
            crate::dlog::dlog(&format!("hotkey Ctrl+Alt+P non enregistrée : {e}"));
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if msg.message == WM_HOTKEY && msg.wParam.0 == HOTKEY_PAUSE as usize {
                toggle_paused();
                continue;
            }
            // fin de la fenêtre F24 sans 2e appui → appui simple : F12
            let timer = F24_TIMER.load(Ordering::Relaxed);
            if msg.message == WM_TIMER && timer != 0 && msg.wParam.0 == timer {
                F24_TIMER.store(0, Ordering::Relaxed);
                let _ = KillTimer(None, timer);
                if F24_PENDING.swap(false, Ordering::Relaxed) {
                    inject_key(VK_F12);
                }
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnregisterHotKey(None, HOTKEY_PAUSE);
        let _ = UnhookWindowsHookEx(hookm);
        let _ = UnhookWindowsHookEx(hook);
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // un panic ne doit jamais sortir d'un `extern "system"` (abort du process
    // depuis Rust 1.81) ni geler la boucle de messages Windows
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        hook_impl(code, wparam, lparam)
    })) {
        Ok(r) => r,
        Err(_) => {
            crate::dlog::dlog("PANIC dans le hook clavier — touche laissée passer");
            unsafe { CallNextHookEx(None, code, wparam, lparam) }
        }
    }
}

unsafe fn hook_impl(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code < 0 {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        // toute frappe (même en pause) invalide une recherche de définition en
        // vol — placé AVANT le test PAUSED : une annulation doit toujours passer
        crate::def::bump();
        // en pause : aucune frappe traitée, aucun swallow — le hook fait juste transiter
        if PAUSED.load(Ordering::Relaxed) {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        // seuls les enfoncements portent une décision ; les relâchements passent
        if wparam.0 != WM_KEYDOWN as usize && wparam.0 != WM_SYSKEYDOWN as usize {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        // nos propres injections (backspaces, unicode, Esc/F11/F12) — sans ça
        // l'Esc d'Alt+Tab serait avalé par notre propre règle « bulle ouverte »
        if injected_ours(kb) {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        // app exclue : aucune frappe traitée, aucun remappage
        if is_excluded() {
            crate::dlog::dlog("exclu (app dans exclusions.txt) — frappe ignorée");
            return CallNextHookEx(None, code, wparam, lparam);
        }
        // CHANTIER 1 : état physique des modificateurs AVANT tout traitement métier
        let m = mods();

        // CHANTIER 2 — pédale F24 : double appui ≤ 250 ms → F11, appui simple → F12
        if kb.vkCode == VK_F24.0 as u32 {
            f24_tap();
            return LRESULT(1);
        }

        // CHANTIER 2 — Alt GAUCHE + touche mappée → caractère spécial
        if m.alt && !m.ctrl && !m.win {
            // diagnostic : ce que Windows remonte réellement sous Alt
            crate::dlog::dlog(&format!(
                "alt vk={:#x} sc={:#x} shift={} caps={}",
                kb.vkCode, kb.scanCode, m.shift, m.caps
            ));
            if let Some(c) =
                key_scan(kb.scanCode, kb.vkCode).and_then(|k| special_char(k, m.shift, m.caps))
            {
                crate::dlog::dlog(&format!("alt -> {c:?}"));
                inject_char_under_alt(c);
                // l'injection est ignorée par le hook : le buffer suit ici
                notify(KeyMsg::Char(c));
                return LRESULT(1);
            }
        }

        // CHANTIER 1 — Ctrl/Win : pass-through absolu. `Invalidate` ne bloque
        // rien, il remet juste le buffer à zéro (presse-papiers / changement d'app).
        if m.win || m.ctrl {
            // Ctrl+Alt+P = hotkey de pause (traitée par WM_HOTKEY) : buffer intact
            if !(kb.vkCode == u32::from(b'P') && m.alt) {
                notify(KeyMsg::Invalidate);
            }
            return CallNextHookEx(None, code, wparam, lparam);
        }
        if m.alt {
            // Alt+Tab → Alt+Esc : l'Alt physique est toujours tenu, l'Esc injecté
            // suffit ; le buffer est invalidé puisqu'on change de fenêtre
            if kb.vkCode == VK_TAB.0 as u32 && !m.shift {
                crate::dlog::dlog("alt+tab -> alt+esc");
                inject_key(VK_ESCAPE);
                notify(KeyMsg::Invalidate);
                return LRESULT(1);
            }
            // tout autre Alt+X (menus, Alt+F4…) : jamais intercepté
            notify(KeyMsg::Invalidate);
            return CallNextHookEx(None, code, wparam, lparam);
        }

        if let Some(msg) = classify(kb) {
            // Shift + touche de contrôle/navigation = raccourci de l'application
            // (Shift+Enter → saut de ligne Discord, Shift+flèches → sélection) :
            // la bulle se referme, la touche n'est jamais avalée
            let msg = if m.shift && app_owns_shift(msg) {
                KeyMsg::Other
            } else {
                msg
            };
            crate::dlog::dlog(&format!("hook vk={:#x} -> {msg:?}", kb.vkCode));
            // changement d'app devant la bulle : suggestion périmée → on la ferme
            // et la frappe repasse comme bulle fermée
            let mut open = POPUP_OPEN.load(Ordering::Relaxed);
            let popup_hwnd = POPUP_HWND.load(Ordering::Relaxed);
            if open && popup_hwnd != 0 && GetForegroundWindow().0 as isize != popup_hwnd {
                crate::dlog::dlog("changement d'app — bulle invalidée");
                open = false;
                notify(KeyMsg::Invalidate);
            }
            // CHANTIER 3 : toute frappe est transmise — c'est elle qui ferme la
            // bulle, et le caractère qui la ferme ne doit jamais être avalé
            notify(msg);
            if swallow(msg, open) {
                crate::dlog::dlog("avale la touche");
                return LRESULT(1);
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }
}

/// vk des modificateurs seuls (jamais classés en message).
/// shortcut: une const avec `|` serait UNE valeur (0xFF), pas une liste —
/// d'où le match en fonction
fn is_modifier(vk: u32) -> bool {
    matches!(
        vk,
        0x10 | 0xA0 | 0xA1 | 0x11 | 0xA2 | 0xA3 | 0x12 | 0xA4 | 0xA5 | 0x14 | 0x5B | 0x5C
    )
}

/// État physique des modificateurs — lu AVANT le traitement, jamais pour la
/// touche courante (son état n'est pas encore publié pendant le hook).
#[derive(Clone, Copy)]
struct Mods {
    shift: bool,
    ctrl: bool,
    /// Alt GAUCHE seulement (RAlt = AltGr, voir `ctrl`)
    alt: bool,
    win: bool,
    caps: bool,
}

fn down(vk: VIRTUAL_KEY) -> bool {
    // 0x8000 = touche enfoncée ; le bit &1 (« pressée depuis le dernier appel »)
    // n'est PAS le verrou — CapsLock se lit avec GetKeyState
    unsafe { GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000 != 0 }
}

fn mods() -> Mods {
    // AltGr = RAlt + Ctrl synthétique : frappe normale (é, è, €…), pas un raccourci
    let altgr = down(VK_RMENU);
    Mods {
        shift: down(VK_SHIFT) || down(VK_LSHIFT) || down(VK_RSHIFT),
        ctrl: (down(VK_CONTROL) || down(VK_LCONTROL) || down(VK_RCONTROL)) && !altgr,
        // VK_MENU en plus de VK_LMENU : certains pilotes ne publient que le
        // générique ; RAlt (AltGr) exclu explicitement
        alt: (down(VK_LMENU) || down(VK_MENU)) && !altgr,
        win: down(VK_LWIN) || down(VK_RWIN),
        caps: unsafe { GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 },
    }
}

/// Touche physique normalisée en scancode : scancode s'il est connu, sinon
/// repli sur le vkCode (lettres et chiffres gardent le même vk d'un layout à
/// l'autre). `None` = touche hors table.
fn key_scan(scan: u32, vk: u32) -> Option<u32> {
    const SCANS: [u32; 11] = [
        0x03, 0x04, 0x05, 0x07, 0x0B, 0x16, 0x17, 0x18, 0x2E, 0x32, 0x34,
    ];
    if SCANS.contains(&scan) {
        return Some(scan);
    }
    Some(match vk {
        0x32 => 0x03, // VK_2
        0x33 => 0x04, // VK_3
        0x34 => 0x05, // VK_4
        0x36 => 0x07, // VK_6
        0x30 => 0x0B, // VK_0
        0x55 => 0x16, // VK_U
        0x49 => 0x17, // VK_I
        0x4F => 0x18, // VK_O
        0x43 => 0x2E, // VK_C
        0xBC => 0x32, // VK_OEM_COMMA (« , »)
        0xBA => 0x34, // VK_OEM_1 (« : »)
        _ => return None,
    })
}

/// Alt GAUCHE + touche → caractère spécial (ex-logique AHK v2), sur scancodes
/// normalisés par `key_scan`.
/// Lettres : majuscule = Shift XOR Verr Maj. `Shift` seul choisit la variante
/// ponctuation. `None` = touche non mappée (l'Alt+X passe à l'application).
fn special_char(scan: u32, shift: bool, caps: bool) -> Option<char> {
    let upper = shift ^ caps;
    Some(match scan {
        // « 2 » : É seulement avec Shift — Alt+2 seul reste un raccourci
        0x03 if shift => 'É',
        0x04 => {
            if shift {
                '‹'
            } else {
                '«'
            }
        }
        0x05 => {
            if shift {
                '›'
            } else {
                '»'
            }
        }
        0x07 => {
            if shift {
                '—'
            } else {
                '–'
            }
        }
        0x0B => 'À',
        0x32 => '‽',
        0x34 => '…',
        0x2E => {
            if upper {
                'Ç'
            } else {
                'ç'
            }
        }
        // œ/Œ sur Alt+Shift+O ; ô/Ô restent sur Alt+O
        0x18 => {
            if shift {
                if caps { 'Œ' } else { 'œ' }
            } else if caps {
                'Ô'
            } else {
                'ô'
            }
        }
        0x16 => {
            if upper {
                'Ù'
            } else {
                'ù'
            }
        }
        0x17 => {
            if upper {
                'Ï'
            } else {
                'ï'
            }
        }
        _ => return None,
    })
}

/// Décision d'un appui F24 : (reste en attente, touche à envoyer tout de suite).
/// false → premier appui, on arme la fenêtre de 250 ms (F12 si rien ne vient).
/// true  → second appui dans la fenêtre : F11 immédiat.
fn f24_next(pending: bool) -> (bool, Option<VIRTUAL_KEY>) {
    if pending {
        (false, Some(VK_F11))
    } else {
        (true, None)
    }
}

fn f24_tap() {
    let (pending, fire) = f24_next(F24_PENDING.swap(false, Ordering::Relaxed));
    F24_PENDING.store(pending, Ordering::Relaxed);
    if let Some(k) = fire {
        let t = F24_TIMER.swap(0, Ordering::Relaxed);
        unsafe {
            if t != 0 {
                let _ = KillTimer(None, t);
            }
        }
        inject_key(k);
    } else {
        // 0 = SetTimer a échoué : on désarme plutôt que laisser un état collé
        let t = unsafe { SetTimer(None, 0, 250, None) };
        if t == 0 {
            F24_PENDING.store(false, Ordering::Relaxed);
        }
        F24_TIMER.store(t, Ordering::Relaxed);
    }
}

/// Envoi groupé marqué MAGIC : le hook ignore ses propres événements.
unsafe fn send(inputs: &[INPUT]) {
    unsafe {
        if !inputs.is_empty() {
            let _ = SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
        }
    }
}

/// Événement clavier marqué MAGIC : le hook ignore ses propres injections.
fn key_event(vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: MAGIC,
            },
        },
    }
}

fn inject_key(vk: VIRTUAL_KEY) {
    unsafe {
        send(&[
            key_event(vk, KEYBD_EVENT_FLAGS(0)),
            key_event(vk, KEYEVENTF_KEYUP),
        ])
    }
}

/// Ajoute le caractère en événements Unicode (paire de substitution incluse).
fn push_char(inputs: &mut Vec<INPUT>, c: char) {
    let mut buf = [0u16; 2];
    let n = c.encode_utf16(&mut buf).len();
    for &u in &buf[..n] {
        for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: u,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: MAGIC,
                    },
                },
            });
        }
    }
}

/// Alt tenu, l'injection Unicode arriverait en WM_SYSCHAR (ignoré par les
/// applis) : on relâche Alt le temps de la frappe puis on le remet, en un seul
/// SendInput pour que l'app ne voie jamais un Alt isolé (barre de menus).
fn inject_char_under_alt(c: char) {
    let mut inputs = Vec::with_capacity(6);
    inputs.push(key_event(VK_LMENU, KEYEVENTF_KEYUP));
    push_char(&mut inputs, c);
    inputs.push(key_event(VK_LMENU, KEYBD_EVENT_FLAGS(0)));
    unsafe { send(&inputs) }
}

/// Nos injections : SendInput marqué MAGIC, plus l'ancien repérage vk 0x8/0xE7
/// (rétrocompatible). En mode test, on les laisse passer pour pouvoir simuler.
fn injected_ours(kb: &KBDLLHOOKSTRUCT) -> bool {
    let injected = kb.flags.0 & 0x10 != 0; // LLKHF_INJECTED
    let ours = kb.dwExtraInfo == MAGIC || matches!(kb.vkCode, 0x8 | 0xE7);
    if injected && ours && !INJECT_TEST.load(Ordering::Relaxed) {
        crate::dlog::dlog(&format!("ignoré injected vk={:#x}", kb.vkCode));
        return true;
    }
    false
}

/// État du glisser souris (thread du hook, entre down et up).
struct MouseState {
    down: Option<(i32, i32)>,
    last_up: Option<((i32, i32), std::time::Instant)>,
}
static MOUSE: Mutex<MouseState> = Mutex::new(MouseState {
    down: None,
    last_up: None,
});

/// glissement minimal détecté (euclidien² > 36) pour qu'un simple clic
/// compte comme sélection
const DRAG_PX: i32 = 6;
/// fenêtre du double-clic : le 2e relâché survient dans ce délai
const DOUBLE_MS: u64 = 450;
/// deux clics rapides MAIS éloignés = deux clics, pas une sélection
const DOUBLE_PX: i32 = 10;

/// Relâchement gauche = sélection probable ? glissement > 6 px, ou
/// double-clic (2e relâché ≤ 450 ms après le précédent, même endroit).
fn drag_detected(down: (i32, i32), up: (i32, i32), prev_up: Option<((i32, i32), u64)>) -> bool {
    let dx = up.0 - down.0;
    let dy = up.1 - down.1;
    if dx * dx + dy * dy > DRAG_PX * DRAG_PX {
        return true;
    }
    let Some((p, ms)) = prev_up else { return false };
    if ms > DOUBLE_MS {
        return false;
    }
    let ex = down.0 - p.0;
    let ey = down.1 - p.1;
    ex * ex + ey * ey <= DOUBLE_PX * DOUBLE_PX
}

/// Le point tombe-t-il sur une fenêtre de CE process (la bulle) ?
fn over_own_window(pt: windows::Win32::Foundation::POINT) -> bool {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(WindowFromPoint(pt), Some(&mut pid));
        pid == GetCurrentProcessId()
    }
}

/// Hook souris (WH_MOUSE_LL) : détection de sélection, jamais de swallow.
/// Rapide : mutex + spawn seulement — presse-papiers et réseau vivent dans
/// `def::worker`.
unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // même garde anti-panic que le clavier : une serrure empoisonnée ne doit
    // pas tuer le process ni geler la boucle de messages
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        mouse_impl(code, wparam, lparam)
    })) {
        Ok(r) => r,
        Err(_) => {
            crate::dlog::dlog("PANIC dans le hook souris — événement laissé passer");
            unsafe { CallNextHookEx(None, code, wparam, lparam) }
        }
    }
}

unsafe fn mouse_impl(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            let pt = (ms.pt.x, ms.pt.y);
            match wparam.0 as u32 {
                WM_LBUTTONDOWN => {
                    // le clic ferme la bulle de définition et invalide une
                    // recherche en vol (un changement d'app passe par un clic)
                    MOUSE.lock().unwrap_or_else(|e| e.into_inner()).down = Some(pt);
                    crate::def::bump();
                    notify(KeyMsg::Click);
                    // clic hors bulle alors qu'une suggestion est ouverte : on la
                    // ferme (sinon elle reste zombie au-dessus de l'app cible) —
                    // le clic n'est jamais avalé, le changement d'app suit son cours
                    if POPUP_OPEN.load(Ordering::Relaxed)
                        && popup_drawn()
                        && !over_own_window(ms.pt)
                    {
                        crate::dlog::dlog("clic hors bulle — suggestion fermée");
                        notify(KeyMsg::Invalidate);
                    }
                }
                WM_LBUTTONUP => {
                    let mut g = MOUSE.lock().unwrap_or_else(|e| e.into_inner());
                    let down = g.down.take();
                    let prev = g.last_up.map(|(p, t)| (p, t.elapsed().as_millis() as u64));
                    g.last_up = Some((pt, std::time::Instant::now()));
                    drop(g);
                    if let Some(d) = down
                        && !is_paused()
                        && drag_detected(d, pt, prev)
                        // app exclue (exclusions.txt) : pas de déclencheur
                        && !is_excluded()
                        && crate::def::try_begin()
                    {
                        // epoch figé au relâchement : le worker refuse d'injecter
                        // si l'utilisateur a repris la main entre-temps
                        let epoch_up = crate::def::current();
                        crate::dlog::dlog(&format!("def : sélection détectée {d:?}→{pt:?}"));
                        std::thread::spawn(move || crate::def::worker(epoch_up));
                    }
                }
                _ => {}
            }
        }
        CallNextHookEx(None, code, wparam, lparam)
    }
}

/// Sous Shift, la touche appartient à l'application : seuls le texte et le
/// Backspace restent à Plume (Maj+lettre = majuscule, Maj+Retour arrière =
/// suppression). Tout le reste (Entrée, flèches, Échap…) passe intact.
fn app_owns_shift(msg: KeyMsg) -> bool {
    !matches!(msg, KeyMsg::Char(_) | KeyMsg::Backspace)
}

/// Bulle ouverte : seules ↑↓/Entrée/Échap sont avalées (la bulle les pilote).
/// Toute écriture passe TOUJOURS — c'est elle qui ferme la bulle, et le
/// caractère qui la ferme ne doit jamais être perdu (bug du « d » avalé).
fn swallow(msg: KeyMsg, popup_open: bool) -> bool {
    popup_open
        && matches!(
            msg,
            KeyMsg::Up | KeyMsg::Down | KeyMsg::Return | KeyMsg::Escape
        )
}

/// Classe un enfoncement « normal ». Les raccourcis (Ctrl/Win/Alt) et les
/// remappages Alt ont déjà été traités par `hook_impl`.
fn classify(kb: &KBDLLHOOKSTRUCT) -> Option<KeyMsg> {
    unsafe {
        let vk = kb.vkCode;

        if vk == VK_UP.0 as u32 {
            return Some(KeyMsg::Up);
        }
        if vk == VK_DOWN.0 as u32 {
            return Some(KeyMsg::Down);
        }
        if vk == VK_RETURN.0 as u32 {
            return Some(KeyMsg::Return);
        }
        if vk == VK_ESCAPE.0 as u32 {
            return Some(KeyMsg::Escape);
        }
        if vk == VK_BACK.0 as u32 {
            return Some(KeyMsg::Backspace);
        }
        if vk == VK_LEFT.0 as u32
            || vk == VK_RIGHT.0 as u32
            || vk == VK_HOME.0 as u32
            || vk == VK_END.0 as u32
            || vk == VK_PRIOR.0 as u32
            || vk == VK_NEXT.0 as u32
            || vk == 0x2D
            || vk == 0x2E
            || vk == 0x09
        {
            // ← → Home End PgUp PgDn Ins Del Tab
            return Some(KeyMsg::Other);
        }
        // modificateurs seuls (Shift/Ctrl/Alt/Caps/Win) : ni caractère ni fermeture de bulle
        if is_modifier(vk) {
            return None;
        }

        to_char(kb).map(KeyMsg::Char)
    }
}

/// shortcut: le nom de process est relu à chaque frappe ; si le buffer contient du
/// texte saisi avant exclusion (ou dans une app exclue), il devient périmé au retour —
/// faux positif possible, cas non géré.
fn is_excluded() -> bool {
    let Some(list) = EXCLUDED.get() else {
        return false;
    };
    if list.is_empty() {
        return false;
    }
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid as *mut u32));
        if pid == 0 {
            return false;
        }
        let Ok(proc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut buf = [0u16; 260];
        let mut size = buf.len() as u32;
        let got = QueryFullProcessImageNameW(
            proc,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
        .is_ok();
        let _ = CloseHandle(proc);
        if !got {
            return false;
        }
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        let file = path.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
        list.contains(&file)
    }
}

fn load_exclusions() -> Vec<String> {
    let content = match std::fs::read_to_string("exclusions.txt") {
        Ok(c) => c,
        Err(_) => {
            // premier lancement : on écrit un modèle commenté, la liste reste vide
            let _ = std::fs::write(
                "exclusions.txt",
                "# Apps à exclure : un nom de fichier .exe par ligne, insensible à la casse.\n\
                 # Les lignes commençant par # sont ignorées. Exemples :\n\
                 # Discord.exe\n\
                 # Steam.exe\n",
            );
            String::new()
        }
    };
    parse_exclusions(&content)
}

fn parse_exclusions(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_lowercase)
        .collect()
}

unsafe fn to_char(kb: &KBDLLHOOKSTRUCT) -> Option<char> {
    unsafe {
        let mut state = [0u8; 256];
        for v in [
            VK_SHIFT,
            VK_LSHIFT,
            VK_RSHIFT,
            VK_CONTROL,
            VK_LCONTROL,
            VK_RCONTROL,
            VK_MENU,
            VK_LMENU,
            VK_RMENU,
            VK_CAPITAL,
        ] {
            if GetAsyncKeyState(v.0 as i32) & 0x8000u16 as i16 != 0 {
                state[v.0 as usize & 0xFF] |= 0x80;
            }
        }
        // le bit &1 de GetAsyncKeyState n'est PAS le verrou (« pressé depuis le
        // dernier appel ») : CapsLock se lisait toujours off → hook et app
        // divergeaient (ex. CapsLock+3 : l'app tape `3`, le hook voit `"` et la
        // règle 5 mange le 3). MSDN impose GetKeyState pour le toggle.
        if GetKeyState(VK_CAPITAL.0 as i32) & 1i16 != 0 {
            state[VK_CAPITAL.0 as usize & 0xFF] |= 0x01;
        }
        state[kb.vkCode as usize & 0xFF] |= 0x80;

        let tid = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let hkl = GetKeyboardLayout(tid);
        let mut buf = [0u16; 4];
        let n = ToUnicodeEx(kb.vkCode, kb.scanCode, &state, &mut buf, 0, Some(hkl));
        if n < 0 {
            // dead key (^ ` ´…) : on remet l'état PARTAGÉ à zéro pour que
            // TranslateMessage de l'app voie l'état propre et pose SON dead key
            // (sans ça il compose avec le nôtre → « gr^^ace »), et on mémorise
            // la touche pour réarmer l'état avant qu'elle ne compose
            let mut junk = [0u16; 4];
            let _ = ToUnicodeEx(kb.vkCode, kb.scanCode, &state, &mut junk, 0, Some(hkl));
            *DEAD.lock().unwrap_or_else(|e| e.into_inner()) = Some((kb.vkCode, kb.scanCode));
            crate::dlog::dlog(&format!("dead key vk={:#x} réinitialisée", kb.vkCode));
            return None;
        }
        if n == 0 {
            return None;
        }
        // notre appel vient de consommer l'état mort posé par l'app : on le remet
        // en place, sinon l'app recevrait la lettre non composée (« gre »).
        // shortcut: plume en pause entre la touche morte et la lettre → DEAD non
        // armé → un caractère possible non composé (cas rare, non géré)
        if let Some((dvk, dscan)) = DEAD.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let mut junk = [0u16; 4];
            let _ = ToUnicodeEx(dvk, dscan, &state, &mut junk, 0, Some(hkl));
            crate::dlog::dlog(&format!("réarmement dead key vk={dvk:#x}"));
        }
        String::from_utf16_lossy(&buf[..n as usize])
            .chars()
            .find(|c| !c.is_control())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn drag_ou_double_clic_detectes() {
        // glissement > 6 px = sélection par drag
        assert!(super::drag_detected((0, 0), (7, 0), None));
        // 5 px = simple clic (bruit de main)
        assert!(!super::drag_detected((0, 0), (5, 0), None));
        // double-clic : 2e relâché dans les 450 ms au même endroit
        assert!(super::drag_detected((0, 0), (0, 0), Some(((0, 0), 400))));
        assert!(super::drag_detected((0, 0), (0, 0), Some(((2, 2), 450))));
        // trop lent = deux clics indépendants
        assert!(!super::drag_detected((0, 0), (0, 0), Some(((0, 0), 600))));
        // rapides mais éloignés = deux clics, pas une sélection
        assert!(!super::drag_detected((0, 0), (0, 0), Some(((50, 50), 400))));
    }

    #[test]
    fn exclusions_ignore_commentaires_casse_et_espaces() {
        let v = super::parse_exclusions("# Discord.exe\nSteam.EXE\n\n  vscode.exe  \n");
        assert_eq!(v, vec!["steam.exe", "vscode.exe"]);
    }

    #[test]
    fn seule_la_navigation_est_avalee() {
        use super::{KeyMsg, swallow};
        // popup ouverte : ↑↓/Entrée/Échap consommés — la bulle les pilote
        assert!(swallow(KeyMsg::Up, true));
        assert!(swallow(KeyMsg::Down, true));
        assert!(swallow(KeyMsg::Return, true));
        assert!(swallow(KeyMsg::Escape, true));
        // l'écriture n'est JAMAIS avalée : elle ferme la bulle et le caractère
        // qui la ferme doit arriver dans l'application (bug du « d » perdu)
        assert!(!swallow(KeyMsg::Char('d'), true));
        assert!(!swallow(KeyMsg::Backspace, true));
        assert!(!swallow(KeyMsg::Invalidate, true));
        // bulle fermée : rien n'est avalé
        assert!(!swallow(KeyMsg::Up, false));
    }

    #[test]
    fn shift_laisse_la_main_a_l_application() {
        use super::{KeyMsg, app_owns_shift};
        // Shift+Enter (saut de ligne Discord), Shift+flèches (sélection)…
        assert!(app_owns_shift(KeyMsg::Return));
        assert!(app_owns_shift(KeyMsg::Up));
        assert!(app_owns_shift(KeyMsg::Down));
        assert!(app_owns_shift(KeyMsg::Escape));
        // …mais Maj+lettre et Maj+Retour arrière restent du texte pour Plume
        assert!(!app_owns_shift(KeyMsg::Char('A')));
        assert!(!app_owns_shift(KeyMsg::Backspace));
    }

    #[test]
    fn alt_caracteres_speciaux() {
        use super::special_char;
        assert_eq!(special_char(0x04, false, false), Some('«'));
        assert_eq!(special_char(0x04, true, false), Some('‹'));
        assert_eq!(special_char(0x05, false, false), Some('»'));
        assert_eq!(special_char(0x05, true, false), Some('›'));
        assert_eq!(special_char(0x32, false, false), Some('‽'));
        assert_eq!(special_char(0x07, false, false), Some('–'));
        assert_eq!(special_char(0x07, true, false), Some('—'));
        assert_eq!(special_char(0x34, false, false), Some('…'));
        assert_eq!(special_char(0x0B, false, false), Some('À'));
        // É exige Shift : Alt+2 seul reste un raccourci
        assert_eq!(special_char(0x03, false, false), None);
        assert_eq!(special_char(0x03, true, false), Some('É'));
        assert_eq!(special_char(0x2E, false, false), Some('ç'));
        assert_eq!(special_char(0x2E, true, false), Some('Ç'));
        assert_eq!(special_char(0x18, false, false), Some('ô'));
        assert_eq!(special_char(0x18, false, true), Some('Ô'));
        assert_eq!(special_char(0x18, true, false), Some('œ'));
        assert_eq!(special_char(0x18, true, true), Some('Œ'));
        assert_eq!(special_char(0x16, false, false), Some('ù'));
        assert_eq!(special_char(0x16, true, false), Some('Ù'));
        assert_eq!(special_char(0x17, false, false), Some('ï'));
        assert_eq!(special_char(0x17, true, false), Some('Ï'));
        // touche non mappée : Alt+X passe à l'application
        assert_eq!(special_char(0x1E, false, false), None);
    }

    #[test]
    fn repli_vk_quand_le_scancode_est_inconnu() {
        use super::key_scan;
        // scancode connu : prioritaire, le vk n'est pas consulté
        assert_eq!(key_scan(0x18, 0x4F), Some(0x18));
        assert_eq!(key_scan(0x1E, 0x51), None);
        // scancode absent/inattendu : repli sur le vkCode
        assert_eq!(key_scan(0, 0x4F), Some(0x18)); // Alt+o
        assert_eq!(key_scan(0, 0x33), Some(0x04)); // Alt+3
        assert_eq!(key_scan(0, 0xBC), Some(0x32)); // Alt+,
        assert_eq!(key_scan(0, 0x41), None); // Alt+a : non mappé
    }

    #[test]
    fn f24_simple_puis_double() {
        use super::f24_next;
        use windows::Win32::UI::Input::KeyboardAndMouse::VK_F11;
        // premier appui : rien tout de suite, on arme la fenêtre de 250 ms
        assert_eq!(f24_next(false), (true, None));
        // second appui dans la fenêtre : F11 immédiat
        assert_eq!(f24_next(true), (false, Some(VK_F11)));
    }

    #[test]
    fn modificateurs_reconnus_un_par_un() {
        // piège : un const avec `|` vaut 0xFF — un seul vk correspondrait
        for vk in [
            0x10, 0xA0, 0xA1, 0x11, 0xA2, 0xA3, 0x12, 0xA4, 0xA5, 0x14, 0x5B, 0x5C,
        ] {
            assert!(
                super::is_modifier(vk),
                "vk={vk:#x} doit être un modificateur"
            );
        }
        for vk in [0x33, 0x41, 0x08, 0x0D, 0x1B, 0xFF] {
            assert!(
                !super::is_modifier(vk),
                "vk={vk:#x} n'est pas un modificateur"
            );
        }
    }

    /// Pourquoi `to_char` lit le verrou CapsLock avec `GetKeyState` : le bit
    /// `&1` de `GetAsyncKeyState` n'est PAS le toggle (« pressé depuis le
    /// dernier appel ») et diverge de l'état réel, alors que l'app (TranslateMessage)
    /// lit `GetKeyState`. Divergence = plume corrige sur un faux caractère.
    /// `cargo test --release sonde_async_caps -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn sonde_async_caps() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetKeyState, VK_CAPITAL,
        };
        unsafe extern "system" {
            fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
        }
        let k = |f: unsafe fn(i32) -> i16| (unsafe { f(VK_CAPITAL.0 as i32) } & 1) != 0;
        let t0 = k(GetKeyState);
        println!(
            "avant:      GetKeyState={t0} GetAsyncKeyState={}",
            k(GetAsyncKeyState)
        );
        unsafe {
            keybd_event(0x14, 0, 0, 0);
            keybd_event(0x14, 0, 2, 0)
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        let t1 = k(GetKeyState);
        println!(
            "apres 1er:  GetKeyState={t1} GetAsyncKeyState={}",
            k(GetAsyncKeyState)
        );
        unsafe {
            keybd_event(0x14, 0, 0, 0);
            keybd_event(0x14, 0, 2, 0)
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        let (t2, a2) = (k(GetKeyState), k(GetAsyncKeyState));
        println!("apres 2eme: GetKeyState={t2} GetAsyncKeyState={a2}");
        assert_eq!(t1, !t0, "GetKeyState doit suivre la bascule (contrôle)");
        assert_eq!(t2, t0, "GetKeyState doit revenir à l'état initial");
        if a2 != t2 {
            println!(
                "=> GetAsyncKeyState diverge : lecture interdite pour le toggle (d'où GetKeyState dans to_char)"
            );
        }
    }

    /// Diagnostic manuel du modèle d'état ToUnicodeEx : `cargo test --release
    /// etat_to_unicode -- --ignored --nocapture`. Deux threads : A pose la touche
    /// morte, B appuie sur « e » — composition = état partagé entre threads.
    #[test]
    #[ignore]
    fn etat_to_unicode_partage_entre_threads() {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, ToUnicodeEx};
        unsafe {
            let hkl = GetKeyboardLayout(0);
            let tou = |vk: u32| -> (i32, String) {
                let mut state = [0u8; 256];
                state[vk as usize & 0xFF] = 0x80;
                let mut buf = [0u16; 4];
                let n = ToUnicodeEx(vk, 0, &state, &mut buf, 0, Some(hkl));
                let s = if n > 0 {
                    String::from_utf16_lossy(&buf[..n as usize])
                } else {
                    String::new()
                };
                (n, s)
            };
            // sonde : touche morte de la disposition courante (reset après)
            let mut dead = None;
            for vk in 0xC0..=0xE1 {
                if tou(vk).0 < 0 {
                    dead = Some(vk);
                    tou(vk);
                    break;
                }
            }
            let Some(dead) = dead else {
                println!("aucune touche morte sur cette disposition — test sans objet");
                return;
            };
            let (n1, _) = tou(dead); // thread A : état mort posé
            // HKL (pointeur brut) n'est pas Send : on le transporte en usize
            let hkl_usize = hkl.0 as usize;
            let b = std::thread::spawn(move || {
                let mut state = [0u8; 256];
                state[0x45] = 0x80;
                let mut buf = [0u16; 4];
                // unsafe couvert par le bloc englobant du test (unused_unsafe)
                let hkl = windows::Win32::UI::Input::KeyboardAndMouse::HKL(hkl_usize as _);
                let n = ToUnicodeEx(0x45, 0, &state, &mut buf, 0, Some(hkl));
                if n > 0 {
                    String::from_utf16_lossy(&buf[..n as usize])
                } else {
                    format!("n={n}")
                }
            })
            .join()
            .unwrap();
            if tou(dead).0 < 0 {
                tou(dead); // nettoyage : remise à zéro si l'état est encore armé
            }
            println!("A dead vk={dead:#x} → n={n1} | B « e » → {b:?}");
            println!(
                "{}",
                if b == "ê" {
                    "→ ÉTAT PARTAGÉ entre threads (modèle retenu par le fix reset+réarmement)"
                } else {
                    "→ états indépendants par thread"
                }
            );
        }
    }
}
