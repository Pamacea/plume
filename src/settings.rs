//! Paramètres persistés (`plume.json`) et fenêtre éponyme.

use crate::hook::KeyMsg;
use crate::theme;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Même répertoire courant que `tics.txt`.
const FILE: &str = "plume.json";

/// Liste fermée : une touche de texte volerait la frappe au hook.
pub const KEYS: [(&str, u32); 7] = [
    ("Entrée", 0x0D),
    ("Tab", 0x09),
    ("Échap", 0x1B),
    ("↑", 0x26),
    ("↓", 0x28),
    ("←", 0x25),
    ("→", 0x27),
];

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub tics: bool,
    pub pause: String,
    pub validate: String,
    pub close: String,
    pub next: String,
    pub prev: String,
    pub alt_chars: Vec<(String, String)>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tics: true,
            pause: "P".into(),
            validate: "Entrée".into(),
            close: "Échap".into(),
            next: "↓".into(),
            prev: "↑".into(),
            alt_chars: Vec::new(),
        }
    }
}

static TICS: AtomicBool = AtomicBool::new(true);
static VALIDATE: AtomicU32 = AtomicU32::new(0x0D);
static CLOSE: AtomicU32 = AtomicU32::new(0x1B);
static NEXT: AtomicU32 = AtomicU32::new(0x28);
static PREV: AtomicU32 = AtomicU32::new(0x26);
// `RwLock::new(Vec::new())` est const, contrairement à `HashMap::new()`.
static ALT: RwLock<Vec<(u32, char)>> = RwLock::new(Vec::new());

/// VK du hotkey global de pause (Ctrl+Alt+touche), publié par `publish`
static PAUSE_VK: AtomicU32 = AtomicU32::new(b'P' as u32);

pub fn pause_vk() -> u32 {
    PAUSE_VK.load(Ordering::Relaxed)
}

/// VK d'une touche assignable : un seul caractère ASCII alphanumérique.
fn key_vk(name: &str) -> Option<u32> {
    let mut chars = name.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    // Les VK des lettres et chiffres coïncident avec leur code ASCII majuscule.
    Some(match c {
        ',' => 0xBC, // VK_OEM_COMMA (AZERTY : « , »)
        ';' => 0xBA, // VK_OEM_1 (AZERTY : « ; »)
        c if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as u32,
        _ => return None,
    })
}

fn publish(s: &Settings) {
    TICS.store(s.tics, Ordering::Relaxed);
    VALIDATE.store(vk_for(&s.validate, 0x0D), Ordering::Relaxed);
    CLOSE.store(vk_for(&s.close, 0x1B), Ordering::Relaxed);
    NEXT.store(vk_for(&s.next, 0x28), Ordering::Relaxed);
    PREV.store(vk_for(&s.prev, 0x26), Ordering::Relaxed);
    PAUSE_VK.store(
        key_vk(&s.pause).unwrap_or(u32::from(b'P')),
        Ordering::Relaxed,
    );
    let mut alt = Vec::with_capacity(s.alt_chars.len());
    for (key, value) in &s.alt_chars {
        // Ligne invalide ignorée sans bruit.
        if let (Some(vk), Some(ch)) = (key_vk(key), value.chars().next()) {
            alt.push((vk, ch));
        }
    }
    if let Ok(mut guard) = ALT.write() {
        *guard = alt;
    }
}

pub fn load() -> Settings {
    std::fs::read_to_string(FILE)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

impl Settings {
    pub fn save(&self) {
        let Ok(json) = serde_json::to_string_pretty(self) else {
            crate::dlog::dlog("settings: JSON impossible");
            return;
        };
        // tmp + rename : un arrêt en cours d'écriture ne doit pas tronquer le fichier
        // (un JSON invalide retomberait silencieusement sur les défauts)
        let tmp = format!("{FILE}.tmp");
        if let Err(e) = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, FILE)) {
            crate::dlog::dlog(&format!("settings: écriture {FILE}: {e}"));
        }
    }

    pub fn apply(&self) {
        let old = PAUSE_VK.load(Ordering::Relaxed);
        publish(self);
        // le hotkey vit sur le thread du hook : il doit le réenregistrer lui-même
        if PAUSE_VK.load(Ordering::Relaxed) != old {
            crate::hook::reload_pause_hotkey();
        }
        self.save();
    }
}

pub fn vk_for(name: &str, default: u32) -> u32 {
    KEYS.iter()
        .find(|(n, _)| *n == name)
        .map(|(_, vk)| *vk)
        .unwrap_or(default)
}

/// Nom affichable d'une touche : aucune police egui ne contient les flèches
/// (↑ ↓ ← → s'affichent en carrés vides) → libellé texte, valeur stockée intacte.
pub fn key_label(name: &str) -> &str {
    match name {
        "↑" => "Haut",
        "↓" => "Bas",
        "←" => "Gauche",
        "→" => "Droite",
        other => other,
    }
}

pub fn init(s: &Settings) {
    publish(s);
}

pub fn tics() -> bool {
    TICS.load(Ordering::Relaxed)
}

pub fn remap(vk: u32) -> Option<KeyMsg> {
    // L'ordre fait foi si deux actions partagent une touche.
    if vk == VALIDATE.load(Ordering::Relaxed) {
        Some(KeyMsg::Return)
    } else if vk == CLOSE.load(Ordering::Relaxed) {
        Some(KeyMsg::Escape)
    } else if vk == NEXT.load(Ordering::Relaxed) {
        Some(KeyMsg::Down)
    } else if vk == PREV.load(Ordering::Relaxed) {
        Some(KeyMsg::Up)
    } else {
        None
    }
}

pub fn alt_char(vk: u32) -> Option<char> {
    ALT.read()
        .ok()
        .and_then(|alt| alt.iter().find(|(k, _)| *k == vk).map(|(_, c)| *c))
}

#[derive(Default)]
pub struct State {
    pub tab: Tab,
    new_key: String,
    new_char: String,
    recording: bool,
    pause_recording: bool,
    native_edit: Vec<(String, String)>,
}

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Tab {
    #[default]
    General,
    Chars,
    Keys,
}

/// Lettre ou chiffre d'une touche egui, en majuscule.
fn captured_char(key: egui::Key) -> Option<char> {
    let name = key.name();
    let mut it = name.chars();
    let c = it.next()?;
    (it.next().is_none() && c.is_ascii_alphanumeric()).then(|| c.to_ascii_uppercase())
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(title)
            .size(11.0)
            .color(theme::MUTED)
            .strong(),
    );
    ui.add_space(2.0);
}

fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn kbd(ui: &mut egui::Ui, label: &str) {
    egui::Frame::new()
        .fill(theme::INPUT)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(7, 3))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).size(12.0).color(theme::TEXT));
        });
}

pub fn window(ui: &mut egui::Ui, s: &mut Settings, st: &mut State) {
    ui.scope(|ui| {
        ui.visuals_mut().selection.bg_fill = theme::INPUT;
        ui.horizontal(|ui| {
            if ui
                .selectable_label(
                    st.tab == Tab::General,
                    egui::RichText::new("Général").size(14.0),
                )
                .clicked()
            {
                st.tab = Tab::General;
            }
            if ui
                .selectable_label(
                    st.tab == Tab::Chars,
                    egui::RichText::new("Caractères spéciaux").size(14.0),
                )
                .clicked()
            {
                st.tab = Tab::Chars;
            }
            if ui
                .selectable_label(
                    st.tab == Tab::Keys,
                    egui::RichText::new("Raccourcis de la bulle").size(14.0),
                )
                .clicked()
            {
                st.tab = Tab::Keys;
            }
        });
    });
    ui.separator();

    match st.tab {
        Tab::General => general(ui, s, st),
        Tab::Chars => chars(ui, s, st),
        Tab::Keys => keys(ui, s),
    }
}

fn general(ui: &mut egui::Ui, s: &mut Settings, st: &mut State) {
    if st.pause_recording {
        ui.ctx().request_repaint();
        let key = ui.ctx().input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    ..
                } => Some(*key),
                _ => None,
            })
        });
        if let Some(key) = key {
            if key == egui::Key::Escape {
                st.pause_recording = false;
            } else if let Some(c) = captured_char(key) {
                s.pause = c.to_string();
                st.pause_recording = false;
                s.apply();
            }
        }
    }

    card(ui, |ui| {
        section(ui, "APPLICATION");
        // Relu chaque frame : l'état peut changer hors de l'application.
        let mut autostart = crate::tray::autostart_enabled();
        if theme::check_row(ui, &mut autostart, "Démarrage automatique", 0.0) {
            crate::tray::set_autostart(autostart);
        }
        if theme::check_row(ui, &mut s.tics, "Suggestions de tics et d'adjectifs", 0.0) {
            s.apply();
        }
    });

    ui.add_space(8.0);
    card(ui, |ui| {
        section(ui, "ÉTAT");
        ui.label(
            egui::RichText::new(format!("Ctrl + Alt + {} bascule aussi la pause.", s.pause))
                .size(11.0)
                .color(theme::MUTED),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let paused = crate::hook::is_paused();
            let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(
                rect.center(),
                4.0,
                if paused { theme::WARN } else { theme::OK },
            );
            ui.label(
                egui::RichText::new(if paused { "En pause" } else { "Actif" })
                    .size(13.0)
                    .color(theme::TEXT),
            );
            // état à gauche, action à l'extrême droite
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if paused {
                    "Reprendre"
                } else {
                    "Mettre en pause"
                };
                if ui.button(label).clicked() {
                    crate::hook::toggle_paused();
                }
            });
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Raccourci de pause").color(theme::TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (bg, border, label) = if st.pause_recording {
                    ui.ctx().request_repaint();
                    let a = 0.45 + 0.55 * (ui.ctx().input(|i| i.time) * 4.0).sin().abs() as f32;
                    (
                        egui::Color32::from_rgb(0x3A, 0x3A, 0x48),
                        egui::Color32::WHITE.gamma_multiply(a),
                        "Appuyez sur une touche…".to_owned(),
                    )
                } else {
                    (
                        theme::INPUT,
                        theme::BORDER,
                        format!("Ctrl + Alt + {}", s.pause),
                    )
                };
                if ui
                    .add_sized(
                        egui::vec2(190.0, 36.0),
                        egui::Button::new(egui::RichText::new(label).size(12.0).color(theme::TEXT))
                            .fill(bg)
                            .stroke(egui::Stroke::new(1.0, border)),
                    )
                    .clicked()
                {
                    st.pause_recording = true;
                }
            });
        });
    });
}

fn chars(ui: &mut egui::Ui, s: &mut Settings, st: &mut State) {
    if st.recording {
        ui.ctx().request_repaint();
        let key = ui.ctx().input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    ..
                } => Some(*key),
                _ => None,
            })
        });
        if let Some(key) = key {
            if key == egui::Key::Escape {
                st.recording = false;
            } else if let Some(c) = captured_char(key) {
                st.new_key = c.to_string();
                st.recording = false;
            }
        }
    }

    let mut changed = false;
    let mut remove = None;

    section(ui, "AJOUTER");
    card(ui, |ui| {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.horizontal(|ui| {
                let (h, char_w, add_w) = (36.0, 64.0, 116.0);
                let cap_w = (ui.available_width() - char_w - add_w - 16.0).max(120.0);
                let (bg, border, label) = if st.recording {
                    ui.ctx().request_repaint();
                    let a = 0.45 + 0.55 * (ui.ctx().input(|i| i.time) * 4.0).sin().abs() as f32;
                    (
                        egui::Color32::from_rgb(0x3A, 0x3A, 0x48),
                        egui::Color32::WHITE.gamma_multiply(a),
                        "Appuyez sur une touche…".to_owned(),
                    )
                } else if st.new_key.is_empty() {
                    (
                        theme::INPUT,
                        theme::BORDER,
                        "Cliquer pour capturer".to_owned(),
                    )
                } else {
                    (theme::INPUT, theme::BORDER, format!("Alt + {}", st.new_key))
                };
                let capture = ui.add_sized(
                    egui::vec2(cap_w, h),
                    egui::Button::new(egui::RichText::new(label).size(12.0).color(theme::TEXT))
                        .fill(bg)
                        .stroke(egui::Stroke::new(1.0, border)),
                );
                if capture.clicked() {
                    st.recording = true;
                }
                ui.add_sized(
                    egui::vec2(char_w, h),
                    egui::TextEdit::singleline(&mut st.new_char)
                        .hint_text("caractère")
                        .margin(egui::Margin::symmetric(8, 6)),
                );
                let can_add = !st.new_key.is_empty() && !st.new_char.is_empty();
                if ui
                    .add_enabled(
                        can_add,
                        egui::Button::new("+ Ajouter").min_size(egui::vec2(add_w, h)),
                    )
                    .clicked()
                {
                    if let Some(entry) = s
                        .alt_chars
                        .iter_mut()
                        .find(|(key, _)| key.eq_ignore_ascii_case(st.new_key.as_str()))
                    {
                        entry.1 = st.new_char.clone();
                    } else {
                        s.alt_chars.push((st.new_key.clone(), st.new_char.clone()));
                    }
                    st.new_key.clear();
                    st.new_char.clear();
                    st.recording = false;
                    changed = true;
                }
            });
        });
    });

    section(ui, "RACCOURCIS ACTIFS");
    card(ui, |ui| {
        ui.label(
            egui::RichText::new(
                "Personnalisés enregistrés dans plume.json — modifier un natif crée une surcharge.",
            )
            .size(11.0)
            .color(theme::MUTED),
        );
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                // lignes aérées : le statut « natif » et le × ne collent plus au
                // texte au-dessus ; la largeur de la barre est réservée à droite
                ui.spacing_mut().item_spacing.y = 12.0;
                let bar =
                    ui.spacing().scroll.bar_width + 2.0 * ui.spacing().scroll.bar_inner_margin;
                ui.set_max_width(ui.available_width() - bar);
                for (i, (key, value)) in s.alt_chars.iter_mut().enumerate() {
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.horizontal(|ui| {
                            kbd(ui, "Alt");
                            ui.label(egui::RichText::new("+").color(theme::MUTED));
                            kbd(ui, key);
                            ui.label(egui::RichText::new("»").color(theme::MUTED));
                            if ui
                                .add_sized(
                                    egui::vec2(52.0, 32.0),
                                    egui::TextEdit::singleline(value)
                                        .char_limit(1)
                                        .margin(egui::Margin::symmetric(8, 6)),
                                )
                                .changed()
                            {
                                changed = true;
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized(
                                            egui::vec2(28.0, 32.0),
                                            egui::Button::new(
                                                egui::RichText::new("×").color(theme::MUTED),
                                            )
                                            .frame(false),
                                        )
                                        .on_hover_text("Supprimer")
                                        .clicked()
                                    {
                                        remove = Some(i);
                                    }
                                },
                            );
                        });
                    });
                }
                // natifs non surchargés : éditer la cible crée une surcharge dans plume.json
                for &(key, _scan, shift, base, variant) in &crate::hook::NATIVE_ALT {
                    if s.alt_chars.iter().any(|(k, _)| k.eq_ignore_ascii_case(key)) {
                        continue;
                    }
                    if !st.native_edit.iter().any(|(k, _)| k == key) {
                        st.native_edit.push((key.to_string(), base.to_string()));
                    }
                    let idx = st
                        .native_edit
                        .iter()
                        .position(|(k, _)| k == key)
                        .expect("brouillon inséré juste au-dessus");
                    let orig = st.native_edit[idx].1.clone();
                    let mut ch = orig.clone();
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.horizontal(|ui| {
                            kbd(ui, "Alt");
                            if shift {
                                ui.label(egui::RichText::new("+").color(theme::MUTED));
                                kbd(ui, "Maj");
                            }
                            ui.label(egui::RichText::new("+").color(theme::MUTED));
                            kbd(ui, key);
                            ui.label(egui::RichText::new("»").color(theme::MUTED));
                            ui.add_sized(
                                egui::vec2(52.0, 32.0),
                                egui::TextEdit::singleline(&mut ch)
                                    .char_limit(1)
                                    .margin(egui::Margin::symmetric(8, 6)),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new("natif").size(10.0).color(theme::MUTED),
                                    );
                                    if let Some(v) = variant {
                                        ui.label(
                                            egui::RichText::new(format!("Maj → {v}"))
                                                .size(10.0)
                                                .color(theme::MUTED),
                                        );
                                    }
                                },
                            );
                        });
                    });
                    if ch != orig {
                        if !ch.is_empty() && ch != base.to_string() {
                            s.alt_chars.push((key.to_string(), ch));
                            st.native_edit.clear();
                            changed = true;
                        } else {
                            // vide = pas de surcharge : le natif reste actif, on réaffiche sa cible
                            st.native_edit[idx].1 =
                                if ch.is_empty() { base.to_string() } else { ch };
                        }
                    }
                }
            });
    });

    if let Some(i) = remove {
        s.alt_chars.remove(i);
        st.native_edit.clear();
        changed = true;
    }

    let invalid = s
        .alt_chars
        .iter()
        .any(|(key, value)| key_vk(key).is_none() || value.is_empty());
    if invalid {
        ui.label(
            egui::RichText::new(
                "touche = 1 lettre, 1 chiffre, « , » ou « ; » ; caractère = 1 symbole",
            )
            .size(11.0)
            .color(theme::WARN),
        );
    }
    if changed {
        s.apply();
    }
}

fn keys(ui: &mut egui::Ui, s: &mut Settings) {
    card(ui, |ui| {
        section(ui, "RACCOURCIS DE LA BULLE");
        ui.label(
            egui::RichText::new("Ces touches ne sont actives que lorsque la bulle est ouverte.")
                .size(11.0)
                .color(theme::MUTED),
        );

        let mut changed = false;
        changed |= key_row(ui, "Valider", &mut s.validate);
        changed |= key_row(ui, "Fermer", &mut s.close);
        changed |= key_row(ui, "Suivant", &mut s.next);
        changed |= key_row(ui, "Précédent", &mut s.prev);
        if changed {
            s.apply();
        }

        let vals = [
            s.validate.as_str(),
            s.close.as_str(),
            s.next.as_str(),
            s.prev.as_str(),
        ];
        let duplicate = (0..vals.len()).any(|i| (i + 1..vals.len()).any(|j| vals[i] == vals[j]));
        if duplicate {
            ui.label(
                egui::RichText::new(
                    "Deux actions partagent la même touche — la première l'emporte.",
                )
                .size(11.0)
                .color(theme::WARN),
            );
        }
    });
}

fn key_row(ui: &mut egui::Ui, label: &str, value: &mut String) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(crate::theme::TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(6.0);
            egui::ComboBox::from_id_salt(label)
                .width(160.0)
                .height(32.0)
                .selected_text(key_label(value))
                .show_ui(ui, |ui| {
                    for name in KEYS.map(|(name, _)| name) {
                        let shown = key_label(name);
                        if ui.selectable_label(value.as_str() == name, shown).clicked()
                            && value.as_str() != name
                        {
                            *value = name.to_string();
                            changed = true;
                        }
                    }
                });
        });
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip() {
        let s = Settings {
            tics: false,
            pause: "P".into(),
            validate: "Tab".into(),
            close: "Échap".into(),
            next: "↑".into(),
            prev: "↓".into(),
            alt_chars: vec![("M".into(), "≥".into())],
        };
        let json = serde_json::to_string(&s).expect("sérialisation");
        let back: Settings = serde_json::from_str(&json).expect("désérialisation");
        assert!(s == back);
    }

    #[test]
    fn vk_for_known_and_unknown() {
        assert_eq!(vk_for("Entrée", 0), 0x0D);
        assert_eq!(vk_for("Tab", 0), 0x09);
        assert_eq!(vk_for("Échap", 0), 0x1B);
        assert_eq!(vk_for("→", 0), 0x27);
        assert_eq!(vk_for("inconnue", 0x42), 0x42);
    }

    #[test]
    fn default_values() {
        let d = Settings::default();
        assert!(d.tics);
        assert_eq!(d.validate, "Entrée");
        assert_eq!(d.close, "Échap");
        assert_eq!(d.next, "↓");
        assert_eq!(d.prev, "↑");
        assert_eq!(d.pause, "P");
        assert!(d.alt_chars.is_empty());
    }

    #[test]
    fn captured_char_filters() {
        assert_eq!(captured_char(egui::Key::C), Some('C'));
        assert_eq!(captured_char(egui::Key::Num3), Some('3'));
        assert_eq!(captured_char(egui::Key::ArrowUp), None);
        assert_eq!(captured_char(egui::Key::Escape), None);
    }

    #[test]
    fn publish_remap_and_alt_chars() {
        let s = Settings {
            tics: false,
            pause: "P".into(),
            validate: "Tab".into(),
            close: "Échap".into(),
            next: "↓".into(),
            prev: "↑".into(),
            alt_chars: vec![
                ("M".into(), "≥".into()),
                ("3".into(), "£".into()),
                ("c".into(), "ç".into()), // minuscule acceptée → VK majuscule
                ("MM".into(), "x".into()), // clé invalide ignorée
                ("A".into(), String::new()), // valeur vide ignorée
            ],
        };
        publish(&s);

        assert!(!tics());
        assert!(matches!(remap(0x09), Some(KeyMsg::Return)));
        assert!(matches!(remap(0x1B), Some(KeyMsg::Escape)));
        assert!(matches!(remap(0x28), Some(KeyMsg::Down)));
        assert!(matches!(remap(0x26), Some(KeyMsg::Up)));
        assert!(remap(0x41).is_none());

        assert_eq!(alt_char(0x4D), Some('≥'));
        assert_eq!(alt_char(0x33), Some('£'));
        assert_eq!(alt_char(0x43), Some('ç'));
        assert!(alt_char(0x41).is_none());

        assert_eq!(pause_vk(), 0x50);

        // Ne pas laisser les statiques modifiées aux autres tests.
        publish(&Settings::default());
    }

    /// Fumée : les trois onglets se rendent sans panique (cases pleine largeur,
    /// liste des raccourcis actifs, ComboBox des touches).
    #[test]
    fn tabs_render() {
        let ctx = egui::Context::default();
        let mut s = Settings::default();
        let mut st = State::default();
        for tab in [Tab::General, Tab::Chars, Tab::Keys] {
            st.tab = tab;
            let _ = ctx.run_ui(Default::default(), |ui| window(ui, &mut s, &mut st));
        }
    }
}
