//! The reticle screen's preview: the view down a red dot sight. A map seen
//! through the sight's glass, the sight's housing around its window, and the
//! reticle glowing on the lens, as it looks aiming in a match.

use super::kit::{Kit, R};
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// The material of reticle `code`'s glow on clear glass.
pub fn material(code: u16) -> String {
    format!("next:reticle_{code}")
}

/// That material's picture: the reticle's light alone, its alpha the light
/// (`reticles::preview` draws the same over dark glass).
pub(in crate::ui) fn image(key: &str, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let code = key.strip_prefix("next:reticle_")?.parse::<u16>().ok()?;
    let reticle = crate::reticles::Reticle::from_code(code);
    const SIZE: u32 = 256;
    let c = reticle.colour();
    let mut px = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = Vec2::new(x as f32 + 0.5, (SIZE - y) as f32 - 0.5) / SIZE as f32 * 2.4 - 1.2;
            let g = if reticle.shape == 0 {
                (1.0 - p.length() / (0.12 * reticle.scale())).clamp(0.0, 1.0).powf(0.6)
            } else {
                crate::reticles::coverage(reticle.shape, p / reticle.scale(), 2.4 / SIZE as f32 / reticle.scale())
            };
            // Its colour, a white-hot core where strongest.
            let rgb: [f32; 3] = std::array::from_fn(|i| (c[i] + (g - 0.7).max(0.0) * 1.5).min(1.0));
            px.extend(rgb.map(|v| (v.powf(1.0 / 2.2) * 255.0) as u8));
            px.push((g.clamp(0.0, 1.0) * 255.0) as u8);
        }
    }
    let mut image = Image::new(
        Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 1 },
        TextureDimension::D2,
        px,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::linear();
    Some(images.add(image))
}

/// The sight in `area` with reticle `code` on its lens: dark glass with a
/// faint sheen and the sight's ring round it, drawn here (no picture of
/// CoD4's own sight is shipped).
pub fn draw(k: &mut Kit, area: R, code: u16) {
    let scale = area.h / 900.0 * 1.6;
    k.fill(area, [0.035, 0.04, 0.045, 1.0]);
    k.grad_v(area, [0.25, 0.3, 0.35, 1.0], 0.12, 0.0);
    let c = Vec2::new(area.cx(), area.cy());
    let radius = area.h.min(area.w) * 0.42;
    k.ring(c, radius, area.h * 0.035, 72, [0.0, 0.0, 0.0, 0.85]);
    k.ring(c, radius - area.h * 0.02, area.h * 0.004, 72, [0.5, 0.55, 0.6, 0.35]);
    // The reticle as big as in the match (measured: the medium circle is
    // 50 pixels across in a 2560-wide window, which the screenshot is).
    let s = 134.0 * scale;
    let c = Vec2::new(area.cx(), area.cy());
    let m = material(code);
    k.pic(R::new(c.x - s * 0.75, c.y - s * 0.75, s * 1.5, s * 1.5), &m, [1.0, 1.0, 1.0, 0.35]);
    k.pic(R::new(c.x - s * 0.5, c.y - s * 0.5, s, s), &m, [1.0; 4]);
    k.pic(R::new(c.x - s * 0.5, c.y - s * 0.5, s, s), &m, [1.0; 4]);
}
