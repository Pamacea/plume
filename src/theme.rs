//! Charte « Dark Slate » : unique source des couleurs et de la géométrie egui.

use eframe::egui;

pub const BG: egui::Color32 = egui::Color32::from_rgb(0x12, 0x12, 0x14);
pub const CARD: egui::Color32 = egui::Color32::from_rgb(0x1E, 0x1E, 0x22);
pub const INPUT: egui::Color32 = egui::Color32::from_rgb(0x2A, 0x2A, 0x30);
pub const HOVER: egui::Color32 = egui::Color32::from_rgb(0x38, 0x38, 0x42);
pub const ACTIVE: egui::Color32 = egui::Color32::from_rgb(0x4A, 0x4A, 0x58);
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(0xE0, 0xE0, 0xE5);
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(0x8E, 0x8E, 0x9A);
pub const BORDER: egui::Color32 = egui::Color32::from_rgb(0x2D, 0x2D, 0x35);
pub const SEL: egui::Color32 = egui::Color32::from_rgb(0x2E, 0x2E, 0x38);
pub const OK: egui::Color32 = egui::Color32::from_rgb(0x4A, 0xDE, 0x80);
pub const WARN: egui::Color32 = egui::Color32::from_rgb(0xF5, 0x9E, 0x0B);

/// Ligne cliquable pleine largeur avec case à cocher : les boutons voisins font
/// `available_width()`, une case seule ne fait que la largeur de son libellé.
/// `indent` aligne la case sur la colonne d'icônes du menu tray.
pub fn check_row(ui: &mut egui::Ui, value: &mut bool, label: &str, indent: f32) -> bool {
    let before = *value;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ui.spacing().interact_size.y),
        egui::Sense::click(),
    );
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(indent, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row.spacing_mut().item_spacing.x = 8.0;
    let check = row.add(egui::Checkbox::without_text(value)).rect;
    row.label(egui::RichText::new(label).color(TEXT));
    // clic hors de la case : elle gère déjà le sien (jamais deux basculements)
    if resp.clicked() && !check.contains(resp.interact_pointer_pos().unwrap_or_default()) {
        *value = !*value;
    }
    *value != before
}

pub fn apply(ctx: &egui::Context) {
    // thème figé : la charte ne doit pas suivre le thème clair de Windows
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|style| {
        let r = egui::CornerRadius::same(6);
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = BG;
        v.window_fill = CARD;
        v.extreme_bg_color = INPUT;
        v.faint_bg_color = CARD;
        v.weak_text_color = Some(MUTED);
        v.hyperlink_color = TEXT;
        v.selection.bg_fill = SEL;
        v.selection.stroke = egui::Stroke::new(1.0, TEXT);
        v.window_corner_radius = egui::CornerRadius::same(8);
        v.menu_corner_radius = r;
        v.window_stroke = egui::Stroke::new(1.0, BORDER);
        // aucun liseré bleu natif : fonds pleins, bord discret, texte clair
        for (w, fill, stroke) in [
            (
                &mut v.widgets.noninteractive,
                CARD,
                egui::Stroke::new(1.0, BORDER),
            ),
            (&mut v.widgets.inactive, INPUT, egui::Stroke::NONE),
            (
                &mut v.widgets.hovered,
                HOVER,
                egui::Stroke::new(1.0, ACTIVE),
            ),
            (
                &mut v.widgets.active,
                ACTIVE,
                egui::Stroke::new(1.0, ACTIVE),
            ),
            (&mut v.widgets.open, ACTIVE, egui::Stroke::NONE),
        ] {
            w.weak_bg_fill = fill;
            w.bg_fill = fill;
            w.bg_stroke = stroke;
            w.fg_stroke = egui::Stroke::new(1.0, TEXT);
            w.corner_radius = r;
            w.expansion = 0.0;
        }
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, MUTED);
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 8.0);
        style.spacing.interact_size.y = 32.0;
        // barres de défilement non flottantes : elles réservent leur largeur au
        // lieu de recouvrir les libellés et boutons à droite
        style.spacing.scroll.floating = false;
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.bar_inner_margin = 6.0;
    });
}
