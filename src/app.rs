//! État principal : reçoit les frappes du hook, gère la bulle et le remplacement.

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use eframe::egui;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

use crate::config::{self, Config};
use crate::conj;
use crate::hook::{self, KeyMsg};
use crate::punct;
use crate::spell;

struct Popup {
    trigger: String,
    alts: Vec<String>,
    sel: usize,
    /// longueur du déclencheur tapé (caractères) pour le backspace
    trigger_len: usize,
    /// caractère délimiteur déjà envoyé à l'application (espace, virgule…)
    delim: char,
    capitalized: bool,
    /// définitions courtes des alternatives (index parallèle à `alts`, `None` si
    /// inconnue de `data/defs.txt` — silencieux, fréquent en spell/conj)
    defs: Vec<Option<String>>,
    /// la sélection vient de changer (↑↓) : le ScrollArea doit la recentrer
    /// au rendu du frame courant, puis le flag est remis à false
    scroll_to_sel: bool,
}

pub struct App {
    rx: Receiver<KeyMsg>,
    cfg: Arc<Config>,
    /// dico d'orthographe (chargé une fois, suggestions mises en cache)
    /// — `None` pendant la pause (mémoire libérée)
    spell: Option<spell::Spell>,
    /// table de conjugaison + règles de cohérence (`None` = fichier absent ou pause)
    conj: Option<conj::Conj>,
    buffer: String,
    popup: Option<Popup>,
    /// position logique demandée pour la bulle (dernier caret), recalée dans la
    /// zone de travail au rendu — la taille réelle n'est connue qu'alors
    bubble_pos: Option<egui::Pos2>,
    /// dernier caret en pixels bruts (avant /scale) : choisit le moniteur du
    /// clamp (3 écrans — le SPI_GETWORKAREA ne couvre que le primaire)
    caret_px: (i32, i32),
    /// buffer vidé par Ctrl+C/V : plus de contexte de phrase → la popup
    /// suivante ne doit pas deviner une majuscule (« Désargenté » à tort)
    post_invalidate: bool,
    enigo: Enigo,
    hidden_sent: bool,
    frames: u64,
    /// dernier état connu de `hook::is_paused` (détection de transition en `ui`)
    paused: bool,
}

impl App {
    pub fn new(rx: Receiver<KeyMsg>, cfg: Arc<Config>) -> App {
        App {
            rx,
            cfg,
            // chargement synchrone : 428k lignes ≈ quelques dizaines de ms, une seule fois
            spell: Some(spell::Spell::load()),
            // chargement synchrone : 272k formes, temps loggé dans plume.log
            conj: conj::Conj::load(),
            buffer: String::new(),
            popup: None,
            bubble_pos: None,
            caret_px: (0, 0),
            post_invalidate: false,
            enigo: Enigo::new(&Settings::default()).expect("enigo"),
            hidden_sent: false,
            frames: 0,
            // false = « pas encore en pause » : si la hotkey a basculé pendant le
            // chargement, le 1er pass détecte la transition et libère les dicos
            paused: false,
        }
    }

    fn on_msg(&mut self, msg: KeyMsg, ctx: &egui::Context) {
        crate::dlog::dlog(&format!("reçu {msg:?}"));
        match msg {
            KeyMsg::Invalidate => {
                // presse-papiers ou raccourci : on ne peut plus garantir que le buffer
                // est un suffixe exact du texte affiché → on vide (faux négatif accepté)
                self.buffer.clear();
                self.post_invalidate = true;
                self.close(ctx);
            }
            KeyMsg::Char(c) => {
                if self.popup.is_some() {
                    // l'utilisateur continue d'écrire : on abandonne le remplacement
                    // (prioritaire même sur une correction typographique)
                    self.close(ctx);
                }
                // le caractère vient d'être injecté par l'OS avant notre traitement :
                // on corrige d'abord, on ne push que le résultat final
                match punct::corrections(c, &self.buffer).as_slice() {
                    // annulation pure (double espace) : rien à pousser, pas de délimiteur
                    // à traiter — le 1er espace l'a déjà été
                    [punct::Fix::CancelLast] => {
                        self.backspaces(1);
                        return;
                    }
                    // le buffer suit l'app : `insert` contient déjà le caractère final
                    [punct::Fix::Rewrite { backspaces, insert }] => {
                        self.backspaces(*backspaces);
                        let _ = self.enigo.text(insert);
                        self.buffer.push_str(insert);
                    }
                    _ => self.buffer.push(c),
                }
                if self.buffer.chars().count() > 120 {
                    let keep: String = self.buffer.chars().skip(60).collect();
                    self.buffer = keep;
                }
                if config::is_delimiter(c) {
                    // priorité : tics/adjectifs > orthographe > conjugaison (règles
                    // A/B/C de conj.rs) > verbes synonymes conjugués — un mot inconnu
                    // du dico mais déclaré comme tic (ex. « c'est ») gagne, c'est
                    // voulu ; conjugaison et verbes n'agissent que si l'ortho se tait.
                    // Une seule popup par délimiteur.
                    // flag consommé ici (même sans popup) : le buffer reconstruit
                    // depuis fait foi pour les délimiteurs suivants
                    let fresh = std::mem::take(&mut self.post_invalidate);
                    let p = if let Some(m) = self.cfg.match_buffer(&self.buffer) {
                        crate::dlog::dlog(&format!(
                            "MATCH trigger={} len={}",
                            m.trigger, m.trigger_len
                        ));
                        let defs = m
                            .alts
                            .iter()
                            .map(|a| self.cfg.def(a).map(str::to_string))
                            .collect();
                        Some(Popup {
                            trigger: m.trigger,
                            alts: m.alts,
                            sel: 0,
                            trigger_len: m.trigger_len,
                            delim: c,
                            // post-collage : prefix vide = faux début de phrase
                            // (« vous etes » → « Désargenté ») → pas de majuscule
                            // devinée ; la frappe seule ne peut plus être lue
                            capitalized: !fresh && m.capitalized,
                            defs,
                            scroll_to_sel: false,
                        })
                    } else if let Some(p) = self.spell_popup(c, fresh) {
                        Some(p)
                    } else if let Some(p) = self.conj_popup(c) {
                        Some(p)
                    } else {
                        self.verb_popup(c)
                    };
                    if let Some(p) = p {
                        self.popup = Some(p);
                        hook::set_popup_open(true);
                        self.show_at_caret(ctx);
                    }
                }
            }
            KeyMsg::Backspace => {
                self.close(ctx);
                self.buffer.pop();
            }
            KeyMsg::Up => {
                if let Some(p) = &mut self.popup {
                    let n = p.alts.len();
                    p.sel = (p.sel + n - 1) % n;
                    p.scroll_to_sel = true;
                }
            }
            KeyMsg::Down => {
                if let Some(p) = &mut self.popup {
                    let n = p.alts.len();
                    p.sel = (p.sel + 1) % n;
                    p.scroll_to_sel = true;
                }
            }
            KeyMsg::Escape => self.close(ctx),
            KeyMsg::Return => {
                if let Some(p) = self.popup.take() {
                    hook::set_popup_open(false);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    self.replace(p);
                }
            }
            KeyMsg::Other => self.close(ctx),
        }
    }

    /// Bulle d'orthographe pour le mot que vient de terminer `c` (aucune si le mot
    /// est correct, trop court, ou présumé nom propre). `fresh` = buffer vidé par
    /// Ctrl+C/V : le préfixe vide ne prouve plus un début de phrase.
    fn spell_popup(&mut self, c: char, fresh: bool) -> Option<Popup> {
        let (mot, trigger_len) = spell::extract_word(&self.buffer)?;
        // ≥ 4 caractères (évite « de », « sur »…) ; ni chiffre, ni apostrophe/tiret :
        // mots.txt ne contient que des mots purs (0 apostrophe) → toute suggestion
        // sur une contraction (« d'une » → « dune ») serait un faux positif
        if trigger_len < 4 || !mot.chars().all(char::is_alphabetic) {
            return None;
        }
        let lower = mot.to_lowercase();
        // mêmes 4 lignes que config::match_buffer : duplication assumée (config hors périmètre)
        let start = self.buffer.chars().count() - trigger_len - 1;
        let prefix: String = self.buffer.chars().take(start).collect();
        let prefix = prefix.trim_end();
        let sentence_start = !fresh && (prefix.is_empty() || prefix.ends_with(['.', '!', '?']));
        let capitale = mot.chars().next()?.is_uppercase();
        // heuristique nom propre (shortcut: « Yanis » en milieu de phrase ne doit pas
        // proposer « anis ») : majuscule ailleurs qu'en début de phrase → on saute
        if mot != lower && !(capitale && sentence_start) {
            return None;
        }
        let mut alts = self.spell.as_mut()?.suggest(&lower);
        if alts.is_empty() {
            return None;
        }
        let capitalized = sentence_start || capitale;
        if capitalized {
            // idempotent : l'UI et `replace` re-capitalisent aussi
            alts = alts.iter().map(|a| config::capitalize(a)).collect();
        }
        crate::dlog::dlog(&format!(
            "SPELL trigger={mot} alts={alts:?} phrase={sentence_start}"
        ));
        let defs = alts
            .iter()
            .map(|a| self.cfg.def(a).map(str::to_string))
            .collect();
        Some(Popup {
            trigger: mot,
            alts,
            sel: 0,
            trigger_len,
            delim: c,
            capitalized,
            defs,
            scroll_to_sel: false,
        })
    }

    /// Popup de cohérence grammaticale (règles A/B/C, voir conj.rs) : le mot
    /// vient d'être terminé par `c`. Même schéma que `spell_popup`, une seule
    /// alternative proposée (la correction), sinon `None`.
    fn conj_popup(&mut self, c: char) -> Option<Popup> {
        let (mot, trigger_len) = spell::extract_word(&self.buffer)?;
        let total = self.buffer.chars().count();
        let prefix: String = self.buffer.chars().take(total - trigger_len - 1).collect();
        let alt = self.conj.as_ref()?.suggest(&prefix, &mot)?;
        let capitalized = mot.chars().next().is_some_and(char::is_uppercase);
        crate::dlog::dlog(&format!("CONJ trigger={mot} alt={alt}"));
        let defs = vec![self.cfg.def(&alt).map(str::to_string)];
        Some(Popup {
            trigger: mot,
            alts: vec![alt],
            sel: 0,
            trigger_len,
            delim: c,
            capitalized,
            defs,
            scroll_to_sel: false,
        })
    }

    /// Synonymes conjugués du verbe tapé (verbes.txt + Conj::variantes) :
    /// « je la regarde » → « j'observe… » au même temps/personne. Même schéma
    /// que `conj_popup`, plusieurs alternatives, `None` si rien de sûr.
    fn verb_popup(&mut self, c: char) -> Option<Popup> {
        let (mot, trigger_len) = spell::extract_word(&self.buffer)?;
        let total = self.buffer.chars().count();
        let prefix: String = self.buffer.chars().take(total - trigger_len - 1).collect();
        let inf = self.conj.as_ref()?.infinitif_de(&mot)?;
        let syns = self.cfg.verb_alts(&inf)?;
        let alts = self.conj.as_ref()?.variantes(&prefix, &mot, syns);
        if alts.is_empty() {
            return None;
        }
        let capitalized = mot.chars().next().is_some_and(char::is_uppercase);
        crate::dlog::dlog(&format!("VERBES trigger={mot} inf={inf} alts={alts:?}"));
        let defs = alts
            .iter()
            .map(|a| self.cfg.def(a).map(str::to_string))
            .collect();
        Some(Popup {
            trigger: mot,
            alts,
            sel: 0,
            trigger_len,
            delim: c,
            capitalized,
            defs,
            scroll_to_sel: false,
        })
    }

    fn replace(&mut self, p: Popup) {
        let mut alt = p.alts[p.sel].clone();
        if p.capitalized {
            alt = config::capitalize(&alt);
        }
        crate::dlog::dlog(&format!("replace '{}' -> '{}{}'", p.trigger, alt, p.delim));
        // déclencheur + délimiteur déjà présents dans l'application
        self.backspaces(p.trigger_len + 1);
        let _ = self.enigo.text(&alt);
        std::thread::sleep(Duration::from_millis(2));
        let _ = self.enigo.text(&p.delim.to_string());
        // nos envois sont "injected" (ignorés par le hook) : on recalcule le buffer
        // à la main — vider perdait le contexte de phrase et le match suivant
        // voyait un début de phrase → majuscule à tort (« Splendide »)
        self.buffer = after_replace(&self.buffer, p.trigger_len + 1, &alt, p.delim);
    }

    /// Simule `n` backspaces dans l'application cible (déjà frappés avant notre traitement).
    fn backspaces(&mut self, n: usize) {
        for _ in 0..n {
            let _ = self.enigo.key(Key::Backspace, Direction::Click);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn show_at_caret(&mut self, ctx: &egui::Context) {
        let (x, y) = caret_pos();
        self.caret_px = (x, y); // brute : sert à choisir le moniteur du clamp
        let scale = ctx.native_pixels_per_point().unwrap_or(1.0);
        crate::dlog::dlog(&format!("show caret=({x},{y}) scale={scale}"));
        let pos = egui::pos2(x as f32 / scale, (y as f32 + 8.0) / scale);
        self.bubble_pos = Some(pos);
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    }

    fn close(&mut self, ctx: &egui::Context) {
        if self.popup.take().is_some() {
            hook::set_popup_open(false);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }
}

/// Buffer après un remplacement : reste un suffixe exact du texte de l'app
/// (retranche `deleted` caractères, ajoute `alt` + délimiteur). Coupure en
/// chars : le déclencheur peut contenir des accents (« allé »).
fn after_replace(buffer: &str, deleted: usize, alt: &str, delim: char) -> String {
    let keep = buffer.chars().count().saturating_sub(deleted);
    let mut s: String = buffer.chars().take(keep).collect();
    s.push_str(alt);
    s.push(delim);
    s
}

/// Cascade de localisation du caret (chaque échec → étape suivante) :
/// GUITHREADINFO du thread de premier plan → UI Automation → souris.
/// Retourne des pixels écran logiques (contrat de `show_at_caret`, qui divise
/// par `native_pixels_per_point`).
fn caret_pos() -> (i32, i32) {
    if let Some(pt) = caret_gui() {
        crate::dlog::dlog(&format!("caret=gui({},{})", pt.0, pt.1));
        return pt;
    }
    if let Some(pt) = caret_uia() {
        crate::dlog::dlog(&format!("caret=uia({},{})", pt.0, pt.1));
        return pt;
    }
    let pt = cursor_pos();
    crate::dlog::dlog(&format!("caret=souris({},{})", pt.0, pt.1));
    pt
}

/// Étape 1 : caret Win32 classique, avec le tid du premier plan
/// (plus fiable que 0 = thread courant = notre propre fenêtre sans caret).
fn caret_gui() -> Option<(i32, i32)> {
    unsafe {
        let tid = windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow(),
            None,
        );
        let mut gi: windows::Win32::UI::WindowsAndMessaging::GUITHREADINFO = std::mem::zeroed();
        gi.cbSize =
            std::mem::size_of::<windows::Win32::UI::WindowsAndMessaging::GUITHREADINFO>() as u32;
        if windows::Win32::UI::WindowsAndMessaging::GetGUIThreadInfo(tid, &mut gi).is_ok()
            && gi.rcCaret.left != gi.rcCaret.right
        {
            return Some(((gi.rcCaret.left + gi.rcCaret.right) / 2, gi.rcCaret.bottom));
        }
        None
    }
}

/// Étape 2 : UI Automation, seul moyen de voir le caret des apps modernes
/// (Notepad Win11 = WinUI3, Word, navigateurs) qui ne remplissent pas `rcCaret`.
/// Échec silencieux (app sans TextPattern : terminal…) → étape suivante.
///
/// Piège DPI : les rects UIA sont en pixels écran logiques, ce qui est
/// exactement ce que `show_at_caret` attend — pas de scaling ici.
fn caret_uia() -> Option<(i32, i32)> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::System::Ole::{
        SafeArrayAccessData, SafeArrayDestroy, SafeArrayUnaccessData,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
    };
    use windows::core::Interface;

    // tout le chemin COM/UIA est unsafe, aucune panique : `?` → étape suivante
    unsafe {
        // S_FALSE = déjà initialisé, RPC_E_CHANGED_MODE = déjà initialisé en MTA :
        // on continue dans les deux cas (si la COM est réellement indisponible,
        // CoCreateInstance échoue plus bas et on retombe sur la souris)
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let auto: IUIAutomation = CoCreateInstance(
            &CUIAutomation,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .ok()?;
        // chaque échec est loggé : plume.log dit alors quelle étape échoue
        // dans l'app de l'utilisateur (raison du fallback souris)
        let el = match auto.GetFocusedElement() {
            Ok(el) => el,
            Err(e) => {
                crate::dlog::dlog(&format!("uia: GetFocusedElement → {e}"));
                return None;
            }
        };
        let pat = match el.GetCurrentPattern(UIA_TextPatternId) {
            Ok(p) => p,
            Err(_) => {
                crate::dlog::dlog("uia: élément focalisé sans TextPattern");
                return None;
            }
        };
        let tp: IUIAutomationTextPattern = pat.cast().ok()?;
        let ranges = match tp.GetSelection() {
            Ok(r) => r,
            Err(e) => {
                crate::dlog::dlog(&format!("uia: GetSelection → {e}"));
                return None;
            }
        };
        let range = ranges.GetElement(0).ok()?;
        // SAFEARRAY de f64 [x, y, l, h] par rect ; le caret est un rect de
        // largeur nulle, on prend le premier rect retourné
        let sa = range.GetBoundingRectangles().ok()?;

        let mut out: Option<(i32, i32)> = None;
        let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
        if SafeArrayAccessData(sa, &mut data).is_ok() {
            // stride lisible même si la crate change de représentation
            let stride = (*sa).cbElements as usize;
            let count = (*sa).rgsabound[0].cElements as usize;
            if (*sa).cDims == 1 && count >= 4 && stride >= 8 {
                let get = |i: usize| {
                    std::ptr::read_unaligned(data.cast::<u8>().add(i * stride).cast::<f64>())
                };
                // bas-gauche du rect = où se trouve le caret
                out = Some((get(0) as i32, (get(1) + get(3)) as i32));
            }
            let _ = SafeArrayUnaccessData(sa);
        }
        let _ = SafeArrayDestroy(sa);
        // (0,0) = sélection dégénérée renvoyée par certaines apps (vu en prod) :
        // mieux vaut le fallback souris qu'une bulle en haut à gauche
        out.filter(|&pt| pt != (0, 0))
    }
}

fn cursor_pos() -> (i32, i32) {
    unsafe {
        let mut pt = windows::Win32::Foundation::POINT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
        (pt.x, pt.y)
    }
}

/// Zone de travail (écran − taskbar) du moniteur le plus proche de `pt` (le
/// caret), renvoyée en bornes (gauche, haut, droite, bas) de la MÊME unité que
/// `bubble_pos` (division par `scale`, comme avant avec SPI_GETWORKAREA).
// shortcut: coordonnées physiques/logiques et scales hétérogènes entre écrans =
// approximation (MONITOR_DEFAULTTONEAREST reste juste au voisinage d'un bord) ;
// échec API = bornes infinies → pas de clamp plutôt qu'un faux clamp
fn work_area(pt: (i32, i32), scale: f32) -> (f32, f32, f32, f32) {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
    };
    unsafe {
        let h = MonitorFromPoint(POINT { x: pt.0, y: pt.1 }, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(h, &mut mi).as_bool() {
            return (f32::MIN, f32::MIN, f32::MAX, f32::MAX);
        }
        let r = mi.rcWork;
        (
            r.left as f32 / scale,
            r.top as f32 / scale,
            r.right as f32 / scale,
            r.bottom as f32 / scale,
        )
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // opaque et identique à la bulle : la transparence DWM rend un carré blanc
        // sur Windows 11 → la fenêtre est exactement la bulle, sans pixel alpha 0
        [24.0 / 255.0, 24.0 / 255.0, 32.0 / 255.0, 1.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // canal du menu tray (pause, quitter…) à vider à chaque frame
        crate::tray::poll();
        let ctx = ui.ctx().clone();
        self.frames += 1;
        if self.frames == 1 {
            // OnceLock : le hook réveille le rendu à chaque frappe utile envoyée
            hook::set_repaint_ctx(ctx.clone());
        }
        if self.frames % 100 == 1 {
            crate::dlog::dlog(&format!("ui pass #{}", self.frames));
        }
        if !self.hidden_sent {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.hidden_sent = true;
        }
        // pause ⇄ reprise : les deux chemins (hotkey Ctrl+Alt+P, menu tray) passent
        // par toggle_paused → Invalidate ; `close` ici est le garde-fou si un chemin
        // basculait PAUSED sans Invalidate (bulle ne doit jamais rester orpheline)
        let paused = hook::is_paused();
        if paused != self.paused {
            self.close(&ctx);
            if paused {
                crate::dlog::dlog(&format!(
                    "pause : libéré spell={} conj={}",
                    self.spell.is_some(),
                    self.conj.is_some()
                ));
                self.conj = None;
                self.spell = None;
            } else {
                // reprise synchrone : ~700 ms de freeze voulu (rechargement d'un coup)
                let t = std::time::Instant::now();
                self.conj = conj::Conj::load();
                self.spell = Some(spell::Spell::load());
                crate::dlog::dlog(&format!("reprise : dicos rechargés en {:?}", t.elapsed()));
            }
            self.paused = paused;
        }
        while let Ok(msg) = self.rx.try_recv() {
            self.on_msg(msg, &ctx);
        }

        let mut clicked: Option<usize> = None;
        // lu avant les emprunts de rendu : consommé dans le même frame
        let follow = self.popup.as_ref().is_some_and(|p| p.scroll_to_sel);
        let mut bubble_size: Option<egui::Vec2> = None;
        if let Some(p) = &self.popup {
            let frame = egui::Frame::new()
                .fill(egui::Color32::from_rgba_unmultiplied(24, 24, 32, 245))
                .corner_radius(10.0)
                .inner_margin(egui::Margin::same(10))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 12,
                    spread: 0,
                    color: egui::Color32::BLACK.linear_multiply(0.4),
                })
                .show(ui, |ui| {
                    ui.set_max_width(280.0);
                    // le déclencheur reste le sujet de la bulle (la déf vit
                    // sur les alternatives, voir plus bas)
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("« {} »", p.trigger))
                                .weak()
                                .italics(),
                        )
                        .wrap(),
                    );
                    ui.add_space(2.0);
                    // hauteur plafonnée ≈ 7 lignes à 17px ; auto_shrink (défaut)
                    // suit le contenu en dessous → pas de boîte vide
                    egui::ScrollArea::vertical()
                        .max_height(210.0)
                        .show(ui, |ui| {
                            for (i, alt) in p.alts.iter().enumerate() {
                                let sel = i == p.sel;
                                // casse proposée selon le contexte (début de phrase détecté)
                                let alt = if p.capitalized {
                                    config::capitalize(alt)
                                } else {
                                    alt.clone()
                                };
                                let resp = ui.selectable_label(
                                    sel,
                                    egui::RichText::new(format!("{}. {}", i + 1, alt)).size(17.0),
                                );
                                if follow && sel {
                                    resp.scroll_to_me(Some(egui::Align::Center));
                                }
                                if resp.clicked() {
                                    clicked = Some(i);
                                }
                                // la déf porte sur l'alternative proposée, pas sur le mot tapé
                                if let Some(def) = p.defs.get(i).and_then(|d| d.as_deref()) {
                                    // ≤ 2 lignes (~6,5 px/car. à 11 px), 70 car. max
                                    let max = ((ui.available_width() * 2.0 / 6.5) as usize).min(70);
                                    if max >= 10 {
                                        let short: String = def.chars().take(max).collect();
                                        let short = if def.chars().count() > max {
                                            format!("{short}…")
                                        } else {
                                            short
                                        };
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(short)
                                                    .size(11.0)
                                                    .italics()
                                                    .weak(),
                                            )
                                            .wrap(),
                                        );
                                    }
                                }
                            }
                        });
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("↑↓ naviguer · Entrée valider · Échap fermer")
                            .weak()
                            .size(11.0),
                    );
                });
            // la fenêtre épouse la bulle : sinon elle garde sa taille par défaut (~800×600)
            // et affiche un gros carré opaque autour
            bubble_size = Some(frame.response.rect.size());
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(frame.response.rect.size()));
        } else {
            ui.allocate_space(egui::Vec2::ZERO);
        }

        // la bulle ne doit jamais sortir de la zone de travail (bas d'écran,
        // taskbar) DU MONITEUR DU CARET (3 écrans) ; sa taille réelle n'est
        // connue qu'après le rendu, d'où ce recalage ici
        if let (Some(pos), Some(size)) = (self.bubble_pos, bubble_size) {
            let scale = ctx.native_pixels_per_point().unwrap_or(1.0);
            let (wl, wt, wr, wb) = work_area(self.caret_px, scale);
            let mut p = pos;
            if p.x + size.x > wr - 4.0 {
                p.x = (wr - 4.0 - size.x).max(wl);
            }
            if p.y + size.y > wb - 4.0 {
                p.y = (wb - 4.0 - size.y).max(wt);
            }
            // écran à gauche/dessus du primaire : bornes négatives, pas de .max(0)
            if p.x < wl {
                p.x = wl;
            }
            if p.y < wt {
                p.y = wt;
            }
            if p != pos {
                crate::dlog::dlog(&format!("bulle clampée {pos} -> {p} (taille {size})"));
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(p));
                self.bubble_pos = Some(p);
            }
        }

        // ordre voulu : messages → rendu → reset (le flag monté dans `on_msg`
        // ce frame est consommé au rendu ci-dessus)
        if let Some(p) = &mut self.popup {
            p.scroll_to_sel = false;
        }

        if let Some(i) = clicked
            && let Some(mut p) = self.popup.take()
        {
            hook::set_popup_open(false);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            p.sel = i;
            self.replace(p);
        }

        // le hook réveille le rendu lui-même à chaque message (set_repaint_ctx) :
        // ce filet ne sert qu'au démarrage et aux événements tray (clic menu →
        // réaction ≤ 250 ms, acceptable), soit 4 réveils/s max au repos
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Tic;

    /// Bug « elle est allée Splendide » : vider le buffer après un remplacement
    /// faisait perdre le contexte de phrase → le match suivant forçait la majuscule.
    #[test]
    fn replace_garde_le_contexte_de_phrase() {
        // suppression en chars : « allé » (4) + délimiteur, comme trigger_len + 1
        let b = after_replace("elle est allé ", "allé".chars().count() + 1, "allée", ' ');
        assert_eq!(b, "elle est allée ");
        // saturating : buffer déjà plus court que la suppression (Invariant rompu)
        assert_eq!(after_replace("ok ", 99, "alt", ' '), "alt ");

        let cfg = Config {
            tics: vec![Tic {
                trigger: "super".into(),
                alts: vec!["splendide".into()],
            }],
            adjs: vec![],
            verbs: vec![],
            defs: std::collections::HashMap::new(),
        };
        let mut typed = b;
        typed.push_str("super ");
        let m = cfg.match_buffer(&typed).unwrap();
        assert!(!m.capitalized, "milieu de phrase : pas de majuscule forcée");
    }
}
