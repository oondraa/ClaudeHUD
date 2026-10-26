//! The visual language of the design: frosted "liquid glass" surfaces, pill
//! buttons, avatars and the small vector icons, drawn with egui's painter.

use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CursorIcon, FontData, FontDefinitions, FontFamily, FontId, Galley, Id, Mesh,
    Painter, Pos2, Rect, Sense, Shadow, Shape, Stroke, StrokeKind, pos2, vec2,
};
use egui::FontTweak;
use egui::epaint::text::VariationCoords;

const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, a)
}

const fn white(a: f32) -> Color32 {
    rgba(255, 255, 255, (a * 255.0) as u8)
}

pub const TEXT: Color32 = Color32::WHITE;
/// Label colors (iOS-style secondary/tertiary on dark glass).
pub const TEXT_2: Color32 = rgba(235, 235, 245, 184);
pub const TEXT_3: Color32 = rgba(235, 235, 245, 158);

pub const GREEN: Color32 = Color32::from_rgb(0x30, 0xD1, 0x58);
pub const GREEN_INK: Color32 = Color32::from_rgb(0x05, 0x2E, 0x12);
pub const CHECK_INK: Color32 = Color32::from_rgb(0x06, 0x29, 0x0F);
pub const AMBER: Color32 = Color32::from_rgb(0xFF, 0x9F, 0x0A);
pub const AMBER_INK: Color32 = Color32::from_rgb(0x3A, 0x22, 0x00);
pub const AMBER_TEXT: Color32 = Color32::from_rgb(0xFF, 0xC2, 0x66);
pub const CODE: Color32 = Color32::from_rgb(0xFF, 0xD9, 0xA0);
pub const RED: Color32 = Color32::from_rgb(0xFF, 0x45, 0x3A);
pub const RED_INK: Color32 = Color32::from_rgb(0x2B, 0x04, 0x02);
pub const BLUE: Color32 = Color32::from_rgb(0x0A, 0x84, 0xFF);
pub const BLUE_TAG: Color32 = Color32::from_rgb(0x40, 0x9C, 0xFF);
/// The dark ring around avatar badges.
pub const BADGE_RING: Color32 = Color32::from_rgb(0x1C, 0x1E, 0x28);

/// Without a live backdrop blur the glass has to be far more opaque than the
/// design's 40 % to stay readable over arbitrary windows.
const GLASS_FILL: Color32 = rgba(26, 28, 40, 232);
const GLASS_BORDER: Color32 = white(0.16);

pub const FILL_SUBTLE: Color32 = white(0.07);
pub const LINE_SUBTLE: Color32 = white(0.08);
pub const WELL: Color32 = rgba(0, 0, 0, 64);

/// Avatar colors, picked by a hash of the session's folder name.
const AVATARS: [Color32; 5] = [
    Color32::from_rgb(0x5E, 0x5C, 0xE6),
    Color32::from_rgb(0xBF, 0x5A, 0xF2),
    Color32::from_rgb(0xA2, 0x84, 0x5E),
    Color32::from_rgb(0x32, 0xAD, 0xE6),
    Color32::from_rgb(0xAC, 0x5B, 0x8C),
];

// ---------------------------------------------------------------------------
// Fonts

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
    Heavy,
}

impl Weight {
    const ALL: [Weight; 5] = [Weight::Regular, Weight::Medium, Weight::Semibold, Weight::Bold, Weight::Heavy];

    fn name(self) -> &'static str {
        match self {
            Weight::Regular => "inter-400",
            Weight::Medium => "inter-500",
            Weight::Semibold => "inter-600",
            Weight::Bold => "inter-700",
            Weight::Heavy => "inter-800",
        }
    }

    fn wght(self) -> f32 {
        match self {
            Weight::Regular => 400.0,
            Weight::Medium => 500.0,
            Weight::Semibold => 600.0,
            Weight::Bold => 700.0,
            Weight::Heavy => 800.0,
        }
    }
}

pub fn font(size: f32, weight: Weight) -> FontId {
    FontId::new(size, FontFamily::Name(weight.name().into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Inter (bundled, variable — one instance per weight) for text, JetBrains
/// Mono from the system when installed (Consolas otherwise) for code.
pub fn install_fonts(ctx: &egui::Context) {
    const INTER: &[u8] = include_bytes!("../assety/fonts/Inter.ttf");
    let mut defs = FontDefinitions::default();
    let fallbacks: Vec<String> = defs.families[&FontFamily::Proportional].clone();
    for w in Weight::ALL {
        let tweak = FontTweak { coords: VariationCoords::new([(b"wght", w.wght())]), ..Default::default() };
        defs.font_data.insert(w.name().into(), Arc::new(FontData::from_static(INTER).tweak(tweak)));
        let mut family = vec![w.name().to_string()];
        family.extend(fallbacks.iter().cloned());
        defs.families.insert(FontFamily::Name(w.name().into()), family);
    }
    defs.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, Weight::Regular.name().into());

    let fonts = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_default().join("Fonts");
    let mono_file = ["JetBrainsMonoNLNerdFontMono-Regular.ttf", "JetBrainsMono-Regular.ttf", "consola.ttf"]
        .iter()
        .find_map(|f| std::fs::read(fonts.join(f)).ok());
    if let Some(bytes) = mono_file {
        defs.font_data.insert("mono".into(), Arc::new(FontData::from_owned(bytes)));
        defs.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, "mono".into());
    }
    ctx.set_fonts(defs);
}

// ---------------------------------------------------------------------------
// Text

/// Lays out `text` on one line, ellipsized to `max_w`.
pub fn fit(p: &Painter, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<Galley> {
    let g = p.layout_no_wrap(text.to_string(), font.clone(), color);
    if g.size().x <= max_w {
        return g;
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let s: String = chars[..mid].iter().collect::<String>() + "…";
        if p.layout_no_wrap(s, font.clone(), color).size().x <= max_w { lo = mid } else { hi = mid - 1 }
    }
    p.layout_no_wrap(chars[..lo].iter().collect::<String>().trim_end().to_string() + "…", font, color)
}

pub fn text_width(p: &Painter, text: &str, font: FontId) -> f32 {
    p.layout_no_wrap(text.to_string(), font, Color32::WHITE).size().x
}

/// Draws one line of text anchored at `pos`; returns its rect.
pub fn text(p: &Painter, pos: Pos2, anchor: Align2, s: &str, font: FontId, color: Color32, max_w: f32) -> Rect {
    let g = fit(p, s, font, color, max_w);
    let rect = anchor.anchor_size(pos, g.size());
    p.galley(rect.min, g, color);
    rect
}

// ---------------------------------------------------------------------------
// Glass

/// x-extent of a rounded rect at height `y`.
fn span_at(rect: Rect, r: f32, y: f32) -> (f32, f32) {
    let dy = if y < rect.top() + r {
        rect.top() + r - y
    } else if y > rect.bottom() - r {
        y - (rect.bottom() - r)
    } else {
        0.0
    };
    let dx = r - (r * r - dy.min(r) * dy.min(r)).max(0.0).sqrt();
    (rect.left() + dx, rect.right() - dx)
}

/// The 160° white sheen across the top of a glass surface, as a fine mesh
/// so the gradient follows the rounded outline.
fn sheen(p: &Painter, rect: Rect, r: f32, strength: f32) {
    let rect = rect.shrink(1.0);
    let r = (r - 1.0).max(0.0);
    let dir = vec2(20f32.to_radians().sin(), 20f32.to_radians().cos());
    let len = rect.width() * dir.x.abs() + rect.height() * dir.y.abs();
    let start = rect.center() - dir * len / 2.0;
    let alpha = |q: Pos2| {
        let t = (q - start).dot(dir) / len;
        (strength * (1.0 - t / 0.38)).max(0.0)
    };
    let bottom = (rect.top() + rect.height() * 0.55).min(rect.bottom());
    let (rows, cols) = (((bottom - rect.top()) / 3.0).ceil().max(1.0) as usize, 10usize);
    let mut mesh = Mesh::default();
    for i in 0..=rows {
        let y = rect.top() + (bottom - rect.top()) * i as f32 / rows as f32;
        let (l, rr) = span_at(rect, r, y);
        for j in 0..=cols {
            let q = pos2(l + (rr - l) * j as f32 / cols as f32, y);
            // Fade out towards the mesh's lower edge so it never ends in a seam.
            let fade = 1.0 - i as f32 / rows as f32;
            mesh.colored_vertex(q, white((alpha(q) * fade * fade).min(1.0)));
        }
    }
    let w = cols as u32 + 1;
    for i in 0..rows as u32 {
        for j in 0..cols as u32 {
            let a = i * w + j;
            mesh.add_triangle(a, a + 1, a + w);
            mesh.add_triangle(a + 1, a + w + 1, a + w);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// A frosted glass surface (`.lg` in the design).
pub fn glass(p: &Painter, rect: Rect, r: f32, border: Option<Color32>) {
    // Kept within ~10 points below the shape: the island sits only 12 points
    // above the taskbar, and a shadow cut off by the window edge shows as a
    // hard-edged box around the glass.
    let shadow = Shadow { offset: [0, 3], blur: 12, spread: 0, color: Color32::from_black_alpha(96) };
    p.add(shadow.as_shape(rect, r));
    let close = Shadow { offset: [0, 1], blur: 4, spread: 0, color: Color32::from_black_alpha(64) };
    p.add(close.as_shape(rect, r));
    p.rect_filled(rect, r, GLASS_FILL);
    sheen(p, rect, r, 0.16);
    p.rect_stroke(rect, r, Stroke::new(1.0, border.unwrap_or(GLASS_BORDER)), StrokeKind::Inside);
}

/// Calm11's dock and clock bar (VidleUITweaker), so the island sits next to
/// them as one family: flat, 80 % opaque, hairline border, 18 px corners.
const BAR_FILL: Color32 = rgba(0x1E, 0x1F, 0x24, 204);
const BAR_BORDER: Color32 = white(0.14);
pub const BAR_RADIUS: f32 = 18.0;

pub fn bar(p: &Painter, rect: Rect, border: Option<Color32>) {
    p.rect_filled(rect, BAR_RADIUS, BAR_FILL);
    p.rect_stroke(rect, BAR_RADIUS, Stroke::new(1.0, border.unwrap_or(BAR_BORDER)), StrokeKind::Inside);
}

// ---------------------------------------------------------------------------
// Controls

pub enum ButtonStyle {
    /// Solid color fill with dark ink text.
    Solid(Color32, Color32),
    /// The translucent glass button (`.lgb`).
    Glass,
}

/// A pill button; returns true when clicked.
pub fn button(ui: &egui::Ui, rect: Rect, id: Id, label: &str, size: f32, style: ButtonStyle) -> bool {
    let resp = ui.interact(rect, id, Sense::click());
    let hovered = resp.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let p = ui.painter();
    let r = rect.height() / 2.0;
    let pressed = resp.is_pointer_button_down_on();
    match style {
        ButtonStyle::Solid(fill, ink) => {
            let fill = if pressed {
                fill.gamma_multiply(0.85)
            } else if hovered {
                lighten(fill, 0.12)
            } else {
                fill
            };
            p.rect_filled(rect, r, fill);
            p.line_segment(
                [pos2(rect.left() + r, rect.top() + 1.0), pos2(rect.right() - r, rect.top() + 1.0)],
                Stroke::new(1.0, white(0.45)),
            );
            p.text(rect.center(), Align2::CENTER_CENTER, label, font(size, Weight::Bold), ink);
        }
        ButtonStyle::Glass => {
            let fill = if pressed { white(0.08) } else if hovered { white(0.2) } else { white(0.12) };
            p.rect_filled(rect, r, fill);
            p.line_segment(
                [pos2(rect.left() + r, rect.top() + 1.0), pos2(rect.right() - r, rect.top() + 1.0)],
                Stroke::new(1.0, white(0.28)),
            );
            p.rect_stroke(rect, r, Stroke::new(1.0, white(0.2)), StrokeKind::Inside);
            p.text(rect.center(), Align2::CENTER_CENTER, label, font(size, Weight::Semibold), TEXT);
        }
    }
    resp.clicked()
}

/// A round glass icon button; `icon` draws into the given center.
pub fn icon_button(ui: &egui::Ui, rect: Rect, id: Id, icon: impl FnOnce(&Painter, Pos2)) -> bool {
    let resp = ui.interact(rect, id, Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let p = ui.painter();
    let fill = if resp.hovered() { white(0.2) } else { white(0.12) };
    p.circle_filled(rect.center(), rect.width() / 2.0, fill);
    p.circle_stroke(rect.center(), rect.width() / 2.0 - 0.5, Stroke::new(1.0, white(0.2)));
    icon(p, rect.center());
    resp.clicked()
}

/// iOS-style switch, 51×31; returns true when toggled.
pub fn switch(ui: &egui::Ui, rect: Rect, id: Id, on: bool) -> bool {
    let resp = ui.interact(rect, id, Sense::click());
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    let t = ui.ctx().animate_bool_with_time(id, on, 0.15);
    let p = ui.painter();
    let off = rgba(120, 120, 128, 102);
    p.rect_filled(rect, rect.height() / 2.0, lerp(off, GREEN, t));
    let knob_r = 13.5;
    let x = egui::lerp((rect.left() + 2.0 + knob_r)..=(rect.right() - 2.0 - knob_r), t);
    let c = pos2(x, rect.center().y);
    p.circle_filled(c + vec2(0.0, 2.0), knob_r + 1.0, Color32::from_black_alpha(50));
    p.circle_filled(c, knob_r, Color32::WHITE);
    resp.clicked()
}

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

fn lighten(c: Color32, t: f32) -> Color32 {
    lerp(c, Color32::WHITE, t)
}

// ---------------------------------------------------------------------------
// Avatars & icons

pub fn avatar_color(name: &str) -> Color32 {
    let h = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32);
    AVATARS[h as usize % AVATARS.len()]
}

/// Two letters for a folder name: "OZBEAT" → OZ, "ClaudeHUD" → CH, "my-app" → MA.
pub fn initials(name: &str) -> String {
    let chars: Vec<char> = name.chars().filter(|c| c.is_alphanumeric() || "-_ .".contains(*c)).collect();
    let first = chars.iter().find(|c| c.is_alphanumeric()).copied().unwrap_or('?');
    let start = chars.iter().position(|c| c.is_alphanumeric()).unwrap_or(0);
    // A camel-case hump (lowercase followed by uppercase).
    let hump = chars.windows(2).skip(start).find(|w| w[0].is_lowercase() && w[1].is_uppercase()).map(|w| w[1]);
    // The first letter of the second word.
    let word = chars
        .windows(2)
        .skip(start)
        .find(|w| "-_ .".contains(w[0]) && w[1].is_alphanumeric())
        .map(|w| w[1]);
    let second = hump
        .or(word)
        .or_else(|| chars.iter().skip(start + 1).find(|c| c.is_alphanumeric()).copied());
    first.to_uppercase().chain(second.into_iter().flat_map(char::to_uppercase)).collect()
}

/// A rounded-square avatar with initials. `ring` draws the stacked-avatar border.
pub fn avatar(p: &Painter, rect: Rect, r: f32, name: &str, text_size: f32, ring: bool) {
    if ring {
        p.rect_filled(rect.expand(2.0), r + 2.0, rgba(20, 22, 32, 178));
    }
    p.rect_filled(rect, r, avatar_color(name));
    p.text(rect.center(), Align2::CENTER_CENTER, initials(name), font(text_size, Weight::Bold), TEXT);
}

/// Polyline in a 24-unit icon box centered on `c`, `size` points wide.
fn icon_path(p: &Painter, c: Pos2, size: f32, pts: &[(f32, f32)], width: f32, color: Color32) {
    let s = size / 24.0;
    let pts: Vec<Pos2> = pts.iter().map(|&(x, y)| c + vec2((x - 12.0) * s, (y - 12.0) * s)).collect();
    let w = width * s;
    for q in [pts[0], pts[pts.len() - 1]] {
        p.circle_filled(q, w / 2.0, color);
    }
    for q in &pts[1..pts.len() - 1] {
        p.circle_filled(*q, w / 2.0, color);
    }
    p.add(Shape::line(pts, Stroke::new(w, color)));
}

pub fn check(p: &Painter, c: Pos2, size: f32, color: Color32) {
    icon_path(p, c, size, &[(5.0, 12.5), (9.5, 17.0), (19.0, 7.5)], 3.5, color);
}

pub fn cross(p: &Painter, c: Pos2, size: f32, color: Color32) {
    icon_path(p, c, size, &[(6.0, 6.0), (18.0, 18.0)], 2.5, color);
    icon_path(p, c, size, &[(18.0, 6.0), (6.0, 18.0)], 2.5, color);
}

pub fn chevron_down(p: &Painter, c: Pos2, size: f32, color: Color32) {
    icon_path(p, c, size, &[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)], 2.5, color);
}

pub fn arrow_down(p: &Painter, c: Pos2, size: f32, color: Color32) {
    icon_path(p, c, size, &[(12.0, 4.0), (12.0, 18.0)], 2.5, color);
    icon_path(p, c, size, &[(6.0, 12.5), (12.0, 18.5), (18.0, 12.5)], 2.5, color);
}

pub fn chevron_right(p: &Painter, c: Pos2, size: f32, color: Color32) {
    icon_path(p, c, size, &[(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)], 2.5, color);
}

pub fn gear(p: &Painter, c: Pos2, size: f32, color: Color32) {
    let s = size / 24.0;
    let stroke = Stroke::new(2.0 * s, color);
    p.circle_stroke(c, 3.0 * s, stroke);
    let teeth = 8;
    let mut pts = Vec::new();
    for i in 0..teeth * 4 {
        let a = (i as f32 + 0.5) / (teeth * 4) as f32 * std::f32::consts::TAU;
        let r = if (i / 2) % 2 == 0 { 9.5 } else { 7.0 };
        pts.push(c + vec2(a.cos(), a.sin()) * r * s);
    }
    p.add(Shape::closed_line(pts, stroke));
}

/// A colored disc with a check / "!" — the status badge.
pub fn badge_check(p: &Painter, c: Pos2, d: f32, ring: bool) {
    if ring {
        p.circle_filled(c, d / 2.0 + 2.0, BADGE_RING);
    }
    p.circle_filled(c, d / 2.0, GREEN);
    check(p, c, d * 0.55, CHECK_INK);
}

pub fn badge_bang(p: &Painter, c: Pos2, d: f32, ring: bool) {
    if ring {
        p.circle_filled(c, d / 2.0 + 2.0, BADGE_RING);
    }
    p.circle_filled(c, d / 2.0, RED);
    p.text(c + vec2(0.0, 0.5), Align2::CENTER_CENTER, "!", font(d * 0.66, Weight::Heavy), RED_INK);
}

/// The pulsing amber dot (`.pulse`: opacity 1→.4, scale 1→.75 over 1.4 s).
pub fn pulse_dot(p: &Painter, c: Pos2, d: f32, time: f64, ring: bool) {
    let phase = ((time % 1.4) / 1.4) as f32;
    let k = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
    let scale = 1.0 - 0.25 * k;
    let alpha = 1.0 - 0.6 * k;
    if ring {
        p.circle_filled(c, (d / 2.0 + 2.0) * scale, BADGE_RING.gamma_multiply(alpha));
    }
    p.circle_filled(c, d / 2.0 * scale, AMBER.gamma_multiply(alpha));
}

/// The working spinner: faint ring with a quarter arc, one turn per 1.1 s.
pub fn spinner(p: &Painter, c: Pos2, size: f32, width: f32, color: Color32, time: f64) {
    let r = size / 2.0 - width / 2.0;
    p.circle_stroke(c, r, Stroke::new(width, white(0.18)));
    let start = ((time % 1.1) / 1.1) as f32 * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
    let pts: Vec<Pos2> = (0..=16)
        .map(|i| {
            let a = start + i as f32 / 16.0 * std::f32::consts::FRAC_PI_2;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.circle_filled(pts[0], width / 2.0, color);
    p.circle_filled(pts[16], width / 2.0, color);
    p.add(Shape::line(pts, Stroke::new(width, color)));
}

/// A `<kbd>` key chip; returns its width.
pub fn kbd(p: &Painter, right_center: Pos2, label: &str) -> f32 {
    let w = text_width(p, label, mono(12.0)) + 16.0;
    let rect = Rect::from_min_size(right_center - vec2(w, 13.0), vec2(w, 26.0));
    p.rect_filled(rect, 8.0, white(0.1));
    p.rect_stroke(rect, 8.0, Stroke::new(1.0, white(0.16)), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, label, mono(12.0), TEXT);
    w
}
