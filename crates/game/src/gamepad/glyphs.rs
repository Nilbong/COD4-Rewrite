//! Xbox and PlayStation button glyphs, drawn at startup. The installed games
//! only carry a few console button images, so every glyph is drawn here from
//! signed distance shapes: Xbox's lettered face buttons, PlayStation's
//! cross, circle, square and triangle, bumpers, triggers, sticks, the D-pad
//! and the View/Menu (Share/Options) buttons. Letters on top (A, LB, L2 ...)
//! are UI text.

use super::PadKind;
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;

/// A button to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Glyph {
    /// A / cross.
    South,
    /// B / circle.
    East,
    /// X / square.
    West,
    /// Y / triangle.
    North,
    LeftBumper,
    RightBumper,
    LeftTrigger,
    RightTrigger,
    LeftStick,
    RightStick,
    LeftStickClick,
    RightStickClick,
    DPad,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
    /// View / Share.
    Select,
    /// Menu / Options.
    Start,
}

impl Glyph {
    pub const ALL: [Glyph; 19] = [
        Glyph::South,
        Glyph::East,
        Glyph::West,
        Glyph::North,
        Glyph::LeftBumper,
        Glyph::RightBumper,
        Glyph::LeftTrigger,
        Glyph::RightTrigger,
        Glyph::LeftStick,
        Glyph::RightStick,
        Glyph::LeftStickClick,
        Glyph::RightStickClick,
        Glyph::DPad,
        Glyph::DPadUp,
        Glyph::DPadDown,
        Glyph::DPadLeft,
        Glyph::DPadRight,
        Glyph::Select,
        Glyph::Start,
    ];
}

/// One drawn glyph: its image (`aspect` wide per unit of height) and the
/// text drawn over it.
#[derive(Clone, Debug)]
pub struct Art {
    pub image: Handle<Image>,
    pub aspect: f32,
    /// Text, its colour and its size as a fraction of the glyph's height.
    pub label: Option<(&'static str, Color, f32)>,
}

#[derive(Resource, Default)]
pub struct Glyphs(HashMap<(PadKind, Glyph), Art>);

impl Glyphs {
    pub fn get(&self, kind: PadKind, glyph: Glyph) -> Option<&Art> {
        self.0.get(&(kind, glyph))
    }

    /// Spawn `glyph`, `height` pixels tall, under `parent`.
    pub fn spawn(&self, commands: &mut Commands, parent: Entity, kind: PadKind, glyph: Glyph, height: f32) {
        let Some(art) = self.get(kind, glyph) else { return };
        let node = commands
            .spawn((
                Node {
                    width: px(height * art.aspect),
                    height: px(height),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    flex_shrink: 0.0,
                    ..default()
                },
                ImageNode::new(art.image.clone()),
                ChildOf(parent),
            ))
            .id();
        if let Some((text, color, size)) = art.label {
            commands.spawn((
                Text::new(text),
                TextFont { font_size: FontSize::Px((height * size).round()), ..default() },
                TextColor(color),
                TextLayout::justify(Justify::Center),
                ChildOf(node),
            ));
        }
    }
}

/// Image height in pixels; glyphs show at 20-40.
const SIZE: usize = 96;

const DARK: [f32; 4] = [0.09, 0.09, 0.10, 0.92];
const RIM: [f32; 4] = [0.78, 0.78, 0.80, 1.0];
const RIM_WIDTH: f32 = 0.07;
const ICON: [f32; 4] = [0.93, 0.93, 0.93, 1.0];
const DIM: [f32; 4] = [0.42, 0.42, 0.45, 1.0];

const XBOX_GREEN: Color = Color::srgb(0.42, 0.78, 0.27);
const XBOX_RED: Color = Color::srgb(0.93, 0.27, 0.22);
const XBOX_BLUE: Color = Color::srgb(0.22, 0.53, 0.96);
const XBOX_YELLOW: Color = Color::srgb(0.99, 0.79, 0.16);
const PS_BLUE: [f32; 4] = [0.49, 0.70, 0.96, 1.0];
const PS_RED: [f32; 4] = [0.96, 0.38, 0.41, 1.0];
const PS_PINK: [f32; 4] = [0.91, 0.52, 0.86, 1.0];
const PS_GREEN: [f32; 4] = [0.27, 0.86, 0.72, 1.0];

/// Startup: every glyph for both pads: the button art
/// (`assets/ui/buttons`), else drawn here.
pub fn build(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    let mut glyphs = Glyphs::default();
    let atlases = [(PadKind::Xbox, atlas(XBOX_ATLAS)), (PadKind::PlayStation, atlas(PS_ATLAS))];
    for (kind, sheet) in &atlases {
        for glyph in Glyph::ALL {
            let art = sheet.as_ref().and_then(|_| sprite_of(*kind, glyph)).and_then(|name| {
                let platform = if *kind == PadKind::PlayStation { "playstation" } else { "xbox" };
                let rect = SPRITES.iter().find(|(p, n, _)| *p == platform && *n == name)?.2;
                let image = crop(sheet.as_ref()?, rect, ART_HEIGHT)?;
                let aspect = image.width() as f32 / image.height() as f32;
                Some(Art { image: images.add(image), aspect, label: None })
            });
            let art = art.unwrap_or_else(|| {
                let (canvas, label) = draw(*kind, glyph);
                let aspect = canvas.w as f32 / canvas.h as f32;
                Art { image: images.add(canvas.image()), aspect, label }
            });
            glyphs.0.insert((*kind, glyph), art);
        }
    }
    commands.insert_resource(glyphs);
}

/// The button art's atlases and their sprites (`controller-buttons-manifest.json`).
const XBOX_ATLAS: &[u8] = include_bytes!("../../assets/ui/buttons/xbox-buttons.png");
const PS_ATLAS: &[u8] = include_bytes!("../../assets/ui/buttons/playstation-buttons.png");
const SPRITES: &[(&str, &str, [u32; 4])] = &[
    ("playstation", "circle", [343, 60, 275, 267]),
    ("playstation", "create", [941, 670, 272, 189]),
    ("playstation", "cross", [45, 60, 270, 268]),
    ("playstation", "dpad_down", [351, 924, 274, 256]),
    ("playstation", "dpad_left", [628, 922, 288, 280]),
    ("playstation", "dpad_right", [943, 922, 254, 312]),
    ("playstation", "dpad_up", [55, 924, 257, 259]),
    ("playstation", "l1", [0, 395, 327, 206]),
    ("playstation", "l2", [642, 343, 268, 260]),
    ("playstation", "l3", [43, 624, 269, 276]),
    ("playstation", "options", [642, 671, 280, 225]),
    ("playstation", "r1", [334, 395, 295, 214]),
    ("playstation", "r2", [943, 356, 271, 245]),
    ("playstation", "r3", [345, 628, 270, 265]),
    ("playstation", "square", [640, 61, 270, 267]),
    ("playstation", "triangle", [924, 61, 287, 267]),
    ("xbox", "a", [49, 70, 278, 258]),
    ("xbox", "b", [327, 71, 276, 256]),
    ("xbox", "dpad_down", [357, 906, 242, 328]),
    ("xbox", "dpad_left", [662, 923, 271, 331]),
    ("xbox", "dpad_right", [966, 906, 256, 322]),
    ("xbox", "dpad_up", [0, 913, 283, 341]),
    ("xbox", "lb", [0, 387, 312, 233]),
    ("xbox", "ls", [18, 638, 307, 259]),
    ("xbox", "lt", [648, 340, 281, 280]),
    ("xbox", "menu", [630, 684, 288, 176]),
    ("xbox", "rb", [342, 340, 287, 244]),
    ("xbox", "rs", [325, 640, 302, 266]),
    ("xbox", "rt", [929, 355, 311, 251]),
    ("xbox", "view", [939, 682, 285, 198]),
    ("xbox", "x", [626, 44, 280, 296]),
    ("xbox", "y", [928, 37, 284, 297]),
];

/// The art's height once cut out (it shows at 20-40 pixels).
const ART_HEIGHT: u32 = 96;

/// Which sprite shows a glyph; `None` keeps the drawn one.
fn sprite_of(kind: PadKind, glyph: Glyph) -> Option<&'static str> {
    use Glyph::*;
    Some(match (kind, glyph) {
        (PadKind::PlayStation, South) => "cross",
        (PadKind::PlayStation, East) => "circle",
        (PadKind::PlayStation, West) => "square",
        (PadKind::PlayStation, North) => "triangle",
        (PadKind::PlayStation, LeftBumper) => "l1",
        (PadKind::PlayStation, RightBumper) => "r1",
        (PadKind::PlayStation, LeftTrigger) => "l2",
        (PadKind::PlayStation, RightTrigger) => "r2",
        (PadKind::PlayStation, LeftStick | LeftStickClick) => "l3",
        (PadKind::PlayStation, RightStick | RightStickClick) => "r3",
        (PadKind::PlayStation, Select) => "create",
        (PadKind::PlayStation, Start) => "options",
        (PadKind::Xbox, South) => "a",
        (PadKind::Xbox, East) => "b",
        (PadKind::Xbox, West) => "x",
        (PadKind::Xbox, North) => "y",
        (PadKind::Xbox, LeftBumper) => "lb",
        (PadKind::Xbox, RightBumper) => "rb",
        (PadKind::Xbox, LeftTrigger) => "lt",
        (PadKind::Xbox, RightTrigger) => "rt",
        (PadKind::Xbox, LeftStick | LeftStickClick) => "ls",
        (PadKind::Xbox, RightStick | RightStickClick) => "rs",
        (PadKind::Xbox, Select) => "view",
        (PadKind::Xbox, Start) => "menu",
        (_, DPadUp) => "dpad_up",
        (_, DPadDown) => "dpad_down",
        (_, DPadLeft) => "dpad_left",
        (_, DPadRight) => "dpad_right",
        (_, DPad) => return None,
    })
}

/// An atlas, decoded to RGBA (`None` if it can't be).
fn atlas(bytes: &[u8]) -> Option<image::RgbaImage> {
    match image::load_from_memory(bytes) {
        Ok(i) => Some(i.to_rgba8()),
        Err(e) => {
            warn!("button art: {e}");
            None
        }
    }
}

/// A sprite cut out of the atlas and scaled to `height` pixels.
fn crop(sheet: &image::RgbaImage, [x, y, w, h]: [u32; 4], height: u32) -> Option<Image> {
    if w == 0 || h == 0 || x + w > sheet.width() || y + h > sheet.height() {
        return None;
    }
    let cut = image::imageops::crop_imm(sheet, x, y, w, h).to_image();
    let width = ((w as f32 * height as f32 / h as f32).round() as u32).max(1);
    let small = image::imageops::resize(&cut, width, height, image::imageops::FilterType::Triangle);
    let mut img = Image::new(
        Extent3d { width, height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        small.into_raw(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = bevy::image::ImageSampler::linear();
    Some(img)
}

/// A glyph's picture and label.
fn draw(kind: PadKind, glyph: Glyph) -> (Canvas, Option<(&'static str, Color, f32)>) {
    use Glyph::*;
    let ps = kind == PadKind::PlayStation;
    let white = Color::WHITE;
    match glyph {
        South | East | West | North => {
            let mut c = Canvas::new(1.0);
            c.button_circle();
            if ps {
                let th = 0.085;
                match glyph {
                    South => c.fill(PS_BLUE, |p| segment(p, Vec2::splat(-0.4), Vec2::splat(0.4)).min(segment(p, Vec2::new(-0.4, 0.4), Vec2::new(0.4, -0.4))) - th),
                    East => c.fill(PS_RED, |p| (p.length() - 0.42).abs() - th),
                    West => c.fill(PS_PINK, |p| rounded_box(p, Vec2::splat(0.38), 0.04).abs() - th),
                    _ => {
                        let (a, b, d) = (Vec2::new(0.0, -0.44), Vec2::new(0.47, 0.36), Vec2::new(-0.47, 0.36));
                        c.fill(PS_GREEN, |p| segment(p, a, b).min(segment(p, b, d)).min(segment(p, d, a)) - th)
                    }
                }
                (c, None)
            } else {
                let label = match glyph {
                    South => ("A", XBOX_GREEN),
                    East => ("B", XBOX_RED),
                    West => ("X", XBOX_BLUE),
                    _ => ("Y", XBOX_YELLOW),
                };
                (c, Some((label.0, label.1, 0.62)))
            }
        }
        LeftBumper | RightBumper => {
            let mut c = Canvas::new(1.7);
            c.framed(|p| rounded_box(p, Vec2::new(1.55, 0.72), 0.36));
            let text = match (glyph, ps) {
                (LeftBumper, false) => "LB",
                (RightBumper, false) => "RB",
                (LeftBumper, true) => "L1",
                _ => "R1",
            };
            (c, Some((text, white, 0.5)))
        }
        LeftTrigger | RightTrigger => {
            let mut c = Canvas::new(1.25);
            // Rounder on top, like the trigger itself.
            c.framed(|p| {
                let r = if p.y < 0.0 { 0.55 } else { 0.18 };
                rounded_box(p, Vec2::new(1.1, 0.9), r)
            });
            let text = match (glyph, ps) {
                (LeftTrigger, false) => "LT",
                (RightTrigger, false) => "RT",
                (LeftTrigger, true) => "L2",
                _ => "R2",
            };
            (c, Some((text, white, 0.5)))
        }
        LeftStick | RightStick | LeftStickClick | RightStickClick => {
            let mut c = Canvas::new(1.0);
            c.button_circle();
            let click = matches!(glyph, LeftStickClick | RightStickClick);
            if click {
                c.fill([0.25, 0.25, 0.27, 1.0], |p| p.length() - 0.66);
            }
            c.fill(DIM, |p| (p.length() - 0.66).abs() - 0.035);
            let left = matches!(glyph, LeftStick | LeftStickClick);
            let text = match (left, click, ps) {
                (true, false, _) => "L",
                (false, false, _) => "R",
                (true, true, false) => "LS",
                (false, true, false) => "RS",
                (true, true, true) => "L3",
                (false, true, true) => "R3",
            };
            (c, Some((text, white, if click { 0.44 } else { 0.55 })))
        }
        DPad | DPadUp | DPadDown | DPadLeft | DPadRight => {
            let mut c = Canvas::new(1.0);
            let cross = |p: Vec2| rounded_box(p, Vec2::new(0.92, 0.31), 0.1).min(rounded_box(p, Vec2::new(0.31, 0.92), 0.1));
            c.fill(RIM, |p| cross(p) - RIM_WIDTH * 0.8);
            c.fill(DARK, cross);
            let arm = match glyph {
                DPadUp => Some(Vec2::new(0.0, -0.56)),
                DPadDown => Some(Vec2::new(0.0, 0.56)),
                DPadLeft => Some(Vec2::new(-0.56, 0.0)),
                DPadRight => Some(Vec2::new(0.56, 0.0)),
                _ => None,
            };
            match arm {
                Some(at) => {
                    let half = if at.x == 0.0 { Vec2::new(0.22, 0.27) } else { Vec2::new(0.27, 0.22) };
                    c.fill(ICON, |p| rounded_box(p - at, half, 0.06));
                }
                None => {
                    for at in [Vec2::new(0.0, -0.56), Vec2::new(0.0, 0.56), Vec2::new(-0.56, 0.0), Vec2::new(0.56, 0.0)] {
                        c.fill(DIM, |p| (p - at).length() - 0.12);
                    }
                }
            }
            (c, None)
        }
        Select | Start if ps => {
            let mut c = Canvas::new(2.0);
            c.framed(|p| rounded_box(p, Vec2::new(1.88, 0.7), 0.7));
            (c, Some((if glyph == Select { "SHARE" } else { "OPTIONS" }, white, 0.34)))
        }
        Select => {
            // View: two overlapping windows.
            let mut c = Canvas::new(1.0);
            c.button_circle();
            c.fill(ICON, |p| rounded_box(p - Vec2::new(0.1, 0.1), Vec2::splat(0.24), 0.05).abs() - 0.045);
            c.fill(DARK, |p| rounded_box(p - Vec2::new(-0.1, -0.1), Vec2::splat(0.26), 0.05));
            c.fill(ICON, |p| rounded_box(p - Vec2::new(-0.1, -0.1), Vec2::splat(0.24), 0.05).abs() - 0.045);
            (c, None)
        }
        Start => {
            // Menu: three lines.
            let mut c = Canvas::new(1.0);
            c.button_circle();
            c.fill(ICON, |p| {
                [-0.24, 0.0, 0.24].iter().map(|&y| segment(p, Vec2::new(-0.34, y), Vec2::new(0.34, y))).fold(f32::MAX, f32::min) - 0.05
            });
            (c, None)
        }
    }
}

/// Distance from `p` to the segment `a`-`b`.
fn segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let (pa, ba) = (p - a, b - a);
    let h = (pa.dot(ba) / ba.dot(ba)).clamp(0.0, 1.0);
    (pa - ba * h).length()
}

/// Signed distance to a box of half size `half` with corner radius `r`,
/// centred on the origin.
fn rounded_box(p: Vec2, half: Vec2, r: f32) -> f32 {
    let q = p.abs() - half + Vec2::splat(r);
    q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - r
}

/// An RGBA picture drawn in a space one unit from its centre to the top
/// and bottom edges (y down), `aspect` wide per unit tall.
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(aspect: f32) -> Canvas {
        let w = (SIZE as f32 * aspect).round() as usize;
        Canvas { w, h: SIZE, px: vec![[0.0; 4]; w * SIZE] }
    }

    /// Paint `color` where `sdf` (in canvas units) is negative, antialiased
    /// over a pixel.
    fn fill(&mut self, color: [f32; 4], sdf: impl Fn(Vec2) -> f32) {
        let half = self.h as f32 * 0.5;
        let cx = self.w as f32 * 0.5;
        for y in 0..self.h {
            for x in 0..self.w {
                let p = Vec2::new((x as f32 + 0.5 - cx) / half, (y as f32 + 0.5 - half) / half);
                let cover = (0.5 - sdf(p) * half).clamp(0.0, 1.0) * color[3];
                if cover <= 0.0 {
                    continue;
                }
                let dst = &mut self.px[y * self.w + x];
                let a = cover + dst[3] * (1.0 - cover);
                for i in 0..3 {
                    dst[i] = (color[i] * cover + dst[i] * dst[3] * (1.0 - cover)) / a.max(1e-6);
                }
                dst[3] = a;
            }
        }
    }

    /// A dark shape with a light rim.
    fn framed(&mut self, sdf: impl Fn(Vec2) -> f32) {
        self.fill(RIM, |p| sdf(p) + 0.04);
        self.fill(DARK, |p| sdf(p) + 0.04 + RIM_WIDTH);
    }

    /// The round face-button base.
    fn button_circle(&mut self) {
        self.framed(|p| p.length() - 0.96);
    }

    fn image(self) -> Image {
        let data = self.px.iter().flat_map(|c| c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
        Image::new(
            Extent3d { width: self.w as u32, height: self.h as u32, depth_or_array_layers: 1 },
            TextureDimension::D2,
            data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_draws_something() {
        for kind in [PadKind::Xbox, PadKind::PlayStation] {
            for glyph in Glyph::ALL {
                let (c, _) = draw(kind, glyph);
                let solid = c.px.iter().filter(|p| p[3] > 0.5).count();
                assert!(solid > c.px.len() / 5, "{kind:?} {glyph:?} is nearly empty");
                // Corners stay clear.
                assert!(c.px[0][3] < 0.1, "{kind:?} {glyph:?} fills its corner");
            }
        }
    }

    #[test]
    fn labels_follow_the_pad() {
        assert_eq!(draw(PadKind::Xbox, Glyph::LeftBumper).1.map(|l| l.0), Some("LB"));
        assert_eq!(draw(PadKind::PlayStation, Glyph::LeftBumper).1.map(|l| l.0), Some("L1"));
        assert_eq!(draw(PadKind::PlayStation, Glyph::RightTrigger).1.map(|l| l.0), Some("R2"));
        assert_eq!(draw(PadKind::Xbox, Glyph::South).1.map(|l| l.0), Some("A"));
        // PlayStation's face buttons are pictures.
        assert!(draw(PadKind::PlayStation, Glyph::South).1.is_none());
    }
}
