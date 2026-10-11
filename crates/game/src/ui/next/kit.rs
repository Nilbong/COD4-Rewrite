//! The new UI's drawing kit: the 1920x1080 design canvas mapped onto the
//! window (or a splitscreen player's part of it), and the primitives every
//! component is drawn with (fills, gradients, lines, cut-corner panels,
//! text, icons and button prompts), all as the menus' own draw ops.

use super::super::expr::Env;
use super::super::{Frontend, Op};
use super::font::{self, Cut};
use super::theme::{self, Rgba, Type};
use bevy::prelude::*;

/// A rectangle in design units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> R {
        R { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w * 0.5
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h * 0.5
    }
    pub fn inset(&self, d: f32) -> R {
        R::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }
    pub fn pad(&self, dx: f32, dy: f32) -> R {
        R::new(self.x + dx, self.y + dy, self.w - 2.0 * dx, self.h - 2.0 * dy)
    }
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }
    /// Split off the top `h`.
    pub fn top(&self, h: f32) -> R {
        R::new(self.x, self.y, self.w, h)
    }
    pub fn below(&self, gap: f32, h: f32) -> R {
        R::new(self.x, self.bottom() + gap, self.w, h)
    }
}

/// The design canvas on the window: window pixel = `o + design * s`.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub o: Vec2,
    pub s: f32,
    /// The window (or viewport) in pixels: its corner and size.
    pub win_pos: Vec2,
    pub win: Vec2,
}

impl View {
    /// The canvas fitted inside the viewport at `pos`, `size` (pixels).
    pub fn new(pos: Vec2, size: Vec2) -> View {
        let (cw, ch) = theme::CANVAS;
        let s = (size.x / cw).min(size.y / ch);
        let o = pos + (size - Vec2::new(cw, ch) * s) * 0.5;
        View { o, s, win_pos: pos, win: size }
    }
    pub fn px(&self, x: f32, y: f32) -> Vec2 {
        self.o + Vec2::new(x, y) * self.s
    }
    /// The viewport's edges in design units (wider than the canvas on
    /// ultrawide windows).
    pub fn bleed(&self) -> R {
        let a = (self.win_pos - self.o) / self.s;
        R::new(a.x, a.y, self.win.x / self.s, self.win.y / self.s)
    }
}

/// Draw ops for the new UI.
pub struct Kit<'a> {
    pub fe: &'a Frontend,
    pub ops: &'a mut Vec<Op>,
    pub v: View,
    /// Applied to every colour (fades).
    pub alpha: f32,
    /// Added to every position (slides), design units.
    pub shift: Vec2,
    /// Seconds since the frontend started, for motion.
    pub now: f32,
}

/// The user's icon sheets (4x4) and their icons.
pub const MENU_ICONS: &str = "next:icons_menu";
pub const HUD_ICONS: &str = "next:icons_hud";
pub mod icons {
    pub const HOME: usize = 0;
    pub const CROSSHAIR: usize = 1;
    pub const RIFLE: usize = 2;
    pub const HELMET: usize = 3;
    pub const COG: usize = 4;
    pub const CRATE: usize = 5;
    pub const LOCK: usize = 6;
    pub const STAR: usize = 7;
    pub const CHECK: usize = 8;
    pub const TROPHY: usize = 9;
    pub const PEOPLE: usize = 10;
    pub const LINK: usize = 11;
    pub const PAD: usize = 12;
    pub const KEYBOARD: usize = 13;
    pub const BOT: usize = 14;
    pub const CROWN: usize = 15;
}

/// Each icon's bounds in its sheet (u0, v0, u1, v1), measured from the
/// art (`assets/ui/next/icons_*.png`).
const ICONS_MENU_BOUNDS: [[f32; 4]; 16] = [
    [0.0752, 0.0996, 0.2256, 0.2422],
    [0.2930, 0.0840, 0.4688, 0.2500],
    [0.5186, 0.1318, 0.7314, 0.2236],
    [0.7744, 0.0918, 0.9434, 0.2500],
    [0.0615, 0.3125, 0.2393, 0.4893],
    [0.2949, 0.3398, 0.4717, 0.4629],
    [0.5615, 0.3115, 0.6846, 0.4795],
    [0.7754, 0.3193, 0.9414, 0.4785],
    [0.0645, 0.5654, 0.2373, 0.6963],
    [0.3018, 0.5459, 0.4658, 0.7109],
    [0.5332, 0.5742, 0.7139, 0.7012],
    [0.7803, 0.5498, 0.9385, 0.7100],
    [0.0557, 0.7920, 0.2422, 0.9092],
    [0.2910, 0.8018, 0.4775, 0.8984],
    [0.5342, 0.7598, 0.7109, 0.9180],
    [0.7676, 0.7803, 0.9502, 0.9180],
];
const ICONS_HUD_BOUNDS: [[f32; 4]; 16] = [
    [0.0830, 0.0713, 0.2119, 0.2441],
    [0.3506, 0.0703, 0.4512, 0.2441],
    [0.5791, 0.0703, 0.6777, 0.2441],
    [0.8281, 0.0674, 0.9238, 0.2441],
    [0.0605, 0.3125, 0.2236, 0.4434],
    [0.3086, 0.3262, 0.4521, 0.4678],
    [0.5137, 0.3574, 0.7344, 0.4336],
    [0.7676, 0.3135, 0.9629, 0.4502],
    [0.0596, 0.5176, 0.2207, 0.7041],
    [0.2969, 0.5654, 0.4668, 0.6582],
    [0.5703, 0.5342, 0.6885, 0.6807],
    [0.8174, 0.5254, 0.9131, 0.6982],
    [0.0723, 0.7656, 0.2119, 0.9385],
    [0.3203, 0.7637, 0.4629, 0.9385],
    [0.5518, 0.7559, 0.7002, 0.9385],
    [0.8115, 0.7725, 0.9199, 0.9287],
];

/// Generated textures.
pub const RAMP: &str = "next:ramp";
pub const TRI: &str = "next:tri";
pub const DISC: &str = "next:disc";
pub const GLOW: &str = "next:glow";
/// Opaque white (CoD4's own `white` isn't quite).
pub const SOLID: &str = "next:solid";

impl<'a> Kit<'a> {
    pub fn new(fe: &'a Frontend, ops: &'a mut Vec<Op>, v: View) -> Kit<'a> {
        let now = fe.millis() as f32 / 1000.0;
        Kit { fe, ops, v, alpha: 1.0, shift: Vec2::ZERO, now }
    }

    fn c(&self, c: Rgba) -> [f32; 4] {
        [c[0], c[1], c[2], c[3] * self.alpha]
    }

    fn p(&self, x: f32, y: f32) -> Vec2 {
        self.v.px(x + self.shift.x, y + self.shift.y)
    }

    /// Pixel-snapped corners of a rect, so edges stay sharp.
    fn snapped(&self, r: R) -> (Vec2, Vec2) {
        let a = self.p(r.x, r.y).round();
        let b = self.p(r.right(), r.bottom()).round();
        (a, b - a)
    }

    pub fn fill(&mut self, r: R, c: Rgba) {
        if r.w <= 0.0 || r.h <= 0.0 || c[3] <= 0.0 {
            return;
        }
        let (pos, size) = self.snapped(r);
        let color = self.c(c);
        self.ops.push(Op::Image { pos, size, material: SOLID.into(), color, uv: None, rot: 0.0, layer: 1 });
    }

    /// A fill under the match's minimap (its draw layer 0).
    pub fn fill_under(&mut self, r: R, c: Rgba) {
        let (pos, size) = self.snapped(r);
        let color = self.c(c);
        self.ops.push(Op::Image { pos, size, material: SOLID.into(), color, uv: None, rot: 0.0, layer: 0 });
    }

    /// A horizontal gradient of `c` from alpha `a0` (left) to `a1` (right).
    pub fn grad_h(&mut self, r: R, c: Rgba, a0: f32, a1: f32) {
        self.grad(r, c, a0, a1, false);
    }

    /// A vertical gradient of `c` from alpha `a0` (top) to `a1` (bottom).
    pub fn grad_v(&mut self, r: R, c: Rgba, a0: f32, a1: f32) {
        self.grad(r, c, a0, a1, true);
    }

    fn grad(&mut self, r: R, c: Rgba, a0: f32, a1: f32, vertical: bool) {
        let top = a0.max(a1);
        if r.w <= 0.0 || r.h <= 0.0 || top <= 0.0 {
            return;
        }
        // The ramp's alpha is u: show the part from a0/top to a1/top.
        let (u0, u1) = (a0 / top, a1 / top);
        let (pos, size) = self.snapped(r);
        let color = self.c([c[0], c[1], c[2], c[3] * top]);
        let uv = Some(Rect::new(u0.min(u1), 0.0, u0.max(u1), 1.0));
        // Mirrored when it falls left to right.
        let flip = u0 > u1;
        if vertical {
            // The ramp turned a quarter (clockwise: left goes to the top).
            let centre = pos + size * 0.5;
            let turned = Vec2::new(size.y, size.x);
            let rot = if flip { -std::f32::consts::FRAC_PI_2 } else { std::f32::consts::FRAC_PI_2 };
            self.ops.push(Op::Image { pos: centre - turned * 0.5, size: turned, material: RAMP.into(), color, uv, rot, layer: 1 });
        } else {
            let (pos, size) = if flip { (pos, size) } else { (pos, size) };
            let rot = if flip { std::f32::consts::PI } else { 0.0 };
            self.ops.push(Op::Image { pos, size, material: RAMP.into(), color, uv, rot, layer: 1 });
        }
    }

    /// A line between two design points, `t` units thick (at least a pixel).
    pub fn line(&mut self, a: Vec2, b: Vec2, t: f32, c: Rgba) {
        let (pa, pb) = (self.p(a.x, a.y), self.p(b.x, b.y));
        let d = pb - pa;
        let size = Vec2::new(d.length(), (t * self.v.s).max(1.0));
        if d.x.abs() < 0.01 || d.y.abs() < 0.01 {
            // Straight: a snapped fill.
            let lo = pa.min(pb);
            let (pos, size) = if d.y.abs() < 0.01 {
                (Vec2::new(lo.x, (pa.y - size.y * 0.5).round()), Vec2::new(d.x.abs(), size.y.round().max(1.0)))
            } else {
                (Vec2::new((pa.x - size.y * 0.5).round(), lo.y), Vec2::new(size.y.round().max(1.0), d.y.abs()))
            };
            let color = self.c(c);
            self.ops.push(Op::Image { pos, size, material: SOLID.into(), color, uv: None, rot: 0.0, layer: 1 });
            return;
        }
        let pos = (pa + pb) * 0.5 - size * 0.5;
        let color = self.c(c);
        self.ops.push(Op::Image { pos, size, material: SOLID.into(), color, uv: None, rot: d.y.atan2(d.x), layer: 1 });
    }

    pub fn hline(&mut self, x0: f32, x1: f32, y: f32, t: f32, c: Rgba) {
        self.line(Vec2::new(x0, y), Vec2::new(x1, y), t, c);
    }

    pub fn vline(&mut self, x: f32, y0: f32, y1: f32, t: f32, c: Rgba) {
        self.line(Vec2::new(x, y0), Vec2::new(x, y1), t, c);
    }

    /// A rectangle's outline, drawn inside it.
    pub fn frame(&mut self, r: R, t: f32, c: Rgba) {
        self.fill(R::new(r.x, r.y, r.w, t), c);
        self.fill(R::new(r.x, r.bottom() - t, r.w, t), c);
        self.fill(R::new(r.x, r.y + t, t, r.h - 2.0 * t), c);
        self.fill(R::new(r.right() - t, r.y + t, t, r.h - 2.0 * t), c);
    }

    /// Corner brackets (the focus frame of cards).
    pub fn brackets(&mut self, r: R, len: f32, t: f32, c: Rgba) {
        for (x, y, sx, sy) in [(r.x, r.y, 1.0, 1.0), (r.right(), r.y, -1.0, 1.0), (r.x, r.bottom(), 1.0, -1.0), (r.right(), r.bottom(), -1.0, -1.0)] {
            let hx = if sx > 0.0 { x } else { x - len };
            let vy = if sy > 0.0 { y } else { y - len };
            self.fill(R::new(hx, if sy > 0.0 { y } else { y - t }, len, t), c);
            self.fill(R::new(if sx > 0.0 { x } else { x - t }, vy, t, len), c);
        }
    }

    /// A panel with its top-left and bottom-right corners cut by `cut`.
    pub fn cut_box(&mut self, r: R, cut: f32, c: Rgba) {
        if cut <= 0.0 {
            return self.fill(r, c);
        }
        // Middle band, then the two side bands short of the cut corners.
        self.fill(R::new(r.x + cut, r.y, r.w - 2.0 * cut, r.h), c);
        self.fill(R::new(r.x, r.y + cut, cut, r.h - cut), c);
        self.fill(R::new(r.right() - cut, r.y, cut, r.h - cut), c);
        // The corners' triangles.
        self.tri(R::new(r.x, r.y, cut, cut), false, c);
        self.tri(R::new(r.right() - cut, r.bottom() - cut, cut, cut), true, c);
    }

    /// The outline of a [`Kit::cut_box`].
    pub fn cut_frame(&mut self, r: R, cut: f32, t: f32, c: Rgba) {
        let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
        self.hline(x0 + cut, x1, y0 + t * 0.5, t, c);
        self.vline(x1 - t * 0.5, y0, y1 - cut, t, c);
        self.hline(x0, x1 - cut, y1 - t * 0.5, t, c);
        self.vline(x0 + t * 0.5, y0 + cut, y1, t, c);
        self.line(Vec2::new(x0, y0 + cut), Vec2::new(x0 + cut, y0), t, c);
        self.line(Vec2::new(x1 - cut, y1), Vec2::new(x1, y1 - cut), t, c);
    }

    /// The filled triangle in `r` against its bottom-right (`!flip`: the
    /// top-left corner cut away) or its top-left (`flip`).
    fn tri(&mut self, r: R, flip: bool, c: Rgba) {
        let (pos, size) = self.snapped(r);
        let color = self.c(c);
        let rot = if flip { std::f32::consts::PI } else { 0.0 };
        self.ops.push(Op::Image { pos, size, material: TRI.into(), color, uv: None, rot, layer: 1 });
    }

    pub fn disc(&mut self, centre: Vec2, radius: f32, c: Rgba) {
        let p = self.p(centre.x - radius, centre.y - radius);
        let size = Vec2::splat(2.0 * radius * self.v.s);
        let color = self.c(c);
        self.ops.push(Op::Pic { pos: p, size, material: DISC.into(), color });
    }

    /// A soft radial glow.
    pub fn glow(&mut self, r: R, c: Rgba) {
        let (pos, size) = (self.p(r.x, r.y), Vec2::new(r.w, r.h) * self.v.s);
        let color = self.c(c);
        self.ops.push(Op::Pic { pos, size, material: GLOW.into(), color });
    }

    /// A UI material (CoD4's or one of ours) stretched over `r`.
    pub fn pic(&mut self, r: R, material: &str, c: Rgba) {
        let (pos, size) = (self.p(r.x, r.y), Vec2::new(r.w, r.h) * self.v.s);
        let color = self.c(c);
        self.ops.push(Op::Pic { pos, size, material: material.into(), color });
    }

    /// A material's `uv` part over `r`.
    pub fn pic_uv(&mut self, r: R, material: &str, uv: Rect, c: Rgba) {
        let (pos, size) = (self.p(r.x, r.y), Vec2::new(r.w, r.h) * self.v.s);
        let color = self.c(c);
        self.ops.push(Op::Image { pos, size, material: material.into(), color, uv: Some(uv), rot: 0.0, layer: 1 });
    }

    /// A picture across `r` whose ends keep their shape: its left `l` and
    /// right `rr` source pixels are drawn at the height's scale, the middle
    /// stretched (`src`: the picture's size in pixels).
    #[allow(clippy::too_many_arguments)]
    pub fn slice3(&mut self, r: R, material: &str, src: Vec2, l: f32, rr: f32, c: Rgba) {
        let k = r.h / src.y;
        let (lw, rw) = ((l * k).min(r.w * 0.5), (rr * k).min(r.w * 0.5));
        let uv = |a: f32, b: f32| Rect::new(a / src.x, 0.0, b / src.x, 1.0);
        self.pic_uv(R::new(r.x, r.y, lw, r.h), material, uv(0.0, l), c);
        self.pic_uv(R::new(r.x + lw, r.y, (r.w - lw - rw).max(0.0), r.h), material, uv(l, src.x - rr), c);
        self.pic_uv(R::new(r.right() - rw, r.y, rw, r.h), material, uv(src.x - rr, src.x), c);
    }

    /// A picture across `r` as nine pieces: its corners (`c` source pixels
    /// square) drawn unstretched at `k` design units per source pixel, its
    /// edges stretched along their length only, its middle both ways.
    pub fn slice9(&mut self, r: R, material: &str, src: Vec2, c: f32, k: f32, tint: Rgba) {
        let d = (c * k).min(r.w * 0.5).min(r.h * 0.5);
        let xs = [r.x, r.x + d, r.right() - d, r.right()];
        let ys = [r.y, r.y + d, r.bottom() - d, r.bottom()];
        let us = [0.0, c / src.x, 1.0 - c / src.x, 1.0];
        let vs = [0.0, c / src.y, 1.0 - c / src.y, 1.0];
        for j in 0..3 {
            for i in 0..3 {
                let part = R::new(xs[i], ys[j], xs[i + 1] - xs[i], ys[j + 1] - ys[j]);
                if part.w > 0.0 && part.h > 0.0 {
                    self.pic_uv(part, material, Rect::new(us[i], vs[j], us[i + 1], vs[j + 1]), tint);
                }
            }
        }
    }

    /// Icon `index` (row by row) of a 4x4 icon sheet ([`MENU_ICONS`],
    /// [`HUD_ICONS`]), fitted and centred in `r` by its own bounds (the
    /// icons don't sit in the middle of their cells).
    pub fn icon(&mut self, sheet: &str, index: usize, r: R, c: Rgba) {
        let b = if sheet == HUD_ICONS { ICONS_HUD_BOUNDS[index % 16] } else { ICONS_MENU_BOUNDS[index % 16] };
        let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
        // (Wide icons, like the rifle, may run a little past the box.)
        let k = (r.w * 1.45 / bw).min(r.h / bh);
        let (w, h) = (bw * k, bh * k);
        self.pic_uv(R::new(r.cx() - w * 0.5, r.cy() - h * 0.5, w, h), sheet, Rect::new(b[0], b[1], b[2], b[3]), c);
    }

    /// A gun as a turnable 3D preview (its 2D `picture` meanwhile).
    pub fn gun(&mut self, r: R, key: i32, weapon: &str, camo: usize, picture: &str) {
        let (pos, size) = self.snapped(r);
        self.ops.push(Op::Gun { pos, size, key, weapon: weapon.into(), camo, picture: picture.into() });
    }

    // --- Text ---------------------------------------------------------------

    /// The atlas and scale for text `size` design units tall.
    fn font(&self, cut: Cut, size: f32) -> Option<(font::Made, f32)> {
        font::pick(cut, size * self.v.s)
    }

    fn shaped(t: &Type, s: &str) -> String {
        if t.caps { s.to_uppercase() } else { s.to_owned() }
    }

    /// Width of `s` in design units.
    pub fn measure(&self, t: Type, s: &str) -> f32 {
        let s = Self::shaped(&t, s);
        let track = t.tracking * t.size * (s.chars().count().saturating_sub(1)) as f32;
        match self.font(t.cut, t.size) {
            Some((m, k)) => super::super::draw::text_width(&self.fe.assets.fonts[m.font], &s, k) / self.v.s + track,
            None => {
                let font = self.fe.font_for(1, t.size * self.v.s);
                let k = t.size * self.v.s / self.fe.assets.fonts[font].pixel_height as f32;
                super::super::draw::text_width(&self.fe.assets.fonts[font], &s, k) / self.v.s + track
            }
        }
    }

    /// `s` with its caps centred in the line box `y..y + t.size`, from `x`
    /// (`align` 0 left, 1 centre, 2 right of `x`); returns its width.
    pub fn text(&mut self, x: f32, y: f32, t: Type, s: &str, c: Rgba, align: u8) -> f32 {
        let shaped = Self::shaped(&t, s);
        let w = self.measure(t, s);
        let x = match align {
            1 => x - w * 0.5,
            2 => x - w,
            _ => x,
        };
        let (font, k, cap) = match self.font(t.cut, t.size) {
            Some((m, k)) => (m.font, k, m.cap * k),
            None => {
                let f = self.fe.font_for(1, t.size * self.v.s);
                let k = t.size * self.v.s / self.fe.assets.fonts[f].pixel_height as f32;
                (f, k, t.size * self.v.s * 0.7)
            }
        };
        let color = self.c(c);
        let top = self.p(x, y);
        let baseline = (top.y + (t.size * self.v.s + cap) * 0.5).round();
        if t.tracking.abs() < 0.001 {
            self.ops.push(Op::Text { text: shaped, x: top.x.round(), y: baseline, font, k, color, shadow: 0.0 });
        } else {
            let track = t.tracking * t.size * self.v.s;
            let f = &self.fe.assets.fonts[font];
            let mut at = top.x;
            for ch in shaped.chars() {
                let one = ch.to_string();
                let adv = super::super::draw::text_width(f, &one, k);
                if ch != ' ' {
                    self.ops.push(Op::Text { text: one, x: at.round(), y: baseline, font, k, color, shadow: 0.0 });
                }
                at += adv + track;
            }
        }
        w
    }

    /// Text vertically centred on `cy`.
    pub fn text_mid(&mut self, x: f32, cy: f32, t: Type, s: &str, c: Rgba, align: u8) -> f32 {
        self.text(x, cy - t.size * 0.5, t, s, c, align)
    }

    /// Word-wrapped text; returns the height used.
    pub fn para(&mut self, x: f32, y: f32, w: f32, t: Type, s: &str, c: Rgba, leading: f32) -> f32 {
        let mut lines: Vec<String> = Vec::new();
        for word in s.split(' ') {
            let next = match lines.last() {
                Some(l) => format!("{l} {word}"),
                None => word.to_owned(),
            };
            if lines.is_empty() {
                lines.push(next);
            } else if self.measure(t, &next) > w {
                lines.push(word.to_owned());
            } else {
                *lines.last_mut().unwrap() = next;
            }
        }
        let step = t.size * leading;
        for (i, l) in lines.iter().enumerate() {
            self.text(x, y + i as f32 * step, t, l, c, 0);
        }
        lines.len() as f32 * step
    }

    // --- Icons (drawn, crisp at any size) -------------------------------------

    /// A chevron pointing right (`dir` 0), down (1), left (2) or up (3).
    pub fn chevron(&mut self, centre: Vec2, size: f32, dir: u8, t: f32, c: Rgba) {
        let h = size * 0.5;
        let (a, m, b) = (Vec2::new(-h * 0.5, -h), Vec2::new(h * 0.5, 0.0), Vec2::new(-h * 0.5, h));
        let turn = |p: Vec2| match dir {
            1 => Vec2::new(-p.y, p.x),
            2 => Vec2::new(-p.x, p.y),
            3 => Vec2::new(p.y, -p.x),
            _ => p,
        };
        self.line(centre + turn(a), centre + turn(m), t, c);
        self.line(centre + turn(m), centre + turn(b), t, c);
    }

    /// A padlock.
    /// A padlock (the menu icons' own).
    pub fn lock(&mut self, centre: Vec2, size: f32, c: Rgba) {
        self.icon(MENU_ICONS, icons::LOCK, R::new(centre.x - size * 0.6, centre.y - size * 0.6, size * 1.2, size * 1.2), c);
    }

    /// A tick (the menu icons' own); `_t` is kept for callers.
    pub fn check(&mut self, centre: Vec2, size: f32, _t: f32, c: Rgba) {
        self.icon(MENU_ICONS, icons::CHECK, R::new(centre.x - size * 0.6, centre.y - size * 0.6, size * 1.2, size * 1.2), c);
    }

    pub fn plus(&mut self, centre: Vec2, size: f32, t: f32, c: Rgba) {
        let h = size * 0.5;
        self.hline(centre.x - h, centre.x + h, centre.y, t, c);
        self.vline(centre.x, centre.y - h, centre.y + h, t, c);
    }

    /// A ring of `segments` straight pieces.
    pub fn ring(&mut self, centre: Vec2, radius: f32, t: f32, segments: usize, c: Rgba) {
        let n = segments.max(6);
        for i in 0..n {
            let a0 = i as f32 / n as f32 * std::f32::consts::TAU;
            let a1 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;
            self.line(centre + Vec2::from_angle(a0) * radius, centre + Vec2::from_angle(a1) * radius, t, c);
        }
    }

    /// A settings cog.
    pub fn cog(&mut self, centre: Vec2, size: f32, c: Rgba) {
        let r = size * 0.32;
        self.ring(centre, r, size * 0.09, 16, c);
        for i in 0..8 {
            let d = Vec2::from_angle(i as f32 / 8.0 * std::f32::consts::TAU);
            self.line(centre + d * r, centre + d * size * 0.48, size * 0.13, c);
        }
    }

    // --- Button prompts -------------------------------------------------------

    /// A key or pad button's glyph, `h` tall, its left edge at `x`, centred
    /// on `cy`; returns its width.
    pub fn glyph(&mut self, x: f32, cy: f32, h: f32, button: Button) -> f32 {
        let pad = self.fe.pad;
        let ps = pad && super::pad_is_playstation();
        let ink = theme::INK;
        if !pad {
            // A keycap.
            let key = button.key();
            let t = Type { cut: Cut::Semi, size: h * 0.5, tracking: 0.04, caps: true };
            let w = (self.measure(t, key) + h * 0.6).max(h);
            let r = R::new(x, cy - h * 0.5, w, h);
            self.fill(r, theme::hex(0x000000, 0.35));
            self.frame(r, 1.5, theme::LINE_STRONG);
            self.text_mid(r.cx(), r.cy(), t, key, ink, 1);
            return w;
        }
        // The user's Xbox / PlayStation button art (`assets/ui/next/buttons`).
        let (sprite, size) = button.pad_sprite(ps);
        let w = h * size.0 / size.1;
        self.pic(R::new(x, cy - h * 0.5, w, h), &format!("next:buttons2/{sprite}"), [1.0; 4]);
        w
    }

    /// A prompt: glyph then label; returns its width.
    pub fn prompt(&mut self, x: f32, cy: f32, button: Button, what: &str) -> f32 {
        let gw = self.glyph(x, cy, 34.0, button);
        let tw = self.text_mid(x + gw + 10.0, cy, theme::Type { size: 20.0, ..theme::ROW_TEXT }, what, theme::INK, 0);
        gw + 10.0 + tw
    }

    /// Prompts right-aligned to `right`.
    pub fn prompts_right(&mut self, right: f32, cy: f32, list: &[(Button, &str)]) {
        let widths: Vec<f32> = list
            .iter()
            .map(|(b, s)| {
                let g = if self.fe.pad { 34.0 * b.pad_sprite(super::pad_is_playstation()).1.0 / 128.0 } else { self.measure(Type { cut: Cut::Semi, size: 17.0, tracking: 0.04, caps: true }, b.key()) + 20.0 };
                g.max(34.0) + 10.0 + self.measure(Type { size: 20.0, ..theme::ROW_TEXT }, s)
            })
            .collect();
        let total: f32 = widths.iter().sum::<f32>() + 36.0 * (list.len().saturating_sub(1)) as f32;
        let mut x = right - total;
        for (i, (b, s)) in list.iter().enumerate() {
            let w = self.prompt(x, cy, *b, s);
            x += w.max(widths[i]) + 36.0;
        }
    }
}

/// What a prompt asks for (shown as the device in use's button).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Confirm,
    Back,
    /// X / Square / a context key.
    Alt,
    /// Y / Triangle.
    Alt2,
    PrevTab,
    NextTab,
    PrevSub,
    NextSub,
    Menu,
    View,
}

impl Button {
    pub fn key(self) -> &'static str {
        match self {
            Button::Confirm => "Enter",
            Button::Back => "Esc",
            Button::Alt => "F",
            Button::Alt2 => "R",
            Button::PrevTab => "Q",
            Button::NextTab => "E",
            Button::PrevSub => "Z",
            Button::NextSub => "C",
            Button::Menu => "Tab",
            Button::View => "Space",
        }
    }

    /// The pad button's sprite and its size (pixels).
    fn pad_sprite(self, ps: bool) -> (&'static str, (f32, f32)) {
        match (self, ps) {
            (Button::Confirm, false) => ("xbox_a", (128.0, 128.0)),
            (Button::Back, false) => ("xbox_b", (127.0, 128.0)),
            (Button::Alt, false) => ("xbox_x", (127.0, 128.0)),
            (Button::Alt2, false) => ("xbox_y", (128.0, 128.0)),
            (Button::PrevTab, false) => ("xbox_lb", (245.0, 128.0)),
            (Button::NextTab, false) => ("xbox_rb", (246.0, 128.0)),
            (Button::PrevSub, false) => ("xbox_lt", (94.0, 128.0)),
            (Button::NextSub, false) => ("xbox_rt", (93.0, 128.0)),
            (Button::Menu, false) => ("xbox_menu", (126.0, 128.0)),
            (Button::View, false) => ("xbox_view", (128.0, 128.0)),
            (Button::Confirm, true) => ("playstation_cross", (130.0, 128.0)),
            (Button::Back, true) => ("playstation_circle", (129.0, 128.0)),
            (Button::Alt, true) => ("playstation_square", (129.0, 128.0)),
            (Button::Alt2, true) => ("playstation_triangle", (129.0, 128.0)),
            (Button::PrevTab, true) => ("playstation_l1", (231.0, 128.0)),
            (Button::NextTab, true) => ("playstation_r1", (232.0, 128.0)),
            (Button::PrevSub, true) => ("playstation_l2", (76.0, 128.0)),
            (Button::NextSub, true) => ("playstation_r2", (76.0, 128.0)),
            (Button::Menu, true) => ("playstation_options", (101.0, 128.0)),
            (Button::View, true) => ("playstation_create", (208.0, 128.0)),
        }
    }

    #[allow(dead_code)]
    fn pad_label(self, ps: bool) -> &'static str {
        match (self, ps) {
            (Button::PrevTab, false) => "LB",
            (Button::NextTab, false) => "RB",
            (Button::PrevSub, false) => "LT",
            (Button::NextSub, false) => "RT",
            (Button::PrevTab, true) => "L1",
            (Button::NextTab, true) => "R1",
            (Button::PrevSub, true) => "L2",
            (Button::NextSub, true) => "R2",
            (Button::Menu, false) => "MENU",
            (Button::Menu, true) => "OPTIONS",
            (Button::View, false) => "VIEW",
            (Button::View, true) => "PAD",
            _ => "",
        }
    }
}

/// The generated textures: an alpha ramp, a corner triangle, a disc and a
/// glow.
pub(super) fn textures(images: &mut Assets<Image>) -> Vec<(&'static str, super::super::assets::UiImage)> {
    let make = |w: u32, h: u32, f: &dyn Fn(f32, f32) -> f32, images: &mut Assets<Image>| {
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let a = f((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32).clamp(0.0, 1.0);
                px.extend_from_slice(&[255, 255, 255, (a * 255.0).round() as u8]);
            }
        }
        let mut image = Image::new(
            bevy::render::render_resource::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            bevy::render::render_resource::TextureDimension::D2,
            px,
            bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
            bevy::asset::RenderAssetUsages::RENDER_WORLD,
        );
        image.sampler = bevy::image::ImageSampler::linear();
        super::super::assets::UiImage { handle: images.add(image), size: Vec2::new(w as f32, h as f32) }
    };
    let n = 128.0;
    vec![
        // Perceptually even: blending is in linear light.
        (RAMP, make(256, 4, &|u, _| u.powf(2.2), images)),
        (SOLID, make(4, 4, &|_, _| 1.0, images)),
        // Opaque below the diagonal from bottom-left to top-right, a pixel
        // of soft edge.
        (TRI, make(128, 128, &|u, v| (u + v - 1.0) * n * 0.5 + 0.5, images)),
        (DISC, make(128, 128, &|u, v| (0.5 - Vec2::new(u - 0.5, v - 0.5).length()) * n + 0.5, images)),
        (GLOW, make(128, 128, &|u, v| (-Vec2::new(u - 0.5, v - 0.5).length_squared() * 14.0).exp() - 0.03, images)),
    ]
}
