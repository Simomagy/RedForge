//! The loading screen, as the C#'s `LoadingScreen`: the app icon's anvil,
//! hammer and sparks at work in the forge's light, embers rising, and how far
//! the loading is as metal heating up, with tips. The shapes and gradients
//! are drawn once into textures; the C#'s storyboards move them.

use std::f32::consts::PI;
use std::time::Instant;

use eframe::egui::{self, Color32, FontId, Mesh, Pos2, Rect, Shape, TextureHandle, Vec2};
use tiny_skia::{
    FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, Point, RadialGradient, SpreadMode, Stroke,
    Transform,
};

/// The loading's stages, by the words of their progress message: what to
/// show, where the bar starts and where it heads while in it.
const STAGES: [(&str, &str, f32, f32); 5] = [
    ("names", "Lighting the forge…", 0.0, 0.06),
    ("keys", "Finding the keys in RDR2.exe…", 0.06, 0.10),
    ("archives", "Reading the RDR2 archives…", 0.10, 0.45),
    ("data files", "Unpacking the game data…", 0.45, 0.85),
    ("Indexing", "Setting up the map…", 0.85, 0.97),
];

const TIPS: [&str; 10] = [
    "Q toggles the move gizmo, E the rotate gizmo, and . switches between object and world axes.",
    "Hold Shift as you start dragging the gizmo to drag copies of the selection.",
    "Home looks at the selection; a double click in the Hierarchy does too.",
    "The menu at the start of the map's toolbar switches the selection mode: collisions, navmeshes, occluders…",
    "Pick an object in Objects and click on the map to place it; Q and E turn it.",
    "Ctrl+S saves the project's files; RedM resource in the Project pane exports them ready to stream.",
    "A ymap picked in the Project pane shows its objects in the Hierarchy.",
    "Drag the edges of the side panels to make the map bigger.",
    "Edit in Blender, in the Edit menu, sends the selection to Blender; what Blender sends back shows on the map.",
    "Right click (or click) selects; with Ctrl it adds to the selection or takes away from it.",
];

/// The icon's shapes in its 256 px space (traced from it, as the C#'s).
const ANVIL: &str = "M95.8,120.5 L96.5,120.0 L208.5,120.0 L209.4,120.5 L209.5,133.5 L209.9,134.5 L217.5,137.1 L224.5,140.4 L230.5,144.2 L238.1,150.5 L233.5,154.9 L225.5,160.8 L218.5,164.9 L210.6,168.5 L209.5,169.5 L209.5,182.6 L208.5,183.1 L180.5,183.1 L179.8,183.5 L181.3,187.5 L185.2,194.5 L190.2,201.5 L196.6,208.5 L204.5,215.8 L208.5,218.4 L220.5,218.4 L221.5,218.8 L221.9,219.5 L221.5,229.0 L84.5,229.2 L83.5,228.7 L83.5,219.2 L84.5,218.4 L95.5,218.5 L96.5,218.2 L100.5,214.6 L105.6,209.5 L114.7,198.5 L119.8,190.5 L122.9,183.5 L122.5,183.1 L96.5,183.1 L95.7,182.5 L95.6,167.5 L81.5,161.9 L74.5,157.6 L69.5,153.7 L63.2,147.5 L59.2,142.5 L56.1,137.5 L52.8,130.5 L53.5,129.9 L94.5,129.9 L95.6,129.5 L95.8,128.5 Z";
const SPARKS: &str = "M199.0,26.5 L199.5,25.6 L201.1,26.5 L191.2,69.5 L191.5,70.1 L192.5,70.0 L226.5,51.7 L227.1,52.5 L204.3,85.5 L238.5,86.6 L238.9,87.5 L175.5,112.9 L174.5,113.0 L174.1,112.5 L174.1,100.5 L173.5,99.6 L144.5,113.2 L143.8,112.5 L144.0,111.5 L159.0,67.5 L159.5,66.8 L160.0,67.5 L163.4,81.5 L164.5,81.5 Z";
const HAMMER: &str = "M101.5,41.5 L102.5,41.0 L103.5,41.6 L129.9,67.5 L140.7,102.5 L116.5,113.7 L115.5,113.7 L101.9,91.5 L100.5,90.4 L17.5,127.9 L15.5,128.2 L9.1,114.5 L9.5,113.2 L93.5,75.4 L94.0,74.5 L93.8,73.5 L86.3,48.5 Z";

/// The hammer's grip, which it turns on; where the sparks grow from; where
/// the spark bits fly from.
const GRIP: Vec2 = Vec2::new(13.0, 121.0);
const SPARKS_CENTRE: Vec2 = Vec2::new(150.0, 112.0);
const BITS_FROM: Vec2 = Vec2::new(118.0, 119.0);
/// One hammer blow, and when it lands in it (seconds).
const BLOW: f32 = 1.6;
const IMPACT: f32 = 0.84;
const BAR: Vec2 = Vec2::new(380.0, 8.0);
/// The textures' pixels per point of the icon's space and the bar's.
const DETAIL: f32 = 3.0;

#[derive(Clone, Copy)]
enum Ease {
    Linear,
    /// Jumps to the key's value at its time.
    Discrete,
    CubicOut,
    QuadIn,
    QuadOut,
    SineOut,
    SineInOut,
}

impl Ease {
    fn at(self, t: f32) -> f32 {
        match self {
            Ease::Linear => t,
            Ease::Discrete => (t >= 1.0) as u8 as f32,
            Ease::CubicOut => 1.0 - (1.0 - t).powi(3),
            Ease::QuadIn => t * t,
            Ease::QuadOut => 1.0 - (1.0 - t) * (1.0 - t),
            Ease::SineOut => (t * PI * 0.5).sin(),
            Ease::SineInOut => (1.0 - (t * PI).cos()) * 0.5,
        }
    }
}

/// A storyboard's key frames at `t`: from `base`, each key eased to its value
/// by its time; the last value after.
fn keys(t: f32, base: f32, keys: &[(f32, f32, Ease)]) -> f32 {
    let (mut at, mut value) = (0.0, base);
    for &(time, v, ease) in keys {
        if t < time {
            let span = time - at;
            let f = if span > 0.0 { ((t - at) / span).clamp(0.0, 1.0) } else { 1.0 };
            return value + (v - value) * ease.at(f);
        }
        (at, value) = (time, v);
    }
    value
}

/// The C#'s `Random`-placed bits, from a seed (not its sequence, the same spread).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

struct Ember {
    at: Vec2,
    radius: f32,
    colour: Color32,
    life: f32,
    rise: f32,
    drift: f32,
    begin: f32,
    peak: f32,
}

struct Bit {
    radius: f32,
    colour: Color32,
    to: Vec2,
    fall: f32,
    life: f32,
}

struct Textures {
    anvil: TextureHandle,
    hammer: TextureHandle,
    sparks: TextureHandle,
    flash: TextureHandle,
    glow: TextureHandle,
    vignette: TextureHandle,
    bar: TextureHandle,
    hot_spot: TextureHandle,
}

pub struct LoadingScreen {
    start: Instant,
    last: Instant,
    stage: usize,
    stage_start: Instant,
    shown: f32,
    /// When the loading finished, and when the screen began to fade.
    finished: Option<Instant>,
    fading: Option<Instant>,
    tip: usize,
    tip_time: Instant,
    textures: Option<Textures>,
    embers: Vec<Ember>,
    bits: Vec<Bit>,
    /// Seconds it stays up at least (`REDFORGE_HOLD_LOADING`, for testing).
    hold: f32,
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn skia(argb: u32) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8((argb >> 16) as u8, (argb >> 8) as u8, argb as u8, (argb >> 24) as u8)
}

/// Gradient stops from `(offset, 0xAARRGGBB)`.
fn stops(list: &[(f32, u32)]) -> Vec<GradientStop> {
    list.iter().map(|&(o, c)| GradientStop::new(o, skia(c))).collect()
}

/// A path of the C#'s `M x,y L x,y … Z` polygons, and its bounds.
fn polygon(data: &str) -> (tiny_skia::Path, Rect) {
    let mut pb = PathBuilder::new();
    let mut bounds = Rect::NOTHING;
    for token in data.split_whitespace() {
        let (cmd, rest) = token.split_at(1);
        if cmd == "Z" {
            pb.close();
            continue;
        }
        let mut xy = rest.split(',').filter_map(|v| v.parse::<f32>().ok());
        let (Some(x), Some(y)) = (xy.next(), xy.next()) else { continue };
        bounds.extend_with(Pos2::new(x, y));
        if cmd == "M" { pb.move_to(x, y) } else { pb.line_to(x, y) }
    }
    (pb.finish().expect("a shape"), bounds)
}

/// The C#'s `RadialGradientBrush` over a box (its centre and radii relative
/// to it).
fn radial(b: Rect, centre: Vec2, radii: Vec2, list: &[(f32, u32)]) -> tiny_skia::Shader<'static> {
    let c = b.min + b.size() * centre;
    let r = b.size() * radii;
    let t = Transform::from_row(r.x, 0.0, 0.0, r.y, c.x, c.y);
    RadialGradient::new(Point::zero(), Point::zero(), 1.0, stops(list), SpreadMode::Pad, t).expect("a gradient")
}

fn texture(ctx: &egui::Context, name: &str, pixmap: &Pixmap) -> TextureHandle {
    let size = [pixmap.width() as usize, pixmap.height() as usize];
    let image = egui::ColorImage::from_rgba_premultiplied(size, pixmap.data());
    ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
}

/// The icon's shape, filled and outlined, over its 256 px space.
fn shape(
    ctx: &egui::Context,
    name: &str,
    data: &str,
    fill: impl Fn(Rect) -> tiny_skia::Shader<'static>,
    outline: (u32, f32),
) -> TextureHandle {
    let size = (256.0 * DETAIL) as u32;
    let mut pixmap = Pixmap::new(size, size).expect("a pixmap");
    let (path, bounds) = polygon(data);
    let t = Transform::from_scale(DETAIL, DETAIL);
    let paint = Paint { shader: fill(bounds), anti_alias: true, ..Default::default() };
    pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
    let mut stroke = Paint { anti_alias: true, ..Default::default() };
    stroke.set_color(skia(0xFF00_0000 | outline.0));
    pixmap.stroke_path(&path, &stroke, &Stroke { width: outline.1, ..Default::default() }, t, None);
    texture(ctx, name, &pixmap)
}

/// A gradient over a whole square, stretched over what it covers.
fn fill_gradient(ctx: &egui::Context, name: &str, size: u32, shader: tiny_skia::Shader<'static>) -> TextureHandle {
    let mut pixmap = Pixmap::new(size, size).expect("a pixmap");
    let rect = tiny_skia::Rect::from_xywh(0.0, 0.0, size as f32, size as f32).unwrap();
    pixmap.fill_rect(rect, &Paint { shader, anti_alias: true, ..Default::default() }, Transform::identity(), None);
    texture(ctx, name, &pixmap)
}

impl Textures {
    fn new(ctx: &egui::Context) -> Self {
        // The icon's red, darker where the light doesn't reach.
        let iron = |b: Rect| {
            let s = [(0.0, 0xFF9A_0000), (0.55, 0xFFD4_0000), (1.0, 0xFFFF_1A0A)];
            radial(b, Vec2::new(0.35, 0.2), Vec2::splat(0.95), &s)
        };
        let anvil = shape(ctx, "anvil", ANVIL, iron, (0x2B_0000, 1.2));
        let hammer = shape(ctx, "hammer", HAMMER, iron, (0x2B_0000, 1.2));
        let sparks = shape(
            ctx,
            "sparks",
            SPARKS,
            |b| {
                let s = [(0.0, 0xFFFF_2A00), (0.5, 0xFFFF_7A10), (1.0, 0xFFFF_D24A)];
                let (from, to) = (Point::from_xy(b.min.x, b.max.y), Point::from_xy(b.max.x, b.min.y));
                LinearGradient::new(from, to, stops(&s), SpreadMode::Pad, Transform::identity()).expect("a gradient")
            },
            (0x7A_1500, 0.8),
        );
        // The flash where the hammer lands.
        let size = (256.0 * DETAIL) as u32;
        let mut pixmap = Pixmap::new(size, size).expect("a pixmap");
        let oval = tiny_skia::Rect::from_xywh(27.0, 70.0, 180.0, 100.0).unwrap();
        let s = [(0.0, 0xFFFF_F4CF), (0.18, 0xD0FF_B347), (0.45, 0x60FF_5A14), (1.0, 0x00FF_3000)];
        let b = Rect::from_min_size(Pos2::new(27.0, 70.0), Vec2::new(180.0, 100.0));
        let paint =
            Paint { shader: radial(b, Vec2::splat(0.5), Vec2::splat(0.5), &s), anti_alias: true, ..Default::default() };
        let t = Transform::from_scale(DETAIL, DETAIL);
        pixmap.fill_path(&PathBuilder::from_oval(oval).unwrap(), &paint, FillRule::Winding, t, None);
        let flash = texture(ctx, "flash", &pixmap);

        let unit = Rect::from_min_size(Pos2::ZERO, Vec2::splat(256.0));
        // The forge's warm light, and the dark around the edges.
        let s = [(0.0, 0x58FF_5A14), (0.4, 0x24B8_300C), (0.8, 0x0000_0000)];
        let glow = fill_gradient(ctx, "glow", 256, radial(unit, Vec2::new(0.5, 0.4), Vec2::new(0.6, 0.7), &s));
        let s = [(0.5, 0x0000_0000), (1.0, 0xC000_0000)];
        let vignette = fill_gradient(ctx, "vignette", 256, radial(unit, Vec2::splat(0.5), Vec2::new(0.75, 0.8), &s));

        // The bar's fill, like metal heating up (squashed with it, as the C#'s).
        let (w, h) = ((BAR.x * DETAIL) as u32, (BAR.y * DETAIL) as u32);
        let mut pixmap = Pixmap::new(w, h).expect("a pixmap");
        let s = [(0.0, 0xFF5E_0B08), (0.45, 0xFFB8_260D), (0.8, 0xFFFF_6A1A), (0.95, 0xFFFF_D166), (1.0, 0xFFFF_F6DC)];
        let shader = LinearGradient::new(
            Point::zero(),
            Point::from_xy(w as f32, 0.0),
            stops(&s),
            SpreadMode::Pad,
            Transform::identity(),
        )
        .expect("a gradient");
        let paint = Paint { shader, anti_alias: true, ..Default::default() };
        let r = h as f32 * 0.5;
        let mut pb = PathBuilder::new();
        pb.push_rect(tiny_skia::Rect::from_xywh(r, 0.0, w as f32 - 2.0 * r, h as f32).unwrap());
        pb.push_circle(r, r, r);
        pb.push_circle(w as f32 - r, r, r);
        pixmap.fill_path(&pb.finish().unwrap(), &paint, FillRule::Winding, Transform::identity(), None);
        let bar = texture(ctx, "bar", &pixmap);
        let s = [(0.0, 0xE0FF_F6DC), (0.35, 0x70FF_B347), (1.0, 0x00FF_6A1A)];
        let square = Rect::from_min_size(Pos2::ZERO, Vec2::splat(64.0));
        let hot_spot = fill_gradient(ctx, "hot spot", 64, radial(square, Vec2::splat(0.5), Vec2::splat(0.5), &s));
        Self { anvil, hammer, sparks, flash, glow, vignette, bar, hot_spot }
    }
}

/// A texture through four corners (top left, top right, bottom right, bottom left).
fn quad(painter: &egui::Painter, texture: &TextureHandle, corners: [Pos2; 4], tint: Color32) {
    let mut mesh = Mesh::with_texture(texture.id());
    let uvs = [Pos2::new(0.0, 0.0), Pos2::new(1.0, 0.0), Pos2::new(1.0, 1.0), Pos2::new(0.0, 1.0)];
    for (p, uv) in corners.into_iter().zip(uvs) {
        mesh.vertices.push(egui::epaint::Vertex { pos: p, uv, color: tint });
    }
    mesh.indices.extend([0, 1, 2, 0, 2, 3]);
    painter.add(Shape::mesh(mesh));
}

fn rect_corners(r: Rect) -> [Pos2; 4] {
    [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()]
}

impl LoadingScreen {
    pub fn new() -> Self {
        let now = Instant::now();
        let mut rng = Rng(7);
        let colours = [0xFFB347, 0xFF8A1F, 0xFF5A1A, 0xFFD27A].map(rgb);
        let embers = (0..46)
            .map(|_| {
                let size = 1.5 + rng.next() * 3.5;
                let colour = colours[(rng.next() * 4.0) as usize % 4];
                let spread = (rng.next() + rng.next() + rng.next() - 1.5) * 380.0;
                let at = Vec2::new(800.0 + spread, 560.0 + rng.next() * 330.0);
                let life = 4.0 + rng.next() * 5.0;
                Ember {
                    at,
                    radius: size * 0.5,
                    colour,
                    life,
                    rise: 300.0 + rng.next() * 420.0,
                    drift: (rng.next() - 0.5) * 180.0,
                    begin: rng.next() * life,
                    peak: 0.35 + rng.next() * 0.55,
                }
            })
            .collect();
        let mut rng = Rng(3);
        let colours = [0xFFF6D6, 0xFFD24A, 0xFF8A1F].map(rgb);
        let bits = (0..16)
            .map(|_| {
                let size = 1.6 + rng.next() * 1.8;
                let colour = colours[(rng.next() * 3.0) as usize % 3];
                let angle = (-165.0 + rng.next() * 150.0_f32).to_radians();
                let speed = 35.0 + rng.next() * 85.0;
                let life = 0.4 + rng.next() * 0.3;
                Bit {
                    radius: size * 0.5,
                    colour,
                    to: Vec2::new(angle.cos(), angle.sin()) * speed,
                    fall: 18.0 + rng.next() * 20.0,
                    life,
                }
            })
            .collect();
        Self {
            start: now,
            last: now,
            stage: 0,
            stage_start: now,
            shown: 0.0,
            finished: None,
            fading: None,
            tip: (now.elapsed().subsec_nanos() as usize) % TIPS.len(),
            tip_time: now,
            textures: None,
            embers,
            bits,
            hold: std::env::var("REDFORGE_HOLD_LOADING").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0),
        }
    }

    /// A progress message of the loading: the stage it starts.
    pub fn set_status(&mut self, status: &str) {
        if let Some(i) = STAGES.iter().position(|s| status.contains(s.0))
            && i != self.stage
        {
            self.stage = i;
            self.stage_start = Instant::now();
        }
    }

    /// The loading is done: the bar fills, then the screen fades.
    pub fn finish(&mut self) {
        self.finished.get_or_insert_with(Instant::now);
    }

    /// Whether it has faded away.
    pub fn gone(&self) -> bool {
        self.fading.is_some_and(|t| t.elapsed().as_secs_f32() > 0.45)
    }

    /// Draws it over the whole window.
    pub fn ui(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        let time = now.duration_since(self.start).as_secs_f32();
        let textures = self.textures.get_or_insert_with(|| Textures::new(ctx));
        let finished = self.finished.filter(|_| time >= self.hold);

        // The bar heads for the stage's end, slower the closer it gets.
        let (_, label, from, to) = STAGES[self.stage];
        let target = match finished {
            Some(_) => 1.0,
            None => from + (to - from) * (1.0 - (-self.stage_start.elapsed().as_secs_f32() / 1.5).exp()),
        };
        self.shown += (target - self.shown) * (1.0 - 0.88_f32.powf(dt * 60.0));
        let label = if finished.is_some() { "Ready" } else { label };
        if finished.is_some() && self.fading.is_none() && self.shown > 0.99 {
            self.fading = Some(now);
        }
        let opacity = self.fading.map_or(1.0, |t| 1.0 - (t.elapsed().as_secs_f32() / 0.45).min(1.0));
        if self.tip_time.elapsed().as_secs_f32() > 7.55 {
            self.tip = (self.tip + 1) % TIPS.len();
            self.tip_time = now;
        }
        // Fading out (the last 0.25 s), then in (0.3 s).
        let tip_age = self.tip_time.elapsed().as_secs_f32();
        let tip_opacity = (tip_age / 0.3).min(1.0) * ((7.25 - tip_age) / 0.25).clamp(0.0, 1.0);

        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("loading")));
        let fade = |c: Color32, a: f32| c.gamma_multiply(a * opacity);
        painter.rect_filled(screen, 0.0, fade(rgb(0x0B0807), 1.0));
        let phase = time % 4.8;
        let breath = if phase < 2.4 { phase / 2.4 } else { 2.0 - phase / 2.4 };
        let glow = 0.7 + 0.3 * Ease::SineInOut.at(breath);
        quad(&painter, &textures.glow, rect_corners(screen), fade(Color32::WHITE, glow));

        // Embers rising, in a 1600x900 space that fills the view.
        let scale = (screen.width() / 1600.0).max(screen.height() / 900.0);
        let origin = screen.center() - Vec2::new(800.0, 450.0) * scale;
        for e in &self.embers {
            let t = time - e.begin;
            if t < 0.0 {
                continue;
            }
            let u = t % e.life;
            let y = -e.rise * u / e.life;
            let x = keys(u, 0.0, &[(e.life * 0.5, e.drift * 0.35, Ease::SineOut), (e.life, e.drift, Ease::SineOut)]);
            let a = keys(u, 0.0, &[(e.life * 0.15, e.peak, Ease::Linear), (e.life, 0.0, Ease::Linear)]);
            let p = origin + (e.at + Vec2::new(x, y)) * scale;
            painter.circle_filled(p, e.radius * scale, fade(e.colour, a));
        }
        quad(&painter, &textures.vignette, rect_corners(screen), fade(Color32::WHITE, 1.0));

        // The words and the bar, laid out under the icon, all centred.
        let fonts = |size: f32| FontId::proportional(size);
        let spaced =
            |text: &str, size: f32, spacing: f32, colour: Color32| -> (Vec<(std::sync::Arc<egui::Galley>, f32)>, f32) {
                let mut out = Vec::new();
                let mut w = 0.0;
                for ch in text.chars() {
                    let g = painter.layout_no_wrap(ch.to_string(), fonts(size), colour);
                    let advance = g.size().x + size * spacing;
                    out.push((g, w));
                    w += advance;
                }
                (out, w - size * spacing)
            };
        let (red, red_w) = spaced("RED", 42.0, 0.22, rgb(0xFF3B1C));
        let (forge, forge_w) = spaced("FORGE", 42.0, 0.22, rgb(0xF4E8DA));
        let (by, by_w) = spaced("by Prysma Studio", 13.0, 0.12, rgb(0xA88B74));
        let stage = painter.layout_no_wrap(label.to_owned(), fonts(13.0), rgb(0xD8C2AE));
        let tip_label = painter.layout_no_wrap("TIP  ".into(), fonts(12.0), rgb(0xFF8A3D));
        let tip = painter.layout(TIPS[self.tip].to_owned(), fonts(12.0), rgb(0x8F7A6A), 680.0 - tip_label.size().x);
        let heights = [280.0, 6.0, 50.0, 2.0, 16.0, 34.0, BAR.y, 14.0, stage.size().y, 26.0, tip.size().y];
        let mut y = screen.center().y - heights.iter().sum::<f32>() * 0.5 - 20.0;
        let cx = screen.center().x;

        // The icon: the anvil, the flash and sparks of each blow, the hammer.
        let blow = time % BLOW;
        let shake =
            keys(blow, 0.0, &[(IMPACT, 1.2, Ease::Discrete), (0.9, -0.6, Ease::Linear), (0.98, 0.0, Ease::Linear)]);
        let k = 280.0 / 256.0;
        let canvas = Pos2::new(cx - 140.0, y + shake * k);
        let to_screen = |p: Vec2| canvas + p * k;
        let full = Rect::from_min_size(to_screen(Vec2::ZERO), Vec2::splat(280.0));
        let flash = keys(blow, 0.0, &[(IMPACT, 1.0, Ease::Discrete), (1.45, 0.0, Ease::Linear)]);
        quad(&painter, &textures.flash, rect_corners(full), fade(Color32::WHITE, flash));
        quad(&painter, &textures.anvil, rect_corners(full), fade(Color32::WHITE, 1.0));
        let sparks = keys(blow, 0.0, &[(IMPACT, 1.0, Ease::Discrete), (1.3, 0.0, Ease::Linear)]);
        let grow = keys(blow, 1.0, &[(IMPACT, 0.55, Ease::Discrete), (1.1, 1.12, Ease::CubicOut)]);
        let corners = [Vec2::ZERO, Vec2::new(256.0, 0.0), Vec2::splat(256.0), Vec2::new(0.0, 256.0)];
        let grown = corners.map(|c| to_screen(SPARKS_CENTRE + (c - SPARKS_CENTRE) * grow));
        quad(&painter, &textures.sparks, grown, fade(Color32::WHITE, sparks));
        for b in &self.bits {
            if blow < IMPACT || blow > IMPACT + b.life {
                continue;
            }
            let x = keys(blow, 0.0, &[(IMPACT, 0.0, Ease::Linear), (IMPACT + b.life, b.to.x, Ease::Linear)]);
            let y = keys(
                blow,
                0.0,
                &[
                    (IMPACT, 0.0, Ease::Linear),
                    (IMPACT + b.life * 0.45, b.to.y, Ease::QuadOut),
                    (IMPACT + b.life, b.to.y + b.fall, Ease::QuadIn),
                ],
            );
            let a =
                keys(blow, 1.0, &[(IMPACT + b.life * 0.6, 1.0, Ease::Linear), (IMPACT + b.life, 0.0, Ease::Linear)]);
            painter.circle_filled(to_screen(BITS_FROM + Vec2::new(x, y)), b.radius * k, fade(b.colour, a));
        }
        // One blow: raise, hold, strike, rebound, rest.
        let angle = keys(
            blow,
            0.0,
            &[
                (0.6, -28.0, Ease::CubicOut),
                (0.7, -30.0, Ease::SineInOut),
                (IMPACT, 3.5, Ease::QuadIn),
                (0.96, -4.0, Ease::QuadOut),
                (1.1, 0.0, Ease::SineInOut),
            ],
        )
        .to_radians();
        let (sin, cos) = angle.sin_cos();
        let turned = corners.map(|c| {
            let d = c - GRIP;
            to_screen(GRIP + Vec2::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos))
        });
        quad(&painter, &textures.hammer, turned, fade(Color32::WHITE, 1.0));
        y += 280.0 + 6.0;

        // "REDFORGE", "by Prysma Studio" (the title drawn a few times over, as
        // the C#'s is a heavy face).
        let mut x = cx - (red_w + 42.0 * 0.22 + forge_w) * 0.5;
        for (list, w, colour) in [(&red, red_w, rgb(0xFF3B1C)), (&forge, forge_w, rgb(0xF4E8DA))] {
            for (g, at) in list {
                for (dx, dy) in [(0.0, 0.0), (0.7, 0.0), (1.4, 0.0), (0.7, 0.6), (0.7, -0.6)] {
                    painter.galley_with_override_text_color(
                        Pos2::new(x + at + dx, y + dy),
                        g.clone(),
                        fade(colour, 1.0),
                    );
                }
            }
            x += w + 42.0 * 0.22;
        }
        y += 50.0 + 2.0;
        let x = cx - by_w * 0.5;
        for (g, at) in &by {
            painter.galley_with_override_text_color(Pos2::new(x + at, y), g.clone(), fade(rgb(0xA88B74), 1.0));
        }
        y += 16.0 + 34.0;

        // The bar, its hot end, the stage and a tip.
        let track = Rect::from_min_size(Pos2::new(cx - BAR.x * 0.5, y), BAR);
        painter.rect(
            track,
            4.0,
            fade(rgb(0x1F1512), 1.0),
            egui::Stroke::new(1.0, fade(rgb(0x33231C), 1.0)),
            egui::StrokeKind::Inside,
        );
        let filled = Rect::from_min_size(track.min, Vec2::new(BAR.x * self.shown, BAR.y));
        quad(&painter, &textures.bar, rect_corners(filled), fade(Color32::WHITE, 1.0));
        let hot = Rect::from_center_size(Pos2::new(filled.max.x, track.center().y), Vec2::splat(26.0));
        quad(&painter, &textures.hot_spot, rect_corners(hot), fade(Color32::WHITE, 1.0));
        y += BAR.y + 14.0;
        painter.galley_with_override_text_color(
            Pos2::new(cx - stage.size().x * 0.5, y),
            stage.clone(),
            fade(rgb(0xD8C2AE), 1.0),
        );
        y += stage.size().y + 26.0;
        let w = tip_label.size().x + tip.size().x;
        let left = cx - w * 0.5;
        painter.galley_with_override_text_color(Pos2::new(left, y), tip_label, fade(rgb(0xFF8A3D), tip_opacity));
        painter.galley_with_override_text_color(
            Pos2::new(left + w - tip.size().x, y),
            tip,
            fade(rgb(0x8F7A6A), tip_opacity),
        );
        ctx.request_repaint();
    }
}
