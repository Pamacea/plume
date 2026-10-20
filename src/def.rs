//! Définition au surlignage souris : la sélection est lue par Ctrl+C injecté,
//! le mot cherché au Wiktionnaire FR (HTTPS, 5 s max) dans un worker ; le
//! résultat est poussé dans `DEF` et signalé par `KeyMsg::Def`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// en cours d'une recherche (anti-empilement : un seul worker à la fois)
static BUSY: AtomicBool = AtomicBool::new(false);
/// bumpé à chaque frappe/clic : un résultat plus vieux que l'epoch courant
/// est périmé (l'utilisateur a bougé entre-temps)
static EPOCH: AtomicU64 = AtomicU64::new(0);
/// (epoch, mot, définition) prêt à afficher, pris par l'app à la réception
static DEF: Mutex<Option<(u64, String, String)>> = Mutex::new(None);
/// cache du dernier succès (un même mot revient souvent pendant une session)
static LAST: Mutex<Option<(String, String)>> = Mutex::new(None);

/// Longueur max d'une sélection recherchable : le mot le plus long du
/// français (« anticonstitutionnellement », 25 ; « protézokératoconjonctivites »,
/// 27) reste dedans, une phrase surlignée entière non.
const MAX_LEN: usize = 32;
/// ponctuation/symboles retirés aux bords d'une sélection
const TRIM: &[char] = &[
    '«', '»', '"', '\'', '’', '‘', '(', ')', '[', ']', '{', '}', ',', ';', ':', '.', '!', '?', '…',
    '-', '–',
];

pub fn bump() {
    EPOCH.fetch_add(1, Ordering::Relaxed);
}

pub fn current() -> u64 {
    EPOCH.load(Ordering::Relaxed)
}

/// Prend le tour du worker : false = un worker tourne déjà.
pub fn try_begin() -> bool {
    BUSY.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
}

pub fn take_def() -> Option<(u64, String, String)> {
    DEF.lock().unwrap().take()
}

struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Relaxed);
    }
}

/// Squeeze les espaces (une sélection copiée garde parfois sa mise en page).
fn squeeze(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Sélection → mot : 1re ligne, espaces condensés, bords dégarnis de
/// ponctuation, 2..=32 caractères. Plusieurs mots restent assemblés (la
/// recherche échouera proprement) ; trop long = phrase surlignée → `None`.
fn clean(sel: &str) -> Option<String> {
    let line = squeeze(sel.lines().next().unwrap_or_default());
    // espace DANS la closure : « albâtre, » a un espace entre la ponctuation
    // et le mot — trim à étapes séparées s'arrêterait dessus
    let mot = line.trim_matches(|c: char| c.is_whitespace() || TRIM.contains(&c));
    let n = mot.chars().count();
    if !(2..=MAX_LEN).contains(&n) {
        return None;
    }
    Some(mot.to_string())
}

fn urlencode(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                o.push(*b as char)
            }
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// Nettoie une ligne de définition wikitexte : `{{gloses}}` retirées (imbrication),
/// `[[a|b]]` → `b`, marques `''italique''`/`'''gras'''` disparues, espaces squeezés.
fn simplify(line: &str) -> String {
    let mut s = line.to_string();
    // {{...}} avec compteur de profondeur ({{pron|...}} imbriqué dans {{lexique}})
    while let Some(st) = s.find("{{") {
        let bytes = s.as_bytes();
        let (mut depth, mut end) = (0i32, None);
        let mut i = st;
        while i + 1 < bytes.len() {
            if bytes[i] == b'{' && bytes[i + 1] == b'{' {
                depth += 1;
                i += 2;
            } else if bytes[i] == b'}' && bytes[i + 1] == b'}' {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            } else {
                i += 1;
            }
        }
        s.replace_range(st..end.unwrap_or(s.len()), "");
    }
    // [[cible|texte]] → texte ; [[cible]] → cible
    while let Some(st) = s.find("[[") {
        let Some(rel) = s[st + 2..].find("]]") else {
            break;
        };
        let inner = &s[st + 2..st + 2 + rel];
        let repl = inner.rsplit('|').next().unwrap_or(inner).to_string();
        s.replace_range(st..st + 2 + rel + 2, &repl);
    }
    // runs de 2+ apostrophes (marques de style) retirés ; l'apostrophe
    // simple (« d'eau ») est conservée
    let mut out = String::with_capacity(s.len());
    let mut quotes = 0usize;
    for c in s.chars() {
        if c == '\'' {
            quotes += 1;
            continue;
        }
        if quotes < 2 {
            for _ in 0..quotes {
                out.push('\'');
            }
        }
        quotes = 0;
        out.push(c);
    }
    if quotes < 2 {
        for _ in 0..quotes {
            out.push('\'');
        }
    }
    squeeze(&out).trim().to_string()
}

/// Première ligne `#` (hors `##`/`#*`/`#:`) de la dernière section de langue
/// française `=== {{S|...|fr}} ===` ; aucune section FR → `None`.
fn extract_def(wikitext: &str) -> Option<String> {
    let mut in_fr = false;
    for line in wikitext.lines() {
        if line.starts_with("==") {
            in_fr = line.starts_with("=== {{S|") && line.contains("|fr}}");
            continue;
        }
        if in_fr
            && let Some(rest) = line.strip_prefix('#')
            && !rest.starts_with(['#', '*', ':'])
        {
            let def = simplify(rest.trim_start());
            if !def.is_empty() {
                return Some(def);
            }
        }
    }
    None
}

fn wikitext(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("parse")?
        .get("wikitext")?
        .as_str()
        .map(str::to_string)
}

/// Définition d'un mot via l'API du Wiktionnaire FR. `None` = page absente,
/// pas de section FR, hors-ligne ou timeout. 1 seule requête (LAST est le cache).
fn lookup(mot: &str) -> Option<String> {
    let url = format!(
        "https://fr.wiktionary.org/w/api.php?action=parse&page={}&prop=wikitext&format=json&formatversion=2&redirects=1",
        urlencode(mot)
    );
    let cfg = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build();
    let agent: ureq::Agent = cfg.into();
    // Err(StatusCode) = page inexistante : pas une panne, juste rien à afficher
    let body: String = agent
        .get(&url)
        .call()
        .ok()?
        .body_mut()
        .read_to_string()
        .ok()?;
    extract_def(&wikitext(&body)?)
}

/// Candidats de recherche pour une sélection, du plus fidèle au plus souple :
/// tel quel, puis minuscule, puis segments découpés sur espaces/apostrophes.
/// « cherchent Vérité l'humilité » n'a pas de page mais « cherchent » si ;
/// « l'humilité » n'en a pas non plus mais « humilité » oui.
fn candidats(mot: &str) -> Vec<String> {
    let bas = mot.to_lowercase();
    let mut v = vec![mot.to_string()];
    if bas != *mot {
        v.push(bas.clone());
    }
    for seg in bas.split(|c: char| c.is_whitespace() || c == '\'' || c == '’') {
        let seg = seg.trim_matches(|c: char| c.is_whitespace() || TRIM.contains(&c));
        if seg.chars().count() >= 2 && !v.iter().any(|x| x == seg) {
            v.push(seg.to_string());
        }
        if v.len() >= 5 {
            break;
        }
    }
    v
}

/// lookup avec secours (voir `candidats`) : premier candidat trouvé gagne.
fn lookup_souple(mot: &str) -> Option<String> {
    candidats(mot).iter().find_map(|c| lookup(c))
}

fn clip_open() -> bool {
    // le presse-papiers est un bien global : un autre process peut le tenir
    for _ in 0..3 {
        if unsafe { OpenClipboard(None) }.is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn clip_get() -> Option<String> {
    if !clip_open() {
        return None;
    }
    unsafe {
        let mut out = None;
        if let Ok(h) = GetClipboardData(CF_UNICODETEXT.0 as u32)
            && !h.is_invalid()
        {
            let p = GlobalLock(HGLOBAL(h.0));
            if !p.is_null() {
                let w = p.cast::<u16>();
                let mut len = 0;
                while *w.add(len) != 0 {
                    len += 1;
                }
                out = Some(String::from_utf16_lossy(std::slice::from_raw_parts(w, len)));
                let _ = GlobalUnlock(HGLOBAL(h.0));
            }
        }
        let _ = CloseClipboard();
        out
    }
}

/// Écrase le presse-papiers texte. Le destructif `EmptyClipboard` n'est
/// appelé QUE depuis `selection()`, qui ne restaure/écrase que si un texte
/// existait déjà (une copie de fichiers n'est jamais détruite à plat).
fn clip_set(s: &str) {
    if !clip_open() {
        return;
    }
    unsafe {
        let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
        let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else {
            let _ = CloseClipboard();
            return;
        };
        let p = GlobalLock(h);
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p.cast::<u16>(), wide.len());
            let _ = GlobalUnlock(h);
            let _ = EmptyClipboard();
            if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(h.0))) {
                crate::dlog::dlog(&format!("def: SetClipboardData {e}"));
            }
        }
        // GlobalLock nul : ~64 o jamais libérés (windows-rs n'expose pas
        // GlobalFree) — cas d'échec extrême, non dégrader
        let _ = CloseClipboard();
    }
}

/// Sélection courante de l'app cible : Ctrl+C injecté, attente d'un
/// changement du presse-papiers (≤ 240 ms). Inchangé = pas de sélection
/// réelle (clic sans texte, terminal qui avale Ctrl+C…) → `None`.
pub fn selection() -> Option<String> {
    let before = clip_get();
    let mut eng = Enigo::new(&Settings::default()).ok()?;
    // Release TOUJOURS exécuté : ni `?` ni and_then entre Press et Release
    // (and_then sauterait le Release sur erreur → Ctrl coincé à vie)
    let r = eng.key(Key::Control, Direction::Press);
    let r = r.and(eng.key(Key::C, Direction::Click));
    let r = r.and(eng.key(Key::Control, Direction::Release));
    if let Err(e) = r {
        crate::dlog::dlog(&format!("def: injection Ctrl+C {e}"));
    }
    // 400 ms : Chromium met 250-400 ms à figer la copie — 240 ms (essai plus
    // court) faisait « inchangé » à chaque fois, la sélection n'arrivait jamais
    let (mut same, mut vide) = (0, 0);
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(20));
        match clip_get() {
            Some(t) if Some(&t) != before.as_ref() => {
                if let Some(b) = &before {
                    clip_set(b);
                }
                return Some(t);
            }
            // aucun changement : distinguer « l'app n'a rien copié »
            // (same) de « presse-papiers inaccessible » (vide)
            Some(_) => same += 1,
            None => vide += 1,
        }
    }
    crate::dlog::dlog(&format!(
        "def : inchangé (fg={:?} avant={} {same}×même {vide}×vide)",
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize,
        before.as_ref().map_or(0, String::len)
    ));
    None
}

/// Le worker déclenché par le hook souris (un seul à la fois via BUSY).
/// `epoch_up` = epoch au relâchement du clic déclencheur.
pub fn worker(epoch_up: u64) {
    let _g = BusyGuard;
    // l'utilisateur a frappé/cliqué entre le geste et ce thread : il a repris
    // la main — injecter Ctrl+C dans sa frappe lui mangerait des lettres
    if current() != epoch_up {
        crate::dlog::dlog("def : annulé (interaction avant le worker)");
        return;
    }
    let Some(sel) = selection() else {
        crate::dlog::dlog("def : pas de sélection (presse-papiers inchangé)");
        return;
    };
    let Some(mot) = clean(&sel) else {
        crate::dlog::dlog(&format!("def : sélection ignorée {sel:?}"));
        return;
    };
    // epoch APRÈS selection() : nos propres injections Ctrl+C ont bumpé
    // l'epoch — capturé avant, l'app jetterait son propre résultat
    let epoch = current();
    let cached = LAST.lock().unwrap().clone();
    let text = match cached {
        Some((m, t)) if m == mot => t,
        _ => {
            let Some(t) = lookup_souple(&mot) else {
                crate::dlog::dlog(&format!("def : introuvable {mot}"));
                return;
            };
            *LAST.lock().unwrap() = Some((mot.clone(), t.clone()));
            t
        }
    };
    if crate::hook::is_paused() {
        return;
    }
    crate::dlog::dlog(&format!("def : {mot} → {text}"));
    *DEF.lock().unwrap() = Some((epoch, mot, text));
    crate::hook::notify(crate::hook::KeyMsg::Def);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_selection() {
        assert_eq!(clean("  albâtre\n").as_deref(), Some("albâtre"));
        assert_eq!(clean("« albâtre, »").as_deref(), Some("albâtre"));
        assert_eq!(clean("d'albâtre.").as_deref(), Some("d'albâtre"));
        assert_eq!(clean("« âtre »").as_deref(), Some("âtre"));
        assert_eq!(clean("le  vent\nbalayait").as_deref(), Some("le vent"));
        assert_eq!(clean(""), None);
        assert_eq!(clean("   "), None);
        assert_eq!(clean("."), None);
        // limite : le mot le plus long du français reste cherchable
        assert_eq!(
            clean("anticonstitutionnellement").as_deref(),
            Some("anticonstitutionnellement")
        );
        assert_eq!(
            clean("protézokératoconjonctivites").as_deref(),
            Some("protézokératoconjonctivites")
        );
        // phrase surlignée entière : rejetée (exclusion par longueur)
        assert_eq!(clean(&"mot ".repeat(20)), None);
        assert_eq!(clean(&"x".repeat(33)), None);
    }

    #[test]
    fn extract_premiere_definition_fr() {
        let wiki = "\
'''albâtre''' {{pron|al.bɑ̃tʁ|fr}}
=== {{S|nom|en}} ===
# [[alabaster]] (à ignorer)
=== {{S|étymologie|fr}} ===
* Du latin [[alabaster]].
=== {{S|nom|fr}} ===
#''(minéral)'' Roche [[carbonate de calcium|carbonate]] {{lexique|minéralogique|fr}} blanche et fine.
## Variante : [[albâtre du Nil]].
#* Citation à ignorer.
=== {{S|verbe|fr}} ===
# Conjugaison du verbe.
";
        assert_eq!(
            extract_def(wiki).as_deref(),
            Some("(minéral) Roche carbonate blanche et fine.")
        );
    }

    #[test]
    fn extract_sans_section_fr() {
        assert_eq!(
            extract_def("== {{S|nom|en}} ==\n# an alabaster stone\n"),
            None
        );
        assert_eq!(extract_def("{\"error\":{\"code\":\"missingtitle\"}}"), None);
    }

    #[test]
    fn wikitext_formatversion2() {
        assert_eq!(
            wikitext("{\"parse\":{\"title\":\"x\",\"wikitext\":\"# d\"}}").as_deref(),
            Some("# d")
        );
        assert_eq!(wikitext("{\"parse\":{\"missing\":true}}"), None);
        assert_eq!(wikitext("pas du json"), None);
    }

    #[test]
    fn candidats_souples() {
        // majuscule → secours minuscule (segments déjà couverts par le bas)
        assert_eq!(candidats("Vérité"), ["Vérité", "vérité"]);
        // apostrophe : l'article n'a pas de page, le mot si
        assert_eq!(candidats("l'humilité"), ["l'humilité", "humilité"]);
        assert_eq!(candidats("d'albâtre"), ["d'albâtre", "albâtre"]);
        // phrase surlignée : entier, minuscule, puis segments (plafond 5)
        assert_eq!(
            candidats("cherchent Vérité l'humilité"),
            [
                "cherchent Vérité l'humilité",
                "cherchent vérité l'humilité",
                "cherchent",
                "vérité",
                "humilité"
            ]
        );
        // mot seul déjà bon : pas de doublon, pas de déchet
        assert_eq!(candidats("albâtre"), ["albâtre"]);
        assert_eq!(candidats("ne"), ["ne"]);
    }

    #[test]
    fn encode_url() {
        assert_eq!(urlencode("albâtre"), "alb%C3%A2tre");
        assert_eq!(urlencode("lettre-AP_l3.0~"), "lettre-AP_l3.0~");
    }
}
