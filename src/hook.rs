//! Hook clavier global (WH_KEYBOARD_LL) : reconstitue les caractères tapés
//! via ToUnicodeEx et avale ↑↓/Entrée/Échap quand la bulle est ouverte.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};

use eframe::egui;
use windows::Win32::Foundation::{CloseHandle, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetKeyboardLayout, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
    RegisterHotKey, ToUnicodeEx, UnregisterHotKey, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DOWN,
    VK_END, VK_ESCAPE, VK_HOME, VK_LCONTROL, VK_LEFT, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU,
    VK_NEXT, VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT, VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT,
    VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
    KBDLLHOOKSTRUCT, MSG, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_KEYBOARD_LL,
    WM_HOTKEY, WM_KEYDOWN, WM_SYSKEYDOWN,
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
static INJECT_TEST: AtomicBool = AtomicBool::new(false);
/// pause globale : en true, hook_proc laisse passer toutes les frappes sans traiter
static PAUSED: AtomicBool = AtomicBool::new(false);
/// mode forcé (option tray, défaut off) : en true, l'écriture est bloquée
/// tant que la bulle est ouverte ; sinon la frappe ferme la bulle et passe
static FORCE: AtomicBool = AtomicBool::new(false);
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

pub fn is_force() -> bool {
    FORCE.load(Ordering::Relaxed)
}

pub fn set_force(v: bool) {
    if v != is_force() {
        crate::dlog::dlog(if v {
            "MODE FORCÉ activé"
        } else {
            "MODE FORCÉ levé"
        });
    }
    FORCE.store(v, Ordering::Relaxed);
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
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnregisterHotKey(None, HOTKEY_PAUSE);
        let _ = UnhookWindowsHookEx(hook);
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code < 0 {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        // en pause : aucune frappe traitée, aucun swallow — le hook fait juste transiter
        if PAUSED.load(Ordering::Relaxed) {
            return CallNextHookEx(None, code, wparam, lparam);
        }
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        // 0x10 = LLKHF_INJECTED : seules nos propres frappes (backspace/Unicode via
        // enigo) sont ignorées — sinon accepter « En réalité » re-déclencherait la
        // bulle. Les injections tierces (macros : Ctrl/Alt/Échap) sont traitées
        // normalement : sans ça aucun Invalidate → buffer périmé → popup à côté.
        let injected = kb.flags.0 & 0x10 != 0;
        let ours = matches!(kb.vkCode, 0x8 | 0xE7);
        let ignore = injected && ours && !INJECT_TEST.load(Ordering::Relaxed);
        if ignore {
            crate::dlog::dlog(&format!("ignoré injected vk={:#x}", kb.vkCode));
        }
        if !ignore && let Some(msg) = classify(kb, wparam) {
            if is_excluded() {
                crate::dlog::dlog("exclu (app dans exclusions.txt) — frappe ignorée");
                return CallNextHookEx(None, code, wparam, lparam);
            }
            let (s, c) = modstate();
            crate::dlog::dlog(&format!("hook vk={:#x} -> {msg:?} s={s} c={c}", kb.vkCode));
            // changement d'app devant la bulle : suggestion périmée → on la ferme
            // (Invalidate) et la frappe repasse comme bulle fermée. Sans ça, en mode
            // forcé, toutes les frappes seraient avalées PARTOUT jusqu'à Échap.
            let mut open = POPUP_OPEN.load(Ordering::Relaxed);
            if open
                && POPUP_HWND.load(Ordering::Relaxed) != 0
                && GetForegroundWindow().0 as isize != POPUP_HWND.load(Ordering::Relaxed)
            {
                crate::dlog::dlog("changement d'app — bulle invalidée");
                open = false;
                if let Some(tx) = TX.get() {
                    let _ = tx.send(KeyMsg::Invalidate);
                }
                repint();
            }
            let (transmit, swallow) = flow(msg, open);
            if transmit
                && let Some(tx) = TX.get()
                && tx.send(msg).is_ok()
            {
                repint();
            }
            if swallow {
                crate::dlog::dlog("avale la touche");
                return LRESULT(1);
            }
        } else if !ignore && is_modifier(kb.vkCode) {
            // diagnostic : les modificateurs ne classent aucun message et étaient
            // invisibles au journal — on les rend visibles avec l'état lu par le
            // hook, pour corréler Maj/Caps au moment d'une frappe testée
            let (s, c) = modstate();
            let up = wparam.0 == 0x101 || wparam.0 == 0x105; // WM_KEYUP / WM_SYSKEYUP
            crate::dlog::dlog(&format!(
                "mod vk={:#x} {} s={s} c={c}",
                kb.vkCode,
                if up { "up" } else { "down" }
            ));
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

/// État que `to_char` utilise pour traduire (Maj tenue / CapsLock verrouillé) — journal.
fn modstate() -> (bool, bool) {
    unsafe {
        let down = |v: i32| GetAsyncKeyState(v) & 0x8000u16 as i16 != 0;
        let s = down(VK_SHIFT.0 as i32) || down(VK_LSHIFT.0 as i32) || down(VK_RSHIFT.0 as i32);
        let c = GetKeyState(VK_CAPITAL.0 as i32) & 1i16 != 0;
        (s, c)
    }
}

/// (transmettre à plume, avaler la frappe) : avec le mode forcé (option tray),
/// l'écriture est avalée ET non transmise tant que la bulle est ouverte —
/// sinon `on_msg` la traiterait et refermerait la popup avant Entrée/Échap.
/// Par défaut (forcé off) la frappe passe et ferme la bulle.
fn flow(msg: KeyMsg, popup_open: bool) -> (bool, bool) {
    let write = matches!(msg, KeyMsg::Char(_) | KeyMsg::Backspace);
    let nav = matches!(
        msg,
        KeyMsg::Up | KeyMsg::Down | KeyMsg::Return | KeyMsg::Escape
    );
    if !popup_open {
        return (true, false);
    }
    let force = FORCE.load(Ordering::Relaxed);
    // navigation : toujours transmise à l'app (la bulle y réagit) et avalée
    // (sinon la flèche bougerait aussi le curseur de l'application)
    if nav {
        return (true, true);
    }
    if write {
        return if force { (false, true) } else { (true, false) };
    }
    // Invalidate/Other : transmis (ferme la bulle), non avalés
    (true, false)
}

fn classify(kb: &KBDLLHOOKSTRUCT, wparam: WPARAM) -> Option<KeyMsg> {
    unsafe {
        if wparam.0 != WM_KEYDOWN as usize && wparam.0 != WM_SYSKEYDOWN as usize {
            return None;
        }
        let vk = kb.vkCode;

        let down = |v: i32| GetAsyncKeyState(v) & 0x8000u16 as i16 != 0;
        // AltGr = RMenu + Ctrl synthétique : on le laisse passer comme une frappe normale
        let altgr = down(VK_RMENU.0 as i32);
        // Ctrl+Alt+P = raccourci de pause : jamais traité comme une frappe, jamais swalloué
        // (WM_HOTKEY le gère dans la boucle de messages du thread hook)
        if vk == u32::from(b'P') && down(VK_CONTROL.0 as i32) && (down(VK_LMENU.0 as i32) || altgr)
        {
            return None;
        }
        // Alt gauche = raccourci (Alt+D, Alt+Espace…) : jamais une frappe, invalide
        // le buffer ; AltGr (RMenu) reste une frappe normale (é, è…)
        if ((down(VK_CONTROL.0 as i32) || down(VK_LMENU.0 as i32)) && !altgr)
            || down(VK_LWIN.0 as i32)
            || down(VK_RWIN.0 as i32)
        {
            return Some(KeyMsg::Invalidate);
        }

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
            *DEAD.lock().unwrap() = Some((kb.vkCode, kb.scanCode));
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
        if let Some((dvk, dscan)) = DEAD.lock().unwrap().take() {
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
    fn exclusions_ignore_commentaires_casse_et_espaces() {
        let v = super::parse_exclusions("# Discord.exe\nSteam.EXE\n\n  vscode.exe  \n");
        assert_eq!(v, vec!["steam.exe", "vscode.exe"]);
    }

    #[test]
    fn bulle_ouverte_flow_selon_mode_force() {
        use super::{KeyMsg, flow, set_force};
        // défaut (mode forcé off) : l'écriture passe et ferme la bulle
        set_force(false);
        assert_eq!(flow(KeyMsg::Char('a'), true), (true, false));
        assert_eq!(flow(KeyMsg::Backspace, true), (true, false));
        // mode forcé : écriture avalée ET non transmise (bulle tenue jusqu'à
        // Entrée/Échap)
        set_force(true);
        assert_eq!(flow(KeyMsg::Char('a'), true), (false, true));
        assert_eq!(flow(KeyMsg::Backspace, true), (false, true));
        set_force(false); // remise à zéro pour les autres tests
        // navigation : avalée mais transmise — la bulle doit y réagir
        assert_eq!(flow(KeyMsg::Up, true), (true, true));
        assert_eq!(flow(KeyMsg::Return, true), (true, true));
        // bulle fermée : tout transmis, rien avalé
        assert_eq!(flow(KeyMsg::Char('a'), false), (true, false));
        // Ctrl+V pendant la bulle : transmis (ferme la bulle), non avalé
        assert_eq!(flow(KeyMsg::Invalidate, true), (true, false));
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
