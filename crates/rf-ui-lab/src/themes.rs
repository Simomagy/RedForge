//! The three styles to compare: Forge (RedForge's own, warm and dark as its
//! loading screen), RedForge 1.x (the C#'s WinUI/Fluent dark, with its
//! coloured icons) and a neutral 3D editor's (Blender, Unreal).

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2,
};

/// What an icon stands for, for the styles that colour them by it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Map,
    Archetype,
    Model,
    Texture,
    Collision,
    Archive,
    Folder,
    Save,
    Select,
    Transform,
    Place,
    History,
    Light,
    Neutral,
}

/// How the dock's tabs look.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tabs {
    /// The active one joined to its panel, flat.
    Joined,
    /// The active one a raised card with rounded top corners (WinUI's TabView).
    Card,
    /// Flat blocks, the active one a shade lighter.
    Block,
}

pub struct Theme {
    pub name: &'static str,
    pub tagline: &'static str,
    /// Regular, strong and heading fonts (system font files; egui's own if missing).
    pub fonts: [&'static str; 3],
    pub size: f32,
    /// Behind everything: the menu, the tab bars.
    pub bg: Color32,
    /// The panels' bodies.
    pub panel: Color32,
    /// Raised things in a panel: groups, the toolbar.
    pub card: Color32,
    pub field: Color32,
    pub widget: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub radius: u8,
    pub big_radius: u8,
    pub spacing: Vec2,
    pub padding: Vec2,
    pub tabs: Tabs,
    /// Toolbar buttons with their names, not only their icons.
    pub labels: bool,
    pub coloured_icons: bool,
}

const fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

pub const THEMES: [Theme; 3] = [
    Theme {
        name: "Forge",
        tagline: "RedForge's own: warm and dark, the icon's red",
        fonts: [
            "/usr/share/fonts/TTF/FiraSans-Regular.ttf",
            "/usr/share/fonts/TTF/FiraSans-SemiBold.ttf",
            "/usr/share/fonts/TTF/FiraSansCondensed-Bold.ttf",
        ],
        size: 14.0,
        bg: rgb(0x0F0B0A),
        panel: rgb(0x171210),
        card: rgb(0x201916),
        field: rgb(0x0B0807),
        widget: rgb(0x2B221E),
        hover: rgb(0x3A2C25),
        pressed: rgb(0x4A3328),
        border: rgb(0x33261F),
        text: rgb(0xF4E8DA),
        weak: rgb(0xA88B74),
        accent: rgb(0xFF3B1C),
        on_accent: rgb(0xFFF6EC),
        radius: 6,
        big_radius: 10,
        spacing: Vec2::new(8.0, 6.0),
        padding: Vec2::new(10.0, 5.0),
        tabs: Tabs::Joined,
        labels: true,
        coloured_icons: true,
    },
    Theme {
        name: "RedForge 1.x",
        tagline: "The C#'s WinUI/Fluent dark, with its coloured icons",
        fonts: [
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/noto/NotoSans-Medium.ttf",
            "/usr/share/fonts/noto/NotoSans-Bold.ttf",
        ],
        size: 14.0,
        bg: rgb(0x202020),
        panel: rgb(0x272727),
        card: rgb(0x2D2D2D),
        field: rgb(0x1F1F1F),
        widget: rgb(0x353535),
        hover: rgb(0x3C3C3C),
        pressed: rgb(0x303030),
        border: rgb(0x3D3D3D),
        text: rgb(0xFFFFFF),
        weak: rgb(0xC5C5C5),
        accent: rgb(0x60CDFF),
        on_accent: rgb(0x000000),
        radius: 4,
        big_radius: 8,
        spacing: Vec2::new(8.0, 6.0),
        padding: Vec2::new(11.0, 5.0),
        tabs: Tabs::Card,
        labels: true,
        coloured_icons: true,
    },
    Theme {
        name: "Neutral",
        tagline: "A 3D editor's: neutral greys, dense, out of the map's way",
        fonts: [
            "/home/msstu/.fonts/Roboto-Regular.ttf",
            "/home/msstu/.fonts/Roboto-Medium.ttf",
            "/home/msstu/.fonts/Roboto-Medium.ttf",
        ],
        size: 13.0,
        bg: rgb(0x161616),
        panel: rgb(0x232323),
        card: rgb(0x2B2B2B),
        field: rgb(0x181818),
        widget: rgb(0x3A3A3A),
        hover: rgb(0x464646),
        pressed: rgb(0x2F4F78),
        border: rgb(0x121212),
        text: rgb(0xE4E4E4),
        weak: rgb(0x8C8C8C),
        accent: rgb(0x4A90D9),
        on_accent: rgb(0xFFFFFF),
        radius: 3,
        big_radius: 4,
        spacing: Vec2::new(6.0, 4.0),
        padding: Vec2::new(6.0, 3.0),
        tabs: Tabs::Block,
        labels: false,
        coloured_icons: false,
    },
];

impl Theme {
    /// An icon's colour: by what it stands for in the coloured styles (the
    /// C#'s icon brushes, warmer in Forge's), else the text's.
    pub fn icon(&self, kind: Kind) -> Color32 {
        if !self.coloured_icons || kind == Kind::Neutral {
            return self.weak;
        }
        let forge = self.name == "Forge";
        rgb(match kind {
            Kind::Map => {
                if forge {
                    0x9CC46B
                } else {
                    0x81C784
                }
            }
            Kind::Archetype => {
                if forge {
                    0xF2B84B
                } else {
                    0xFFD54F
                }
            }
            Kind::Model => {
                if forge {
                    0x6FB7D8
                } else {
                    0x4FC3F7
                }
            }
            Kind::Texture => {
                if forge {
                    0xC98BC9
                } else {
                    0xBA68C8
                }
            }
            Kind::Collision => {
                if forge {
                    0xE8664A
                } else {
                    0xE57373
                }
            }
            Kind::Archive => {
                if forge {
                    0xFF8A4C
                } else {
                    0xFF8A65
                }
            }
            Kind::Folder => {
                if forge {
                    0xE9B44C
                } else {
                    0xFFCA28
                }
            }
            Kind::Save => {
                if forge {
                    0x8DB7D6
                } else {
                    0x64B5F6
                }
            }
            Kind::Select => {
                if forge {
                    0xFF8A3D
                } else {
                    0x4DD0E1
                }
            }
            Kind::Transform => {
                if forge {
                    0xFFB060
                } else {
                    0xFFB74D
                }
            }
            Kind::Place => {
                if forge {
                    0x9CC46B
                } else {
                    0x81C784
                }
            }
            Kind::History => {
                if forge {
                    0xB49AC9
                } else {
                    0xB39DDB
                }
            }
            Kind::Light => {
                if forge {
                    0xFFD166
                } else {
                    0xFFE082
                }
            }
            Kind::Neutral => unreachable!(),
        })
    }

    pub fn heading(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("heading".into()))
    }

    pub fn strong(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("strong".into()))
    }

    /// Its fonts (they take effect from the next frame; every style has the
    /// same families, so one can follow another).
    pub fn fonts(&self) -> FontDefinitions {
        let mut defs = FontDefinitions::default();
        let names = ["text", "strong", "heading"];
        for (name, path) in names.iter().zip(self.fonts) {
            if let Ok(bytes) = std::fs::read(path) {
                defs.font_data.insert((*name).into(), FontData::from_owned(bytes).into());
            }
        }
        if defs.font_data.contains_key("text") {
            defs.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "text".into());
        }
        egui_phosphor::add_to_fonts(&mut defs, egui_phosphor::Variant::Regular);
        let fallback = defs.families[&FontFamily::Proportional].clone();
        for name in ["strong", "heading"] {
            let mut list = fallback.clone();
            if defs.font_data.contains_key(name) {
                list.insert(0, name.into());
            }
            defs.families.insert(FontFamily::Name(name.into()), list);
        }
        defs
    }

    /// Makes it the app's style.
    pub fn apply(&self, ctx: &egui::Context) {
        ctx.set_fonts(self.fonts());
        ctx.set_theme(egui::Theme::Dark);
        let r = CornerRadius::same(self.radius);
        ctx.all_styles_mut(|s| {
            s.text_styles = [
                (TextStyle::Small, FontId::proportional(self.size - 2.0)),
                (TextStyle::Body, FontId::proportional(self.size)),
                (TextStyle::Button, FontId::proportional(self.size)),
                (TextStyle::Heading, self.heading(self.size + 8.0)),
                (TextStyle::Monospace, FontId::monospace(self.size - 1.0)),
            ]
            .into();
            s.spacing.item_spacing = self.spacing;
            s.spacing.button_padding = self.padding;
            s.spacing.interact_size.y = self.size + self.padding.y * 2.0 + 4.0;
            s.spacing.menu_margin = Margin::same(6);
            s.spacing.window_margin = Margin::same(10);

            let v = &mut s.visuals;
            v.dark_mode = true;
            v.override_text_color = None;
            v.weak_text_color = Some(self.weak);
            v.panel_fill = self.panel;
            v.window_fill = self.card;
            v.window_stroke = Stroke::new(1.0, self.border);
            v.window_corner_radius = CornerRadius::same(self.big_radius);
            v.menu_corner_radius = CornerRadius::same(self.big_radius);
            v.window_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(140) };
            v.popup_shadow = Shadow { offset: [0, 4], blur: 12, spread: 0, color: Color32::from_black_alpha(120) };
            v.extreme_bg_color = self.field;
            v.text_edit_bg_color = Some(self.field);
            v.faint_bg_color = self.card;
            v.code_bg_color = self.field;
            v.hyperlink_color = self.accent;
            v.selection.bg_fill = self.accent.linear_multiply(0.45);
            v.selection.stroke = Stroke::new(1.0, self.on_accent);
            v.slider_trailing_fill = true;
            v.striped = false;

            let w = &mut v.widgets;
            w.noninteractive.bg_fill = self.panel;
            w.noninteractive.weak_bg_fill = self.panel;
            w.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
            w.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
            w.noninteractive.corner_radius = r;
            for (wv, fill) in [
                (&mut w.inactive, self.widget),
                (&mut w.hovered, self.hover),
                (&mut w.active, self.pressed),
                (&mut w.open, self.hover),
            ] {
                wv.bg_fill = fill;
                wv.weak_bg_fill = fill;
                wv.corner_radius = r;
                wv.expansion = 0.0;
                wv.fg_stroke = Stroke::new(1.0, self.text);
                wv.bg_stroke = Stroke::NONE;
            }
            w.hovered.bg_stroke = Stroke::new(1.0, self.border.gamma_multiply(1.6));
            w.active.bg_stroke = Stroke::new(1.0, self.accent);
        });
    }

    /// egui_dock's style for it.
    pub fn dock(&self, ui: &egui::Ui) -> egui_dock::Style {
        let mut s = egui_dock::Style::from_egui(ui.style().as_ref());
        s.main_surface_border_stroke = Stroke::NONE;
        s.separator.width = 2.0;
        s.separator.color_idle = self.bg;
        s.separator.color_hovered = self.accent.gamma_multiply(0.6);
        s.separator.color_dragged = self.accent;
        s.tab_bar.bg_fill = self.bg;
        s.tab_bar.height = self.size + 16.0;
        s.tab_bar.hline_color = self.bg;
        s.tab.tab_body.bg_fill = self.panel;
        s.tab.tab_body.stroke = Stroke::NONE;
        s.tab.tab_body.inner_margin = Margin::same(8);
        s.tab.hline_below_active_tab_name = false;
        s.tab.spacing = if self.tabs == Tabs::Card { 2.0 } else { 0.0 };
        let top = CornerRadius { nw: self.radius + 2, ne: self.radius + 2, sw: 0, se: 0 };
        let small = CornerRadius { nw: self.radius, ne: self.radius, sw: 0, se: 0 };
        let (active_fill, radius) = match self.tabs {
            Tabs::Joined => (self.panel, small),
            Tabs::Card => (self.panel, top),
            Tabs::Block => (self.panel, CornerRadius::ZERO),
        };
        for t in
            [&mut s.tab.active, &mut s.tab.focused, &mut s.tab.active_with_kb_focus, &mut s.tab.focused_with_kb_focus]
        {
            t.bg_fill = active_fill;
            t.text_color = self.text;
            t.outline_color = Color32::TRANSPARENT;
            t.corner_radius = radius;
        }
        for t in [&mut s.tab.inactive, &mut s.tab.inactive_with_kb_focus] {
            t.bg_fill = self.bg;
            t.text_color = self.weak;
            t.outline_color = Color32::TRANSPARENT;
            t.corner_radius = radius;
        }
        s.tab.hovered.bg_fill = self.card;
        s.tab.hovered.text_color = self.text;
        s.tab.hovered.outline_color = Color32::TRANSPARENT;
        s.tab.hovered.corner_radius = radius;
        s.buttons.close_tab_color = self.weak;
        s.buttons.close_tab_active_color = self.text;
        s.buttons.close_tab_bg_fill = self.hover;
        s.overlay.selection_color = self.accent.gamma_multiply(0.4);
        s
    }
}
