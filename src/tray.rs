//! Icône de notification (générée en RGBA) + contenu de la popup egui.
//!
//! Créée sur le thread principal AVANT `eframe::run_native` : tray-icon repose sur
//! la boucle de messages Windows, que seule winit (thread principal) vide.
//! L'état tient dans un `thread_local!` (tray-icon est !Send) pour ne pas étoffer `App`.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use tray_icon::{Icon, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::hook;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "plume";

struct State {
    // doit rester vivante : la destruction de l'icône retire le tray
    tray: TrayIcon,
}

thread_local! {
    // tray-icon est !Send (!Sync) : l'état vit sur le seul thread principal,
    // qui crée le tray et vide ses événements — pas de static possible
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    // clic physique en attente (x centre, y haut de l'icône) : ancre de la popup
    static PENDING: Cell<Option<(f32, f32)>> = const { Cell::new(None) };
}
/// dernier état affiché par l'icône (synchro hotkey ⇄ tray)
static SHOWN_PAUSED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    let paused = hook::is_paused();
    let tray = TrayIconBuilder::new()
        .with_icon(icon(paused))
        .with_tooltip(tooltip(paused))
        // aucun menu natif : la popup egui est rendue par l'appelant
        .with_menu_on_left_click(false)
        .with_menu_on_right_click(false)
        .build()
        .expect("création de l'icône de tray");
    STATE.with(|s| {
        *s.borrow_mut() = Some(State { tray });
    });
    SHOWN_PAUSED.store(paused, Ordering::Relaxed);
    crate::dlog::dlog("icône de tray installée");
}

/// À appeler à chaque frame depuis `App::ui` : winit ne délivre pas les événements
/// de tray tout seul, ils arrivent par un canal qu'il faut vider.
pub fn poll() {
    STATE.with(|cell| {
        let slot = cell.borrow();
        let Some(s) = slot.as_ref() else {
            return;
        };
        // l'icône suit aussi les bascules faites par la hotkey Ctrl+Alt+P
        let paused = hook::is_paused();
        if paused != SHOWN_PAUSED.swap(paused, Ordering::Relaxed) {
            let _ = s.tray.set_icon(Some(icon(paused)));
            let _ = s.tray.set_tooltip(Some(tooltip(paused)));
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            // clic gauche comme droit, au relâchement : mémorise l'ancre de la popup
            if let TrayIconEvent::Click {
                rect,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                PENDING.with(|p| {
                    p.set(Some((
                        (rect.position.x + rect.size.width as f64 / 2.0) as f32,
                        rect.position.y as f32,
                    )))
                });
            }
        }
    });
}

/// Ancre du dernier clic sur l'icône, consommée une seule fois par l'appelant.
pub fn take_menu_request() -> Option<(f32, f32)> {
    PENDING.with(|p| p.take())
}

fn tooltip(paused: bool) -> &'static str {
    if paused {
        "plume — en pause"
    } else {
        "plume"
    }
}

/// Icône 32×32 : carré arrondi, plume blanche (normal) ou pictogramme pause.
fn icon(paused: bool) -> Icon {
    const SIZE: u32 = 32;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let alpha = rounded_rect_coverage(px, py);
            // fond : dégradé vertical cyan → bleu, ou ardoise en pause
            let t = y as f32 / (SIZE - 1) as f32;
            let mut c = if paused {
                [51.0, 65.0, 85.0]
            } else {
                [34.0 + 3.0 * t, 211.0 - 112.0 * t, 238.0 - 3.0 * t]
            };
            // motif blanc composé sur le fond
            let motif = if paused {
                pause_coverage(px, py)
            } else {
                feather_coverage(px, py)
            };
            for ch in &mut c {
                *ch = *ch * (1.0 - motif) + 255.0 * motif;
            }
            rgba.extend_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, (alpha * 255.0) as u8]);
        }
    }
    // dimensions fixes : l'échec serait un bug de code, pas une entrée utilisateur
    Icon::from_rgba(rgba, SIZE, SIZE).expect("icône 32×32")
}

/// Couverture anti-aliasée d'un carré arrondi centré (demi-taille 15, rayon 7).
fn rounded_rect_coverage(x: f32, y: f32) -> f32 {
    let (qx, qy) = ((x - 16.0).abs() - 8.0, (y - 16.0).abs() - 8.0); // 15 - 7
    let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - 7.0;
    (0.5 - d).clamp(0.0, 1.0) // 1 px de couverture sur le bord
}

/// Couverture de la plume : amande inclinée à -45°, rachis creusé.
fn feather_coverage(x: f32, y: f32) -> f32 {
    let (dx, dy) = (x - 16.0, y - 16.0);
    // repère tourné de -45° : u = axe de la plume, v = largeur (≈ 10 px par unité)
    let (u, v) = ((dx - dy) * 0.0707, (dx + dy) * 0.0707);
    if !(-1.0..=1.0).contains(&u) {
        return 0.0;
    }
    let half = 0.30 * (std::f32::consts::PI * (u + 1.0) / 2.0).sin();
    // rachis : fente centrale qui laisse voir le fond (lit « plume », pas « feuille »)
    if v.abs() > half || v.abs() < 0.05 {
        0.0
    } else {
        1.0
    }
}

/// Couverture des deux barres de pause arrondies (blanches).
fn pause_coverage(x: f32, y: f32) -> f32 {
    let bar = |cx: f32| {
        let (qx, qy) = ((x - cx).abs() - 1.0, (y - 16.0).abs() - 5.5);
        let d = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - 1.5;
        (0.5 - d).clamp(0.0, 1.0)
    };
    bar(11.5).max(bar(20.5))
}

/// Action choisie dans la popup : l'appelant ferme/ouvre/quitte lui-même.
pub enum MenuAction {
    Close,
    Settings,
    Quit,
}

/// Contenu de la popup (l'appelant fournit le viewport egui et le cadre sombre).
pub fn menu(ui: &mut egui::Ui) -> Option<MenuAction> {
    ui.spacing_mut().item_spacing.y = 4.0;
    let mut action = None;
    let paused = hook::is_paused();

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Plume").size(16.0).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(if paused { "En pause" } else { "Actif" })
                    .size(11.0)
                    .color(crate::theme::MUTED),
            );
            let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(
                rect.center(),
                4.0,
                if paused {
                    crate::theme::WARN
                } else {
                    crate::theme::OK
                },
            );
        });
    });
    ui.separator();

    // glyphes vérifiés dans les polices egui : ▶ (U+25B6) et ✕ (U+2715) y sont absents
    let (glyph, label) = if paused {
        ("⏵", "Reprendre")
    } else {
        ("⏸", "Mettre en pause")
    };
    if item(ui, glyph, label, crate::theme::TEXT, crate::theme::HOVER).clicked() {
        hook::toggle_paused();
        action = Some(MenuAction::Close);
    }

    let mut on = autostart_enabled();
    // même largeur que les lignes cliquables ci-dessus (une case seule est plus courte)
    if crate::theme::check_row(ui, &mut on, "Démarrage automatique", 10.0) {
        set_autostart(on);
    }

    ui.separator();
    if item(
        ui,
        "↻",
        "Vérifier les mises à jour",
        crate::theme::TEXT,
        crate::theme::HOVER,
    )
    .clicked()
    {
        crate::update::launch(); // quitte le process en cas de succès
    }
    if item(
        ui,
        "⚙",
        "Paramètres…",
        crate::theme::TEXT,
        crate::theme::HOVER,
    )
    .clicked()
    {
        action = Some(MenuAction::Settings);
    }

    ui.separator();
    let quit = egui::Color32::from_rgb(0xFF, 0x55, 0x55);
    if item(
        ui,
        "×",
        "Quitter",
        quit,
        egui::Color32::from_rgb(0x2A, 0x1A, 0x1C),
    )
    .clicked()
    {
        action = Some(MenuAction::Quit);
    }
    action
}

/// Ligne cliquable pleine largeur : glyphe à gauche, libellé à droite.
fn item(
    ui: &mut egui::Ui,
    glyph: &str,
    label: &str,
    color: egui::Color32,
    hover: egui::Color32,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(6), hover);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // glyphe discret par défaut, teinté quand une couleur d'alerte est demandée
    let glyph_color = if color == crate::theme::TEXT {
        crate::theme::MUTED
    } else {
        color
    };
    let painter = ui.painter();
    painter.text(
        rect.left_center() + egui::vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        glyph,
        egui::FontId::proportional(14.0),
        glyph_color,
    );
    painter.text(
        rect.left_center() + egui::vec2(38.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        color,
    );
    resp
}

pub fn autostart_enabled() -> bool {
    winreg::HKCU
        .open_subkey(RUN_KEY)
        .and_then(|k| k.get_value::<String, _>(VALUE_NAME))
        .is_ok()
}

/// Écrit/supprime la valeur `plume` dans `HKCU\...\Run` ; true = l'état voulu est en place.
pub fn set_autostart(on: bool) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn couverture_carre_arrondi_et_plume() {
        assert_eq!(rounded_rect_coverage(16.0, 16.0), 1.0);
        assert_eq!(rounded_rect_coverage(0.5, 0.5), 0.0); // coin hors du carré
        assert_eq!(feather_coverage(16.0, 16.0), 0.0); // centre = rachis creusé
    }
}
