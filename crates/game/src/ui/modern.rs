//! The modern main menu: a framed panel on the left over a painted
//! background, with the profile card, the main menu's actions in three
//! sections, CoD4's logo and the key hints (Options > Game > Menu Style,
//! `ui_menustyle`: `modern`, the default, or `classic`).
//!
//! It stands in for `main_text`. Its rows are the classic menu's own
//! buttons (as the lobby, Headquarters and supply drops dress it), found by
//! their labels and moved into the panel with their text cleared: every
//! action, visibility rule and focus sound stays the classic one, and the
//! menu code's mouse and pad navigation works as it does everywhere. Only
//! the drawing is this file's.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

/// The setting: `classic` keeps CoD4's main menu.
pub const STYLE_DVAR: &str = "ui_menustyle";

/// The menu that replaces `main_text`.
pub const MENU: &str = "main_modern";

/// Whether a controller is in use (key hints switch to its footer).
static PAD_IN_USE: AtomicBool = AtomicBool::new(false);

/// When the menu last opened (ms), for its slide-in.
static OPENED_AT: AtomicI64 = AtomicI64::new(i64::MIN);

pub(super) fn build(app: &mut App) {
    app.add_systems(Update, (track_pad, load_font));
}

/// The modern menu's font: Windows' Bahnschrift (a DIN-style sans on every
/// Windows 10 and 11 PC; read from the system, never shipped), rasterised
/// into a CoD4-style glyph atlas so the menus' text drawing takes it as is.
/// Without it, CoD4's own font.
pub(super) const FONT: &str = "modern/bahnschrift";
const FONT_MATERIAL: &str = "modern:bahnschrift";

/// The atlas's glyph height (pixels): text is drawn scaled from it.
const FONT_PX: f32 = 64.0;

fn load_font(fe: Option<ResMut<Frontend>>, mut images: ResMut<Assets<Image>>, mut tried: Local<bool>) {
    let Some(mut fe) = fe else { return };
    if std::mem::replace(&mut *tried, true) {
        return;
    }
    let windows = std::env::var("WINDIR").unwrap_or_else(|_| String::from("C:/Windows"));
    let path = std::path::Path::new(&windows).join("Fonts").join("bahnschrift.ttf");
    let Ok(bytes) = std::fs::read(&path) else {
        info!("modern menu: no {} (CoD4's font instead)", path.display());
        return;
    };
    match rasterise(bytes, &mut images) {
        Some((font, image)) => {
            fe.assets.add_image(FONT_MATERIAL, image);
            fe.assets.fonts.push(font);
        }
        None => warn!("modern menu: couldn't read {}", path.display()),
    }
}

/// Printable ASCII and a few extras into one atlas.
fn rasterise(bytes: Vec<u8>, images: &mut Assets<Image>) -> Option<(iw3::menu::Font, assets::UiImage)> {
    use ab_glyph::{Font as _, FontVec, PxScale, ScaleFont};
    let font = FontVec::try_from_vec(bytes).ok()?;
    let scaled = font.as_scaled(PxScale::from(FONT_PX));
    let chars: Vec<char> = (32u8..127).map(char::from).chain(['\u{b7}', '\u{2013}', '\u{2019}']).collect();
    const W: u32 = 1024;
    const H: u32 = 512;
    let mut pixels = vec![0u8; (W * H * 4) as usize];
    let (mut x, mut y, mut row) = (1u32, 1u32, 0u32);
    let mut glyphs = Vec::new();
    for c in chars {
        let id = font.glyph_id(c);
        let advance = scaled.h_advance(id).round().clamp(0.0, 255.0) as u8;
        let g = id.with_scale_and_position(PxScale::from(FONT_PX), ab_glyph::point(0.0, 0.0));
        let Some(outline) = font.outline_glyph(g) else {
            glyphs.push(iw3::menu::Glyph { letter: c as u16, x0: 0, y0: 0, dx: advance, pixel_width: 0, pixel_height: 0, s0: 0.0, t0: 0.0, s1: 0.0, t1: 0.0 });
            continue;
        };
        let b = outline.px_bounds();
        let (gw, gh) = (b.width().ceil() as u32, b.height().ceil() as u32);
        if x + gw + 1 >= W {
            x = 1;
            y += row + 1;
            row = 0;
        }
        if y + gh + 1 >= H {
            break;
        }
        outline.draw(|px, py, cov| {
            let i = (((y + py) * W + x + px) * 4) as usize;
            if i + 4 <= pixels.len() {
                pixels[i..i + 4].copy_from_slice(&[255, 255, 255, (cov.clamp(0.0, 1.0) * 255.0) as u8]);
            }
        });
        glyphs.push(iw3::menu::Glyph {
            letter: c as u16,
            x0: b.min.x.floor().clamp(-128.0, 127.0) as i8,
            // Relative to the baseline, which the glyph's origin sits on.
            y0: b.min.y.floor().clamp(-128.0, 127.0) as i8,
            dx: advance,
            pixel_width: gw.min(255) as u8,
            pixel_height: gh.min(255) as u8,
            s0: x as f32 / W as f32,
            t0: y as f32 / H as f32,
            s1: (x + gw) as f32 / W as f32,
            t1: (y + gh) as f32 / H as f32,
        });
        x += gw + 1;
        row = row.max(gh);
    }
    let mut image = Image::new(
        bevy::render::render_resource::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        bevy::render::render_resource::TextureDimension::D2,
        pixels,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::linear();
    let size = Vec2::new(W as f32, H as f32);
    let handle = images.add(image);
    let font = iw3::menu::Font { name: FONT.into(), pixel_height: FONT_PX as i32, material: FONT_MATERIAL.into(), glow_material: String::new(), glyphs };
    Some((font, assets::UiImage { handle, size }))
}

fn track_pad(active: Option<Res<crate::gamepad::ActiveDevice>>) {
    PAD_IN_USE.store(active.is_some_and(|a| a.pad.is_some()), Ordering::Relaxed);
}

/// Whether the modern menu is wanted.
pub(super) fn wanted(fe: &Frontend) -> bool {
    !fe.dvar(STYLE_DVAR).eq_ignore_ascii_case("classic")
}

const SECTIONS: [&str; 3] = ["MULTIPLAYER", "PLAYER", "SYSTEM"];

/// A row: its section, label, and the words in the classic button's label
/// that find it (any one, lower case).
struct Row {
    section: usize,
    label: &'static str,
    keys: &'static [&'static str],
}

const ROWS: [Row; 13] = [
    Row { section: 0, label: "Headquarters", keys: &["headquarters"] },
    Row { section: 0, label: "Join Game", keys: &["join game"] },
    Row { section: 0, label: "Private Match", keys: &["private match", "start new server"] },
    Row { section: 1, label: "Select Profile", keys: &["profile"] },
    Row { section: 1, label: "Create a Class", keys: &["create a class"] },
    Row { section: 1, label: "Character", keys: &["character"] },
    Row { section: 1, label: "Supply Drops", keys: &["supply drop"] },
    Row { section: 1, label: "Combat Record", keys: &["combat record", "rank and challenges", "rank & challenges", "challenges"] },
    Row { section: 2, label: "Controls", keys: &["controls"] },
    Row { section: 2, label: "Options", keys: &["options"] },
    Row { section: 2, label: "Mods", keys: &["mods"] },
    Row { section: 2, label: "Single Player", keys: &["single player"] },
    Row { section: 2, label: "Quit", keys: &["quit"] },
];

// Layout in CoD4's virtual units, from the left and top edges and scaled by
// the window's height (480 tall), measured off the mock-up.
const PANEL: (f32, f32, f32, f32) = (34.0, 30.0, 194.0, 396.0);
const CUT: f32 = 9.0;
const EMBLEM: (f32, f32, f32) = (78.0, 62.0, 25.0);
const NAME: (f32, f32) = (112.0, 67.0);
const HEADER_X: f32 = 60.0;
const ROW_X: f32 = 51.0;
const ROW_W: f32 = 170.0;
const ROW_H: f32 = 18.0;
const TEXT_X: f32 = 67.0;
const ROW_PITCH: f32 = 19.8;
/// Each section's header line (its first row follows it).
const SECTION_Y: [f32; 3] = [100.0, 182.0, 307.0];

/// #DDD6C4.
const INK: [f32; 4] = [0.867, 0.839, 0.769, 1.0];
/// Headers and the footer note: a dimmer tan.
const DIM: [f32; 4] = [0.66, 0.62, 0.52, 1.0];
const LINE: [f32; 4] = [0.75, 0.75, 0.68, 0.22];
const FRAME: [f32; 4] = [0.78, 0.78, 0.7, 0.55];
const FILL: [f32; 4] = [0.05, 0.055, 0.05, 0.72];
const OLIVE: [f32; 4] = [0.78, 0.8, 0.36, 1.0];
/// #8A8466 at 70%, fading out to the right.
const HIGHLIGHT: [f32; 4] = [0.54, 0.52, 0.4, 0.7];

/// The virtual rectangle of the row `i` (in `ROWS`).
fn row_rect(i: usize) -> VRect {
    let s = ROWS[i].section;
    let k = ROWS[..i].iter().filter(|r| r.section == s).count();
    let y = SECTION_Y[s] + 9.0 + k as f32 * ROW_PITCH;
    VRect { x: ROW_X, y, w: ROW_W, h: ROW_H, horz_align: 1, vert_align: 1 }
}

/// The modern menu built from the dressed `main_text`; `None` if none of
/// its buttons are found.
pub(super) fn menu(fe: &Frontend, classic: &Menu) -> Option<Menu> {
    let mut out = Menu { items: Vec::new(), full_screen: true, ..classic.clone() };
    out.window.name = MENU.into();
    let mut found = 0;
    for it in classic.items.iter().filter(|it| it.ty == item_type::BUTTON) {
        let raw = if it.text_exp.is_empty() { it.text.clone() } else { eval(&it.text_exp, fe).text() };
        let label = fe.assets.localize(&raw).to_ascii_lowercase();
        if label.trim().is_empty() {
            continue;
        }
        let Some(row) = ROWS.iter().position(|r| r.keys.iter().any(|k| label.contains(k))) else {
            debug!("modern menu: no row for {label:?}");
            continue;
        };
        found += 1;
        let mut item = it.clone();
        item.window = MenuWindow { rect: row_rect(row), dynamic_flags: flags::VISIBLE, name: format!("modern_{row}"), ..MenuWindow::default() };
        // The label stays (for finding the row), undrawn: this file draws it.
        item.text = ROWS[row].label.into();
        item.text_exp = Statement::default();
        item.text_scale = 0.0;
        out.items.push(item);
    }
    (found > 0).then_some(out)
}

/// Note the opening, for the slide-in.
pub(super) fn opened(fe: &Frontend) {
    OPENED_AT.store(fe.millis(), Ordering::Relaxed);
}

impl Frontend {
    /// The modern menu's drawing, under its (invisible) rows.
    pub(super) fn paint_modern(&self, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
        if om.menu.window.name != MENU {
            return;
        }
        let now = self.millis();
        let since = (now - OPENED_AT.load(Ordering::Relaxed)).max(0) as f32 / 1000.0;
        // A short fade and slide in.
        let t = (since / 0.35).clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - t) * (1.0 - t);
        let fade = ease;
        let slide = (1.0 - ease) * -14.0;
        let mut p = Painter { fe: self, pl, ops, dx: slide, alpha: fade };

        // Another menu open over this one (Options and its pages): its own
        // panel, logo and key hints would show through it.
        let covered = self
            .stack
            .iter()
            .skip_while(|m| !std::ptr::eq(*m, om))
            .skip(1)
            .any(|m| m.menu.visible_exp.is_empty() || eval(&m.menu.visible_exp, self).truthy());
        p.background(now);
        // Only the backdrop under another menu.
        if covered {
            return;
        }
        p.logo();
        p.panel();
        p.profile();
        for (s, name) in SECTIONS.iter().enumerate() {
            p.header(s, name);
        }
        let focused = self.focus.as_ref().filter(|(m, _)| *m == om.name).map(|(_, i)| *i);
        let unopened = crate::supply::inventory().unopened;
        for (i, row) in ROWS.iter().enumerate() {
            // Shown if any of its buttons is.
            let items: Vec<usize> = om.menu.items.iter().enumerate().filter(|(_, it)| it.window.name == format!("modern_{i}")).map(|(k, _)| k).collect();
            if !items.iter().any(|&k| self.item_visible(om, k)) {
                continue;
            }
            let hot = focused.is_some_and(|f| items.contains(&f));
            let badge = (row.label == "Supply Drops" && unopened > 0).then_some(unopened);
            p.row(i, row.label, hot, badge);
        }
        p.footer(PAD_IN_USE.load(Ordering::Relaxed));
    }
}

struct Painter<'a> {
    fe: &'a Frontend,
    pl: &'a Placement,
    ops: &'a mut Vec<Op>,
    /// The panel's slide, virtual units.
    dx: f32,
    alpha: f32,
}

impl Painter<'_> {
    fn a(&self, c: [f32; 4]) -> [f32; 4] {
        [c[0], c[1], c[2], c[3] * self.alpha]
    }

    /// Window pixels of a left/top-anchored virtual point.
    fn px(&self, x: f32, y: f32) -> Vec2 {
        Vec2::new((x + self.dx) * self.pl.scale, y * self.pl.scale)
    }

    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, c: [f32; 4]) {
        let pos = self.px(x, y);
        let c = self.a(c);
        self.ops.push(Op::Fill { pos, size: Vec2::new(w, h) * self.pl.scale, color: c });
    }

    /// A line between two virtual points, `t` virtual units thick.
    fn line(&mut self, a: (f32, f32), b: (f32, f32), t: f32, c: [f32; 4]) {
        let (pa, pb) = (self.px(a.0, a.1), self.px(b.0, b.1));
        let d = pb - pa;
        let size = Vec2::new(d.length(), (t * self.pl.scale).max(1.0));
        let pos = (pa + pb) * 0.5 - size * 0.5;
        let c = self.a(c);
        self.ops.push(Op::Image { pos, size, material: "white".into(), color: c, uv: None, rot: d.y.atan2(d.x), layer: 1 });
    }

    /// Bahnschrift if loaded, else CoD4's font for the size.
    fn font(&self, font_enum: i32, height: f32) -> usize {
        self.fe.assets.fonts.iter().position(|f| f.name == FONT).unwrap_or_else(|| self.fe.font_for(font_enum, height * self.pl.scale))
    }

    fn text(&mut self, x: f32, y: f32, height: f32, font_enum: i32, text: &str, c: [f32; 4]) {
        let font = self.font(font_enum, height);
        let k = height * self.pl.scale / self.fe.assets.fonts[font].pixel_height as f32;
        let pos = self.px(x, y);
        let c = self.a(c);
        self.ops.push(Op::Text { text: text.to_owned(), x: pos.x, y: pos.y, font, k, color: c, shadow: 0.0 });
    }

    fn text_width(&self, height: f32, font_enum: i32, text: &str) -> f32 {
        let font = self.font(font_enum, height);
        let k = height * self.pl.scale / self.fe.assets.fonts[font].pixel_height as f32;
        draw::text_width(&self.fe.assets.fonts[font], text, k) / self.pl.scale
    }

    /// Spaced capitals (headers): one letter at a time.
    fn spaced(&mut self, x: f32, y: f32, height: f32, text: &str, spacing: f32, c: [f32; 4]) {
        let mut at = x;
        for ch in text.chars() {
            let s = ch.to_string();
            self.text(at, y, height, 1, &s, c);
            at += self.text_width(height, 1, &s) + spacing;
        }
    }

    /// The painted background, cover-cropped to the window, drifting slowly,
    /// darker towards the panel.
    fn background(&mut self, now: i64) {
        let (w, h) = (self.pl.w, self.pl.h);
        const ASPECT: f32 = 1672.0 / 941.0;
        // A little larger than the window, so the drift never shows an edge.
        let zoom = 1.04;
        let (uw, uh) = if w / h > ASPECT { (1.0, (ASPECT * h / w)) } else { (w / h / ASPECT, 1.0) };
        let (uw, uh) = (uw / zoom, uh / zoom);
        let t = now as f32 / 1000.0;
        let drift = Vec2::new((t * 0.021).sin(), (t * 0.017).cos()) * 0.5;
        let centre = Vec2::splat(0.5) + drift * Vec2::new(1.0 - uw, 1.0 - uh);
        let uv = Rect::from_center_size(centre, Vec2::new(uw, uh));
        self.ops.push(Op::Image { pos: Vec2::ZERO, size: Vec2::new(w, h), material: BACKGROUND.into(), color: [1.0; 4], uv: Some(uv), rot: 0.0, layer: 1 });
        // Darker, cooler and a little greyer overall (the mock's grade).
        self.ops.push(Op::Fill { pos: Vec2::ZERO, size: Vec2::new(w, h), color: [0.11, 0.12, 0.13, 0.28] });
        // Darker still under the panel, fading out across the left half.
        let strips = 24;
        for i in 0..strips {
            let f = i as f32 / strips as f32;
            let x = f * w * 0.55;
            let a = 0.72 * (1.0 - f) * (1.0 - f);
            self.ops.push(Op::Fill { pos: Vec2::new(x, 0.0), size: Vec2::new(w * 0.55 / strips as f32 + 1.0, h), color: [0.0, 0.0, 0.0, a] });
        }
        // And a little along the bottom, under the footer.
        for i in 0..8 {
            let f = i as f32 / 8.0;
            self.ops.push(Op::Fill { pos: Vec2::new(0.0, h * (0.86 + 0.14 * f)), size: Vec2::new(w, h * 0.14 / 8.0 + 1.0), color: [0.0, 0.0, 0.0, 0.08 + 0.3 * f] });
        }
    }

    /// CoD4's logo, top right over the sky.
    fn logo(&mut self) {
        // The lettering about 22% of the width, its top at 15% of the
        // height (the picture has a wide transparent margin).
        let w = self.pl.w * 0.32;
        let size = Vec2::new(w, w * 68.0 / 278.0);
        let pos = Vec2::new(self.pl.w * 0.565, self.pl.h * 0.115);
        // A soft green glow behind "MODERN WARFARE": the logo's lower part,
        // tinted, spread a few pixels each way.
        let lower = Rect::new(0.0, 0.62, 1.0, 1.0);
        let part = Vec2::new(size.x, size.y * 0.38);
        let at = pos + Vec2::new(0.0, size.y * 0.62);
        let spread = size.y * 0.06;
        for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-0.7, -0.7), (0.7, -0.7), (-0.7, 0.7), (0.7, 0.7), (-2.0, 0.0), (2.0, 0.0)] {
            let c = self.a([0.35, 0.95, 0.35, 0.055]);
            self.ops.push(Op::Image { pos: at + Vec2::new(dx, dy) * spread, size: part, material: "logo_cod2".into(), color: c, uv: Some(lower), rot: 0.0, layer: 1 });
        }
        let c = self.a([1.0; 4]);
        self.ops.push(Op::Pic { pos, size, material: "logo_cod2".into(), color: c });
    }

    /// The panel: a dark translucent fill with cut corners, a thin bevelled
    /// frame and a few faint scratches.
    fn panel(&mut self) {
        let (x, y, w, h) = PANEL;
        // The fill in strips, the corners stepped down to the cuts.
        let steps = 6;
        for i in 0..steps {
            let inset = CUT * (1.0 - (i as f32 + 0.5) / steps as f32);
            let band = CUT / steps as f32;
            self.fill(x + inset, y + i as f32 * band, w - 2.0 * inset, band, FILL);
            self.fill(x + inset, y + h - (i as f32 + 1.0) * band, w - 2.0 * inset, band, FILL);
        }
        self.fill(x, y + CUT, w, h - 2.0 * CUT, FILL);
        // The frame: outer line and a faint inner bevel.
        for (inset, c, t) in [(0.0, FRAME, 0.8), (2.5, LINE, 0.5)] {
            let (x0, y0, x1, y1) = (x + inset, y + inset, x + w - inset, y + h - inset);
            let k = CUT - inset * 0.4;
            let pts = [
                (x0 + k, y0),
                (x1 - k, y0),
                (x1, y0 + k),
                (x1, y1 - k),
                (x1 - k, y1),
                (x0 + k, y1),
                (x0, y1 - k),
                (x0, y0 + k),
            ];
            for j in 0..pts.len() {
                self.line(pts[j], pts[(j + 1) % pts.len()], t, c);
            }
        }
        // Scratches.
        for (a, b) in [((x + 12.0, y + h - 30.0), (x + 40.0, y + h - 36.0)), ((x + w - 50.0, y + 18.0), (x + w - 20.0, y + 12.0)), ((x + w - 30.0, y + h - 60.0), (x + w - 14.0, y + h - 52.0))] {
            self.line(a, b, 0.4, [0.8, 0.8, 0.75, 0.08]);
        }
        // Small tick marks on the right edge near the top, as on gear.
        for i in 0..3 {
            let tx = x + w - 26.0 + i as f32 * 3.0;
            self.line((tx, y + 76.0), (tx, y + 80.0), 0.5, LINE);
        }
    }

    /// The emblem in an octagon, and the profile name.
    fn profile(&mut self) {
        let (cx, cy, r) = EMBLEM;
        // The octagon's fill and frame.
        let k = r * 0.41;
        let pts = [(cx - k, cy - r), (cx + k, cy - r), (cx + r, cy - k), (cx + r, cy + k), (cx + k, cy + r), (cx - k, cy + r), (cx - r, cy + k), (cx - r, cy - k)];
        self.fill(cx - r + 1.0, cy - k, 2.0 * r - 2.0, 2.0 * k, [0.0, 0.0, 0.0, 0.5]);
        // The emblem itself, inside the octagon.
        let s = r * 1.62;
        let pos = self.px(cx - s * 0.5, cy - s * 0.5);
        let c = self.a([1.0; 4]);
        self.ops.push(Op::Pic { pos, size: Vec2::splat(s * self.pl.scale), material: EMBLEM_MATERIAL.into(), color: c });
        for j in 0..pts.len() {
            self.line(pts[j], pts[(j + 1) % pts.len()], 1.2, [0.62, 0.62, 0.55, 0.9]);
        }
        let name = self.fe.profile_name().to_uppercase();
        self.spaced(NAME.0, NAME.1, 12.5, &name, 0.9, INK);
        // The rule under the name, and its end ticks.
        let rx1 = PANEL.0 + PANEL.2 - 16.0;
        self.line((NAME.0, NAME.1 + 8.0), (rx1, NAME.1 + 8.0), 0.5, LINE);
    }

    fn header(&mut self, s: usize, name: &str) {
        let y = SECTION_Y[s];
        self.spaced(HEADER_X, y, 9.5, name, 2.4, DIM);
        // A small bracket on the left, and the rule under the header.
        self.line((HEADER_X - 9.0, y - 7.0), (HEADER_X - 9.0, y + 3.0), 0.6, LINE);
        self.line((HEADER_X - 7.0, y - 7.0), (HEADER_X - 7.0, y + 3.0), 0.6, LINE);
        self.line((HEADER_X - 1.0, y + 4.0), (PANEL.0 + PANEL.2 - 14.0, y + 4.0), 0.6, LINE);
    }

    fn row(&mut self, i: usize, label: &str, hot: bool, badge: Option<u32>) {
        let r = row_rect(i);
        if hot {
            let strips = 24;
            for k in 0..strips {
                let f = k as f32 / strips as f32;
                let a = HIGHLIGHT[3] * (1.0 - f).powf(1.4) + 0.04;
                self.fill(r.x + f * r.w, r.y, r.w / strips as f32 + 0.05, r.h, [HIGHLIGHT[0], HIGHLIGHT[1], HIGHLIGHT[2], a]);
            }
            self.line((r.x, r.y), (r.x + r.w, r.y), 0.5, [0.85, 0.82, 0.7, 0.2]);
            self.line((r.x, r.y + r.h), (r.x + r.w, r.y + r.h), 0.5, [0.85, 0.82, 0.7, 0.2]);
            self.fill(r.x, r.y, 1.5, r.h, OLIVE);
            // The chevron.
            let (cx, cy) = (r.x + r.w - 7.0, r.y + r.h * 0.5);
            self.line((cx - 2.5, cy - 4.0), (cx + 0.6, cy + 0.4), 0.9, INK);
            self.line((cx - 2.5, cy + 4.0), (cx + 0.6, cy - 0.4), 0.9, INK);
        }
        // The thin tan rule under each row.
        self.line((r.x + 14.0, r.y + r.h + 0.6), (r.x + r.w, r.y + r.h + 0.6), 0.5, [0.7, 0.65, 0.52, 0.22]);
        self.text(TEXT_X, r.y + r.h * 0.5 + 4.6, 12.8, 1, label, if hot { [1.0, 0.98, 0.92, 1.0] } else { INK });
        if let Some(n) = badge {
            let text = n.to_string();
            let tw = self.text_width(11.0, 1, &text);
            let bw = (tw + 8.0).max(18.0);
            let (bx, by) = (r.x + r.w - 26.0 - bw * 0.5, r.y + 2.5);
            self.fill(bx, by, bw, r.h - 5.0, [0.0, 0.0, 0.0, 0.45]);
            for (a, b) in [((bx, by), (bx + bw, by)), ((bx, by + r.h - 5.0), (bx + bw, by + r.h - 5.0)), ((bx, by), (bx, by + r.h - 5.0)), ((bx + bw, by), (bx + bw, by + r.h - 5.0))] {
                self.line(a, b, 0.6, FRAME);
            }
            self.text(bx + (bw - tw) * 0.5, by + (r.h - 5.0) * 0.5 + 4.0, 11.0, 1, &text, INK);
        }
    }

    /// The online note, and (without a pad: its own footer shows then) the
    /// key hints.
    fn footer(&mut self, pad: bool) {
        let dx = self.dx;
        self.dx = 0.0;
        self.text(PANEL.0, 456.0, 10.5, 1, "Game experience may change during online play.", DIM);
        if !pad {
            let right = self.pl.w / self.pl.scale;
            let mut x = right - 34.0;
            for (key, what) in [("ESC", "Back"), ("ENTER", "Select")] {
                let ww = self.text_width(10.5, 1, what);
                x -= ww;
                self.text(x, 456.0, 10.5, 1, what, INK);
                let kw = self.text_width(9.5, 1, key) + 10.0;
                x -= kw + 8.0;
                let (top, bottom) = (442.0, 461.0);
                self.fill(x, top, kw, bottom - top, [0.0, 0.0, 0.0, 0.5]);
                for (a, b) in [((x, top), (x + kw, top)), ((x, bottom), (x + kw, bottom)), ((x, top), (x, bottom)), ((x + kw, top), (x + kw, bottom))] {
                    self.line(a, b, 0.6, FRAME);
                }
                self.text(x + 5.0, 455.6, 9.5, 1, key, INK);
                x -= 26.0;
            }
        }
        self.dx = dx;
    }
}

/// The background picture and the emblem, as UI materials (see
/// `UiAssets::material`).
pub(super) const BACKGROUND: &str = "modern:background";
pub(super) const EMBLEM_MATERIAL: &str = "modern:emblem";

/// The embedded pictures for those materials.
pub(super) fn picture(name: &str) -> Option<&'static [u8]> {
    match name {
        "modern:background" => Some(include_bytes!("../../assets/ui/menu_background.png")),
        "modern:emblem" => Some(include_bytes!("../../../launcher/assets/icon.png")),
        _ => None,
    }
}
