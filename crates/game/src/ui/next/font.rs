//! The new UI's type: Windows' Bahnschrift (a DIN-style variable sans on
//! every Windows 10 and 11 PC, read from the system, never shipped) in three
//! cuts, rasterised on demand at the exact pixel size each piece of text is
//! drawn at, so text is never scaled and stays crisp at any window size.
//! Each (cut, size) becomes a CoD4-style glyph atlas in the menus' font list,
//! so the menus' own text drawing takes it as is. Until a size is ready (it is
//! made before the next frame) the nearest ready one stands in, scaled.

use super::super::Frontend;
use super::super::assets::UiImage;
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// A cut of the face.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cut {
    /// Body text, values.
    Regular,
    /// Labels, buttons, headers.
    Semi,
    /// Titles and big numbers: bold, condensed.
    Display,
}

impl Cut {
    /// (weight, width) on the face's axes.
    fn axes(self) -> (f32, f32) {
        match self {
            Cut::Regular => (400.0, 100.0),
            Cut::Semi => (600.0, 100.0),
            Cut::Display => (700.0, 75.0),
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Cut::Regular => "r",
            Cut::Semi => "s",
            Cut::Display => "d",
        }
    }
}

/// A ready atlas: its index in the menus' fonts, and its ascent and cap
/// height in its own pixels.
#[derive(Clone, Copy, Debug)]
pub struct Made {
    pub font: usize,
    pub px: u16,
    pub ascent: f32,
    pub cap: f32,
}

#[derive(Default)]
struct Registry {
    made: HashMap<(Cut, u16), Made>,
    wanted: Vec<(Cut, u16)>,
    /// No face on this PC: CoD4's fonts instead.
    missing: bool,
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(Default::default)
}

fn face() -> Option<&'static [u8]> {
    static BYTES: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    BYTES
        .get_or_init(|| {
            let windows = std::env::var("WINDIR").unwrap_or_else(|_| String::from("C:/Windows"));
            let path = std::path::Path::new(&windows).join("Fonts").join("bahnschrift.ttf");
            std::fs::read(&path).inspect_err(|_| info!("new ui: no {} (CoD4's fonts instead)", path.display())).ok()
        })
        .as_deref()
}

/// The face's bytes (Windows' Bahnschrift), for text drawn by Bevy's own
/// text (the in-match hints); `None` where the PC lacks it.
pub fn face_bytes() -> Option<&'static [u8]> {
    face()
}

/// Sizes are kept from 8 to 200 pixels.
fn clamp_px(px: f32) -> u16 {
    px.round().clamp(8.0, 200.0) as u16
}

/// The atlas to draw `cut` at `px` pixels tall (em), and the scale to draw
/// it at; asks for the exact size if it isn't made yet. `None` without the
/// face.
pub fn pick(cut: Cut, px: f32) -> Option<(Made, f32)> {
    let want = clamp_px(px);
    let mut r = registry().lock().ok()?;
    if r.missing {
        return None;
    }
    if let Some(m) = r.made.get(&(cut, want)) {
        return Some((*m, px / m.px as f32));
    }
    if !r.wanted.contains(&(cut, want)) {
        r.wanted.push((cut, want));
    }
    // Meanwhile the nearest of the same cut (any cut if none).
    let near = r
        .made
        .iter()
        .filter(|((c, _), _)| *c == cut)
        .min_by_key(|((_, p), _)| (*p as i32 - want as i32).abs())
        .or_else(|| r.made.iter().next())
        .map(|(_, m)| *m)?;
    Some((near, px / near.px as f32))
}

/// Make the sizes asked for since last frame.
pub(super) fn make_wanted(fe: Option<ResMut<Frontend>>, mut images: ResMut<Assets<Image>>) {
    let Some(mut fe) = fe else { return };
    let wanted = {
        let Ok(mut r) = registry().lock() else { return };
        // The first frame: a few common sizes, so something is ready.
        if r.made.is_empty() && r.wanted.is_empty() && !r.missing {
            r.wanted.extend([(Cut::Regular, 20), (Cut::Semi, 20), (Cut::Display, 40)]);
        }
        std::mem::take(&mut r.wanted)
    };
    if wanted.is_empty() {
        return;
    }
    let Some(bytes) = face() else {
        if let Ok(mut r) = registry().lock() {
            r.missing = true;
        }
        return;
    };
    // At most a few per frame (each is a millisecond or two).
    let (now, later) = wanted.split_at(wanted.len().min(6));
    for &(cut, px) in now {
        let Some((font, image, ascent, cap)) = rasterise(bytes, cut, px, &mut images) else { continue };
        let material = font.material.clone();
        fe.assets.add_image(&material, image);
        fe.assets.fonts.push(font);
        let made = Made { font: fe.assets.fonts.len() - 1, px, ascent, cap };
        if let Ok(mut r) = registry().lock() {
            r.made.insert((cut, px), made);
        }
    }
    if let Ok(mut r) = registry().lock() {
        r.wanted.extend_from_slice(later);
    }
}

/// Printable ASCII and a few extras into one atlas.
fn rasterise(bytes: &[u8], cut: Cut, px: u16, images: &mut Assets<Image>) -> Option<(iw3::menu::Font, UiImage, f32, f32)> {
    use ab_glyph::{Font as _, FontRef, PxScale, ScaleFont, VariableFont as _};
    let mut font = FontRef::try_from_slice(bytes).ok()?;
    let (wght, wdth) = cut.axes();
    font.set_variation(b"wght", wght);
    font.set_variation(b"wdth", wdth);
    let scale = PxScale::from(px as f32);
    let scaled = font.as_scaled(scale);
    let chars: Vec<char> =
        (32u8..127).map(char::from).chain(['\u{b7}', '\u{2013}', '\u{2014}', '\u{2019}', '\u{2022}', '\u{d7}', '\u{b0}']).collect();
    // Room for every glyph: rows of the em's height.
    let cell = px as u32 + 4;
    let w: u32 = (cell * 12).clamp(128, 2048);
    let rows = (chars.len() as u32 * (px as u32 * 3 / 4 + 3)).div_ceil(w) + 2;
    let h = (rows * cell).max(32);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    let (mut x, mut y, mut row) = (1u32, 1u32, 0u32);
    let mut glyphs = Vec::new();
    for c in chars {
        let id = font.glyph_id(c);
        let advance = scaled.h_advance(id).round().clamp(0.0, 255.0) as u8;
        let g = id.with_scale_and_position(scale, ab_glyph::point(0.0, 0.0));
        let Some(outline) = font.outline_glyph(g) else {
            glyphs.push(iw3::menu::Glyph { letter: c as u16, x0: 0, y0: 0, dx: advance, pixel_width: 0, pixel_height: 0, s0: 0.0, t0: 0.0, s1: 0.0, t1: 0.0 });
            continue;
        };
        let b = outline.px_bounds();
        let (gw, gh) = (b.width().ceil() as u32, b.height().ceil() as u32);
        if x + gw + 2 >= w {
            x = 1;
            y += row + 2;
            row = 0;
        }
        if y + gh + 2 >= h {
            warn!("new ui: atlas full at {c:?} ({cut:?} {px})");
            break;
        }
        outline.draw(|gx, gy, cov| {
            let i = (((y + gy) * w + x + gx) * 4) as usize;
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
            s0: x as f32 / w as f32,
            t0: y as f32 / h as f32,
            s1: (x + gw) as f32 / w as f32,
            t1: (y + gh) as f32 / h as f32,
        });
        x += gw + 2;
        row = row.max(gh);
    }
    let cap = font.outline_glyph(font.glyph_id('H').with_scale(scale)).map_or(px as f32 * 0.7, |o| o.px_bounds().height());
    let mut image = Image::new(
        bevy::render::render_resource::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        bevy::render::render_resource::TextureDimension::D2,
        pixels,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::linear();
    let size = Vec2::new(w as f32, h as f32);
    let handle = images.add(image);
    let name = format!("next:font_{}_{px}", cut.tag());
    let font = iw3::menu::Font { name: name.clone(), pixel_height: px as i32, material: name, glow_material: String::new(), glyphs };
    Some((font, UiImage { handle, size }, scaled.ascent(), cap))
}
