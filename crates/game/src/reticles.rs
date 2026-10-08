//! Custom red dot reticles: a shape, a colour and a size, picked per class
//! weapon in Create a Class (`ui/reticle_menu.rs`) and drawn by the red dot
//! shader (`ui/reflex.wgsl`) on the sight line, procedurally so they stay
//! crisp, or CoD4's own dot ("Classic") recoloured.
//!
//! A gun's reticle travels with its camo: camo numbers fit in 16 bits, and
//! the reticle's code rides above them ([`with_camo`]), so every place a
//! gun is built (first and third person, dropped guns, the killcam, the
//! menus' previews) gets it without more plumbing. [`split`] takes it off
//! again where the gun model is put together.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

pub const SHAPES: [&str; 7] = ["Classic", "Dot", "Cross", "Circle Dot", "Chevron", "Holo", "T"];
pub const COLOURS: [(&str, [f32; 3]); 8] = [
    ("Red", [1.0, 0.12, 0.08]),
    ("Green", [0.15, 1.0, 0.2]),
    ("Blue", [0.2, 0.45, 1.0]),
    ("Yellow", [1.0, 0.9, 0.12]),
    ("White", [1.0, 1.0, 1.0]),
    ("Cyan", [0.1, 0.95, 1.0]),
    ("Magenta", [1.0, 0.15, 0.9]),
    ("Orange", [1.0, 0.5, 0.08]),
];
pub const SIZES: [(&str, f32); 3] = [("Small", 0.75), ("Medium", 1.0), ("Large", 1.35)];

/// A reticle; the default is CoD4's red dot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Reticle {
    pub shape: u8,
    pub colour: u8,
    /// Index into [`SIZES`] less one (so the default, 0, is medium).
    pub size: u8,
}

impl Reticle {
    /// Its code: shape, colour and size in four bits each (0 is the default).
    pub fn code(self) -> u16 {
        (self.shape as u16 & 15) | (self.colour as u16 & 15) << 4 | (self.size as u16 & 15) << 8
    }

    pub fn from_code(code: u16) -> Reticle {
        Reticle {
            shape: ((code & 15) as u8).min(SHAPES.len() as u8 - 1),
            colour: ((code >> 4 & 15) as u8).min(COLOURS.len() as u8 - 1),
            size: ((code >> 8 & 15) as u8).min(SIZES.len() as u8 - 1),
        }
    }

    /// The size's scale (`size` 0 is medium, 1 large, 2 small).
    pub fn scale(self) -> f32 {
        SIZES[match self.size {
            1 => 2,
            2 => 0,
            _ => 1,
        }]
        .1
    }

    pub fn colour(self) -> [f32; 3] {
        COLOURS[self.colour as usize % COLOURS.len()].1
    }

    /// The shader's settings: x shape, y size; and the colour.
    pub fn uniforms(self) -> (Vec4, Vec4) {
        let c = self.colour();
        (Vec4::new(self.shape as f32, self.scale(), 0.0, 0.0), Vec4::new(c[0], c[1], c[2], 0.0))
    }
}

/// A gun's camo number with its reticle riding above it.
pub fn with_camo(camo: usize, reticle: Reticle) -> usize {
    (camo & 0xFFFF) | (reticle.code() as usize) << 16
}

/// A camo number as built: (camo, reticle).
pub fn split(camo: usize) -> (usize, Reticle) {
    (camo & 0xFFFF, Reticle::from_code((camo >> 16) as u16))
}

/// How much of a procedural shape covers point `p` (the dot texture's
/// span, -1..1 from the sight line), `px` its size in pixels for
/// antialiasing. The shader (`reflex.wgsl`) draws the same shapes.
pub fn coverage(shape: u8, p: Vec2, px: f32) -> f32 {
    let ring = |r: f32, w: f32| 1.0 - ((p.length() - r).abs() - w).max(0.0) / px;
    let disc = |r: f32| 1.0 - (p.length() - r).max(0.0) / px;
    // A bar from a to b, half-width w.
    let bar = |a: Vec2, b: Vec2, w: f32| {
        let ab = b - a;
        let t = ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
        1.0 - ((p - (a + ab * t)).length() - w).max(0.0) / px
    };
    let v = match shape {
        1 => disc(0.09),
        2 => bar(Vec2::new(-0.45, 0.0), Vec2::new(-0.1, 0.0), 0.035)
            .max(bar(Vec2::new(0.1, 0.0), Vec2::new(0.45, 0.0), 0.035))
            .max(bar(Vec2::new(0.0, -0.45), Vec2::new(0.0, -0.1), 0.035))
            .max(bar(Vec2::new(0.0, 0.1), Vec2::new(0.0, 0.45), 0.035))
            .max(disc(0.035)),
        3 => ring(0.55, 0.03).max(disc(0.07)),
        4 => bar(Vec2::new(-0.3, -0.25), Vec2::ZERO, 0.035).max(bar(Vec2::new(0.3, -0.25), Vec2::ZERO, 0.035)),
        5 => ring(0.6, 0.03)
            .max(bar(Vec2::new(-0.85, 0.0), Vec2::new(-0.6, 0.0), 0.03))
            .max(bar(Vec2::new(0.6, 0.0), Vec2::new(0.85, 0.0), 0.03))
            .max(bar(Vec2::new(0.0, -0.85), Vec2::new(0.0, -0.6), 0.03))
            .max(disc(0.06)),
        6 => bar(Vec2::new(-0.4, 0.0), Vec2::new(0.4, 0.0), 0.035).max(bar(Vec2::new(0.0, 0.0), Vec2::new(0.0, -0.45), 0.035)),
        _ => disc(0.09),
    };
    v.clamp(0.0, 1.0)
}

/// A menu picture of `reticle` lit on dark glass (`SIZE`² sRGB).
pub fn preview(reticle: Reticle, images: &mut Assets<Image>) -> Handle<Image> {
    const SIZE: u32 = 128;
    let c = reticle.colour();
    let mut px = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            // y up; the picture spans the dot texture's span, a little more.
            let p = Vec2::new(x as f32 + 0.5, (SIZE - y) as f32 - 0.5) / SIZE as f32 * 2.4 - 1.2;
            let g = if reticle.shape == 0 {
                // CoD4's dot: a soft disc.
                (1.0 - p.length() / (0.12 * reticle.scale())).clamp(0.0, 1.0).powf(0.6)
            } else {
                coverage(reticle.shape, p / reticle.scale(), 2.4 / SIZE as f32 / reticle.scale())
            };
            // A white-hot core in its colour's glow, over tinted dark glass.
            let glass = [0.05, 0.06, 0.07];
            let rgb: [f32; 3] = std::array::from_fn(|i| glass[i] * (1.0 - g) + (c[i] + (g - 0.7).max(0.0) * 1.5).min(1.0) * g);
            px.extend(rgb.map(|v| (v.powf(1.0 / 2.2) * 255.0) as u8));
            px.push(255);
        }
    }
    images.add(Image::new(
        Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        TextureDimension::D2,
        px,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_ride_on_camos() {
        for shape in 0..SHAPES.len() as u8 {
            for colour in 0..COLOURS.len() as u8 {
                for size in 0..SIZES.len() as u8 {
                    let r = Reticle { shape, colour, size };
                    assert_eq!(Reticle::from_code(r.code()), r);
                    assert_eq!(split(with_camo(305, r)), (305, r));
                }
            }
        }
        assert_eq!(Reticle::default().code(), 0);
        assert_eq!(with_camo(6, Reticle::default()), 6, "the default leaves camo numbers as they were");
    }

    #[test]
    fn shapes_cover_their_centre_or_ring() {
        for shape in [1, 2, 3, 5] {
            assert!(coverage(shape, Vec2::ZERO, 0.01) > 0.99, "shape {shape} has a centre");
        }
        assert!(coverage(3, Vec2::new(0.55, 0.0), 0.01) > 0.99);
        assert!(coverage(4, Vec2::new(0.0, 0.5), 0.01) < 0.01);
    }
}
