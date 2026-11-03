//! RedForge's look, "Islands" (tried out in `rf-ui-lab`): neutral greys
//! with an orange accent, Fira Sans, Phosphor icons, and the panels as
//! rounded islands over a darker background with gaps between them (as
//! JetBrains' new UI). Also the widgets that look: list rows with the
//! accent's bar, the toolbar's icon buttons and their groups.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Response, Sense, Shadow,
    Stroke, TextStyle, Vec2,
};

pub use egui_phosphor::regular as icon;

const fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Behind the islands, the menu and the status bar.
pub const BG: Color32 = rgb(0x131313);
/// The islands.
pub const PANEL: Color32 = rgb(0x212121);
/// Raised things on an island: groups, the toolbar's.
pub const CARD: Color32 = rgb(0x2A2A2A);
pub const FIELD: Color32 = rgb(0x191919);
pub const WIDGET: Color32 = rgb(0x323232);
pub const HOVER: Color32 = rgb(0x3C3C3C);
pub const PRESSED: Color32 = rgb(0x474747);
pub const BORDER: Color32 = rgb(0x2D2D2D);
pub const TEXT: Color32 = rgb(0xE6E6E6);
pub const WEAK: Color32 = rgb(0x8E8E8E);
pub const ACCENT: Color32 = rgb(0xFF8A3D);
/// Text and icons on the accent.
pub const ON_ACCENT: Color32 = rgb(0x1A1A1A);

pub const SIZE: f32 = 14.0;
pub const RADIUS: u8 = 8;
pub const BIG_RADIUS: u8 = 12;
/// Between the islands, and around them.
pub const GAP: i8 = 8;

/// Fira Sans (SIL Open Font License, `assets/fonts/OFL.txt`) for text, its
/// semibold for what stands out, its condensed bold for headings; Phosphor's
/// icons in every family; egui's own fonts after (emoji and the rest).
fn fonts() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    for (name, bytes) in [
        ("fira", &include_bytes!("../assets/fonts/FiraSans-Regular.ttf")[..]),
        ("fira-semibold", &include_bytes!("../assets/fonts/FiraSans-SemiBold.ttf")[..]),
        ("fira-condensed-bold", &include_bytes!("../assets/fonts/FiraSansCondensed-Bold.ttf")[..]),
    ] {
        defs.font_data.insert(name.into(), FontData::from_static(bytes).into());
    }
    defs.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "fira".into());
    egui_phosphor::add_to_fonts(&mut defs, egui_phosphor::Variant::Regular);
    let rest = defs.families[&FontFamily::Proportional].clone();
    for (family, font) in [("strong", "fira-semibold"), ("heading", "fira-condensed-bold")] {
        let mut list = rest.clone();
        list.insert(0, font.into());
        defs.families.insert(FontFamily::Name(family.into()), list);
    }
    defs
}

/// The font of what stands out (section names, field names).
pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("strong".into()))
}

pub fn heading(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("heading".into()))
}

/// Makes it the app's look (the fonts from the next frame).
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_theme(egui::Theme::Dark);
    let r = CornerRadius::same(RADIUS);
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(SIZE - 2.0)),
            (TextStyle::Body, FontId::proportional(SIZE)),
            (TextStyle::Button, FontId::proportional(SIZE)),
            (TextStyle::Heading, heading(SIZE + 8.0)),
            (TextStyle::Monospace, FontId::monospace(SIZE - 1.0)),
        ]
        .into();
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(8.0, 4.0);
        s.spacing.interact_size.y = SIZE + 12.0;
        s.spacing.menu_margin = Margin::same(6);
        s.spacing.window_margin = Margin::same(12);

        let v = &mut s.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.weak_text_color = Some(WEAK);
        v.panel_fill = PANEL;
        v.window_fill = CARD;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(BIG_RADIUS);
        v.menu_corner_radius = CornerRadius::same(BIG_RADIUS);
        v.window_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(140) };
        v.popup_shadow = Shadow { offset: [0, 4], blur: 12, spread: 0, color: Color32::from_black_alpha(120) };
        v.extreme_bg_color = FIELD;
        v.text_edit_bg_color = Some(FIELD);
        v.faint_bg_color = CARD;
        v.code_bg_color = FIELD;
        v.hyperlink_color = ACCENT;
        // Selected list items: a lighter grey (the accent goes in their bar).
        v.selection.bg_fill = PRESSED;
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.slider_trailing_fill = true;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = PANEL;
        w.noninteractive.weak_bg_fill = PANEL;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.corner_radius = r;
        for (wv, fill) in
            [(&mut w.inactive, WIDGET), (&mut w.hovered, HOVER), (&mut w.active, PRESSED), (&mut w.open, HOVER)]
        {
            wv.bg_fill = fill;
            wv.weak_bg_fill = fill;
            wv.corner_radius = r;
            wv.expansion = 0.0;
            wv.fg_stroke = Stroke::new(1.0, TEXT);
            wv.bg_stroke = Stroke::NONE;
        }
        w.hovered.bg_stroke = Stroke::new(1.0, BORDER.gamma_multiply(1.6));
        w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    });
}

/// egui_dock's style: each leaf an island (its tab bar the top, its body
/// the rest), the separators gaps of the background, the tabs pills.
pub fn dock(style: &egui::Style) -> egui_dock::Style {
    let mut s = egui_dock::Style::from_egui(style);
    let big = BIG_RADIUS;
    s.main_surface_border_stroke = Stroke::NONE;
    s.separator.width = GAP as f32;
    s.separator.extra_interact_width = 2.0;
    s.separator.color_idle = BG;
    s.separator.color_hovered = BG;
    s.separator.color_dragged = ACCENT.gamma_multiply(0.5);
    s.tab_bar.bg_fill = PANEL;
    s.tab_bar.corner_radius = CornerRadius { nw: big, ne: big, sw: 0, se: 0 };
    s.tab_bar.height = SIZE + 22.0;
    s.tab_bar.inner_margin = Margin { left: 6, right: 6, top: 5, bottom: 3 };
    s.tab_bar.hline_color = PANEL;
    s.tab_bar.show_scroll_bar_on_overflow = false;
    s.tab.tab_body.bg_fill = PANEL;
    s.tab.tab_body.stroke = Stroke::NONE;
    s.tab.tab_body.corner_radius = CornerRadius { nw: 0, ne: 0, sw: big, se: big };
    s.tab.tab_body.inner_margin = Margin::same(10);
    s.tab.hline_below_active_tab_name = false;
    s.tab.spacing = 4.0;
    let pill = CornerRadius::same(RADIUS);
    for t in [&mut s.tab.active, &mut s.tab.focused, &mut s.tab.active_with_kb_focus, &mut s.tab.focused_with_kb_focus]
    {
        t.bg_fill = WIDGET;
        t.text_color = TEXT;
        t.outline_color = Color32::TRANSPARENT;
        t.corner_radius = pill;
    }
    for t in [&mut s.tab.inactive, &mut s.tab.inactive_with_kb_focus] {
        t.bg_fill = PANEL;
        t.text_color = WEAK;
        t.outline_color = Color32::TRANSPARENT;
        t.corner_radius = pill;
    }
    s.tab.hovered.bg_fill = CARD;
    s.tab.hovered.text_color = TEXT;
    s.tab.hovered.outline_color = Color32::TRANSPARENT;
    s.tab.hovered.corner_radius = pill;
    s.buttons.close_tab_color = WEAK;
    s.buttons.close_tab_active_color = TEXT;
    s.buttons.close_tab_bg_fill = HOVER;
    s.overlay.selection_color = ACCENT.gamma_multiply(0.4);
    s
}

/// An icon then a label (the icon dimmer), for tab titles, buttons, menus.
pub fn icon_text(glyph: &str, label: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font = FontId::proportional(SIZE);
    job.append(glyph, 0.0, egui::TextFormat { font_id: font.clone(), color: WEAK, ..Default::default() });
    if !label.is_empty() {
        job.append(label, 6.0, egui::TextFormat { font_id: font, color: TEXT, ..Default::default() });
    }
    job
}

/// A toolbar button: an icon, in the accent when it's on.
pub fn tool(ui: &mut egui::Ui, on: bool, glyph: &str, tip: &str) -> Response {
    let colour = if on { ON_ACCENT } else { TEXT };
    let b = egui::Button::new(egui::RichText::new(glyph).size(SIZE + 1.0).color(colour))
        .fill(if on { ACCENT } else { Color32::TRANSPARENT })
        .corner_radius(CornerRadius::same(RADIUS))
        .min_size(Vec2::new(28.0, 26.0));
    ui.add(b).on_hover_text(tip)
}

/// A group of toolbar controls, on a pill of its own.
pub fn group<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(RADIUS + 2))
        .inner_margin(Margin::symmetric(4, 3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(add).inner
        })
        .inner
}

/// A list row: selectable, its icon and name, a dim note after; selected,
/// a lighter grey with the accent's bar and icon.
pub fn row(ui: &mut egui::Ui, selected: bool, glyph: &str, name: &str, note: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(RADIUS), WIDGET);
        let bar = egui::Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height()));
        painter.rect_filled(bar, CornerRadius::same(2), ACCENT);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(RADIUS), HOVER.gamma_multiply(0.6));
    }
    let font = FontId::proportional(SIZE);
    let (mut x, y) = (rect.min.x + 10.0, rect.center().y);
    if !glyph.is_empty() {
        let colour = if selected { ACCENT } else { WEAK };
        painter.text(egui::pos2(x, y), egui::Align2::LEFT_CENTER, glyph, font.clone(), colour);
        x += SIZE + 8.0;
    }
    let shown = painter.text(egui::pos2(x, y), egui::Align2::LEFT_CENTER, name, font, TEXT);
    if !note.is_empty() {
        let at = egui::pos2(shown.max.x + 10.0, y);
        painter.text(at, egui::Align2::LEFT_CENTER, note, FontId::proportional(SIZE - 2.0), WEAK);
    }
    resp
}

/// How tall a [`row`] (and a [`section`]) is.
pub const ROW: f32 = SIZE + 10.0;

/// A section's header in a list: open or closed, its name and how many.
pub fn section(ui: &mut egui::Ui, open: bool, glyph: &str, name: &str, count: usize) -> Response {
    let size = Vec2::new(ui.available_width(), ROW);
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    let resp = ui
        .allocate_ui_with_layout(size, layout, |ui| {
            ui.label(egui::RichText::new(if open { icon::CARET_DOWN } else { icon::CARET_RIGHT }).color(WEAK));
            if !glyph.is_empty() {
                ui.label(egui::RichText::new(glyph).color(WEAK));
            }
            ui.label(egui::RichText::new(name).font(strong(SIZE)).color(TEXT));
            egui::Frame::new()
                .fill(WIDGET)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(6, 0))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(count.to_string()).small().color(WEAK));
                });
        })
        .response;
    ui.interact(resp.rect, resp.id.with("section"), Sense::click())
}
