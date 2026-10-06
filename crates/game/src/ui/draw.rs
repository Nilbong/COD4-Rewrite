//! Drawing menus: placement of the 640x480 virtual screen, CoD font text,
//! and an immediate-mode list of textured quads shown with pooled sprites.

use super::assets::UiImage;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use iw3::menu::{Font, Rect as VRect};

/// `ScreenPlacement`: maps virtual coordinates to window pixels. The 4:3
/// virtual screen is scaled to the window height and centred; the
/// `*_ALIGN_*` modes anchor to its edges, the window's edges or its centre.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub w: f32,
    pub h: f32,
    pub scale: f32,
    sub_left: f32,
}

impl Placement {
    pub fn new(w: f32, h: f32) -> Placement {
        let scale = h / 480.0;
        Placement { w, h, scale, sub_left: (w - 640.0 * scale) * 0.5 }
    }

    fn x(&self, x: f32, align: u8) -> f32 {
        match align {
            1 => x * self.scale,
            2 | 7 => self.w * 0.5 + x * self.scale,
            3 => self.w + x * self.scale,
            4 | 6 => x * self.w / 640.0,
            5 => x,
            _ => self.sub_left + x * self.scale,
        }
    }

    fn y(&self, y: f32, align: u8) -> f32 {
        match align {
            2 | 7 => self.h * 0.5 + y * self.scale,
            3 => self.h + y * self.scale,
            4 | 6 => y * self.h / 480.0,
            5 => y,
            _ => y * self.scale,
        }
    }

    pub fn sx(&self, align: u8) -> f32 {
        match align {
            4 | 6 => self.w / 640.0,
            5 => 1.0,
            _ => self.scale,
        }
    }

    pub fn sy(&self, align: u8) -> f32 {
        match align {
            4 | 6 => self.h / 480.0,
            5 => 1.0,
            _ => self.scale,
        }
    }

    /// Window-pixel top-left and size of a virtual rectangle.
    pub fn rect(&self, r: &VRect) -> (Vec2, Vec2) {
        (
            Vec2::new(self.x(r.x, r.horz_align), self.y(r.y, r.vert_align)),
            Vec2::new(r.w * self.sx(r.horz_align), r.h * self.sy(r.vert_align)),
        )
    }
}

#[derive(Clone)]
pub struct Quad {
    /// Window pixels, top-left origin.
    pub pos: Vec2,
    pub size: Vec2,
    pub image: Handle<Image>,
    /// Source rectangle in image pixels (glyphs), or the whole image.
    pub uv: Option<Rect>,
    pub color: Color,
    /// Clockwise turn about the centre, radians.
    pub rot: f32,
    /// In a match: 0 draws under the minimap's map, 1 over it.
    pub layer: u8,
    /// Mirrored across or down.
    pub flip: BVec2,
}

impl Quad {
    /// A negative width or height draws the image mirrored in the same place
    /// (as `UI_DrawHandlePic` does).
    pub fn new(pos: Vec2, size: Vec2, image: Handle<Image>, uv: Option<Rect>, color: Color) -> Quad {
        let flip = BVec2::new(size.x < 0.0, size.y < 0.0);
        Quad { pos, size: size.abs(), image, uv, color, rot: 0.0, layer: 1, flip }
    }
}

#[derive(Resource, Default)]
pub struct DrawList(pub Vec<Quad>);

pub fn color(c: [f32; 4]) -> Color {
    Color::srgba(c[0], c[1], c[2], c[3])
}

/// The `^n` colour codes.
fn code_color(c: char) -> Option<[f32; 3]> {
    Some(match c {
        '0' => [0.0, 0.0, 0.0],
        '1' => [1.0, 0.36, 0.36],
        '2' => [0.0, 1.0, 0.0],
        '3' => [1.0, 1.0, 0.0],
        '4' => [0.0, 0.0, 1.0],
        '5' => [0.0, 1.0, 1.0],
        '6' => [1.0, 0.36, 1.0],
        '7' => [1.0, 1.0, 1.0],
        '8' | '9' => [0.5, 0.5, 0.5],
        _ => return None,
    })
}

fn advance(font: &Font, c: char, k: f32) -> f32 {
    font.glyph(c).map_or(font.pixel_height as f32 * 0.3, |g| g.dx as f32) * k
}

/// Width in window pixels of a line drawn at `k` pixels per font pixel.
pub fn text_width(font: &Font, text: &str, k: f32) -> f32 {
    let mut w = 0.0;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' && chars.peek().is_some_and(|n| code_color(*n).is_some()) {
            chars.next();
            continue;
        }
        w += advance(font, c, k);
    }
    w
}

/// Greedy word wrap to `max` pixels.
pub fn wrap(font: &Font, text: &str, k: f32, max: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
            if !line.is_empty() && text_width(font, &candidate, k) > max {
                lines.push(std::mem::replace(&mut line, word.to_owned()));
            } else {
                line = candidate;
            }
        }
        lines.push(line);
    }
    lines
}

/// Draw one line of text with its baseline at `y`; `shadow` is the drop
/// shadow offset in pixels (0 for none).
#[allow(clippy::too_many_arguments)]
pub fn draw_text(out: &mut Vec<Quad>, font: &Font, image: &UiImage, text: &str, x: f32, y: f32, k: f32, rgba: [f32; 4], shadow: f32) {
    if shadow > 0.0 {
        text_pass(out, font, image, text, x + shadow, y + shadow, k, [0.0, 0.0, 0.0, rgba[3]], false);
    }
    text_pass(out, font, image, text, x, y, k, rgba, true);
}

#[allow(clippy::too_many_arguments)]
fn text_pass(out: &mut Vec<Quad>, font: &Font, image: &UiImage, text: &str, mut x: f32, y: f32, k: f32, rgba: [f32; 4], codes: bool) {
    let mut c4 = rgba;
    let size = image.size;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' {
            if let Some(rgb) = chars.peek().and_then(|n| code_color(*n)) {
                chars.next();
                if codes {
                    c4 = [rgb[0], rgb[1], rgb[2], rgba[3]];
                }
                continue;
            }
        }
        let Some(g) = font.glyph(c) else {
            x += advance(font, c, k);
            continue;
        };
        if g.pixel_width > 0 && g.pixel_height > 0 {
            out.push(Quad::new(
                Vec2::new(x + g.x0 as f32 * k, y + g.y0 as f32 * k),
                Vec2::new(g.pixel_width as f32, g.pixel_height as f32) * k,
                image.handle.clone(),
                Some(Rect::new(g.s0 * size.x, g.t0 * size.y, g.s1 * size.x, g.t1 * size.y)),
                color(c4),
            ));
        }
        x += g.dx as f32 * k;
    }
}

#[derive(Component)]
pub struct UiSprite;

#[derive(Component)]
pub struct UiImageNode;

/// UI image nodes for the in-game menus, under one root drawn over the HUD.
#[derive(Resource, Default)]
pub struct NodePool {
    /// Per layer: its root and nodes.
    roots: [Option<Entity>; 2],
    nodes: [Vec<Entity>; 2],
}

/// Forget the match's nodes (despawned with it).
pub fn reset_nodes(mut pool: ResMut<NodePool>, mut list: ResMut<DrawList>) {
    *pool = NodePool::default();
    list.0.clear();
}

/// UI stacking: layer 0, the minimap's map ([`MINIMAP_Z`]), layer 1.
const LAYER_Z: [i32; 2] = [1000, 1002];
pub const MINIMAP_Z: i32 = 1001;

/// Show the draw list with one UI image node per quad, in order. During a
/// match the menus go through Bevy's UI (like the HUD) rather than a 2D
/// camera, so they sit on the finished frame.
pub fn sync_nodes(
    mut commands: Commands,
    list: Res<DrawList>,
    mut pool: ResMut<NodePool>,
    mut nodes: Query<(&mut Node, &mut ImageNode, &mut UiTransform, &mut Visibility), With<UiImageNode>>,
) {
    for layer in 0..2 {
        let quads: Vec<&Quad> =
            list.0.iter().filter(|q| q.layer as usize == layer && q.size.x > 0.0 && q.size.y > 0.0).collect();
        if quads.is_empty() && pool.nodes[layer].is_empty() {
            continue;
        }
        let root = *pool.roots[layer].get_or_insert_with(|| {
            commands
                .spawn((
                    Name::new("in-game ui"),
                    Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() },
                    GlobalZIndex(LAYER_Z[layer]),
                ))
                .id()
        });
        for (i, q) in quads.iter().enumerate() {
            let node = Node {
                position_type: PositionType::Absolute,
                left: px(q.pos.x),
                top: px(q.pos.y),
                width: px(q.size.x),
                height: px(q.size.y),
                ..default()
            };
            let image = ImageNode {
                image: q.image.clone(),
                color: q.color,
                rect: q.uv,
                image_mode: NodeImageMode::Stretch,
                flip_x: q.flip.x,
                flip_y: q.flip.y,
                ..default()
            };
            let turn = UiTransform::from_rotation(Rot2::radians(q.rot));
            match pool.nodes[layer].get(i).and_then(|&e| nodes.get_mut(e).ok()) {
                Some((mut n, mut im, mut t, mut v)) => {
                    n.set_if_neq(node);
                    let same = im.image == image.image
                        && im.color == image.color
                        && im.rect == image.rect
                        && im.flip_x == image.flip_x
                        && im.flip_y == image.flip_y;
                    if !same {
                        *im = image;
                    }
                    t.set_if_neq(turn);
                    v.set_if_neq(Visibility::Inherited);
                }
                None if i >= pool.nodes[layer].len() => {
                    let e = commands.spawn((UiImageNode, node, image, turn, Visibility::Inherited, ChildOf(root))).id();
                    pool.nodes[layer].push(e);
                }
                // Spawned last frame and not queryable yet.
                None => {}
            }
        }
        for &e in &pool.nodes[layer][quads.len().min(pool.nodes[layer].len())..] {
            if let Ok((_, _, _, mut v)) = nodes.get_mut(e) {
                v.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

#[derive(Resource, Default)]
pub struct SpritePool(pub Vec<Entity>);

/// Show the draw list with one sprite per quad, in order.
pub fn sync_sprites(
    mut commands: Commands,
    list: Res<DrawList>,
    mut pool: ResMut<SpritePool>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut sprites: Query<(&mut Sprite, &mut Transform, &mut Visibility), With<UiSprite>>,
) {
    let (w, h) = (window.width(), window.height());
    let quads = list.0.iter().filter(|q| q.size.x > 0.0 && q.size.y > 0.0);
    let mut used = 0;
    for (i, q) in quads.enumerate() {
        let tf = Transform::from_xyz(
            q.pos.x + q.size.x * 0.5 - w * 0.5,
            h * 0.5 - (q.pos.y + q.size.y * 0.5),
            -900.0 + i as f32 * 0.01,
        )
        .with_rotation(Quat::from_rotation_z(-q.rot));
        let sprite = Sprite {
            image: q.image.clone(),
            color: q.color,
            custom_size: Some(q.size),
            rect: q.uv,
            flip_x: q.flip.x,
            flip_y: q.flip.y,
            ..default()
        };
        match pool.0.get(i).and_then(|&e| sprites.get_mut(e).ok()) {
            Some((mut s, mut t, mut v)) => {
                *s = sprite;
                *t = tf;
                *v = Visibility::Inherited;
            }
            None if i >= pool.0.len() => {
                let e = commands.spawn((UiSprite, sprite, tf, Visibility::Inherited)).id();
                pool.0.push(e);
            }
            // Spawned last frame and not queryable yet.
            None => {}
        }
        used = i + 1;
    }
    for &e in &pool.0[used.min(pool.0.len())..] {
        if let Ok((_, _, mut v)) = sprites.get_mut(e) {
            *v = Visibility::Hidden;
        }
    }
}
