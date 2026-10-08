//! Player-made camos (the Camo Editor, `ui/custom_camo.rs`): a pattern in
//! two to four colours, its scale and turn on the gun, and a finish.
//!
//! Custom camo slot `n` is camo `FIRST + n` in the class camo stats, beside
//! CoD4's (0..6), Black Ops' (100 + n) and the mastery finishes (200, 201).
//! The editor's unsaved draft is [`DRAFT`]. Definitions live in the profile
//! (saved dvars `cod4rw_ccamo_<n>`) and are copied here, where
//! [`crate::gunmodel`] reads them for every gun it builds: first and third
//! person, dropped guns, the killcam and the menus' previews.
//!
//! Each definition is baked once into a small tiling texture (with mips);
//! the gun shader (`ui/camo.wgsl`, mode 6) paints it over the gun's own
//! wear, turned and scaled by uniforms. Patterns are CoD4's own camo
//! textures recoloured (their brightness split into equal-area bands), or
//! made here: digital, blobs, tiger stripes and solid.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

/// Camo number of custom slot 0.
pub const FIRST: usize = 300;
/// Saved custom camos at most.
pub const SLOTS: usize = 8;
/// The editor's draft, shown in its preview only.
pub const DRAFT: usize = FIRST + SLOTS;
/// The saved dvar of slot `n` is this and `n`.
pub const DVAR_PREFIX: &str = "cod4rw_ccamo_";
/// Longest name.
pub const NAME_LEN: usize = 16;
/// Baked texture size (square, tiling).
const SIZE: usize = 256;

/// Is camo `camo` a custom one (or the draft)?
pub fn is_custom(camo: usize) -> bool {
    (FIRST..=DRAFT).contains(&camo)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub name: &'static str,
    /// CoD4's camo texture recoloured, or none for a made one.
    pub texture: Option<&'static str>,
}

/// The patterns, in the editor's order. Saved by index: only add at the end.
pub const PATTERNS: [Pattern; 9] = [
    Pattern { name: "Woodland", texture: Some("weapon_camo_brockhaurd") },
    Pattern { name: "Desert", texture: Some("weapon_camo_bush_dweller") },
    Pattern { name: "Marpat", texture: Some("weapon_camo_black_white_marpat") },
    Pattern { name: "Commando Tiger", texture: Some("weapon_camo_commando_tiger_red") },
    Pattern { name: "Stagger", texture: Some("weapon_camo_stagger_blue") },
    Pattern { name: "Digital", texture: None },
    Pattern { name: "Blobs", texture: None },
    Pattern { name: "Tiger Stripes", texture: None },
    Pattern { name: "Solid", texture: None },
];
const DIGITAL: usize = 5;
const BLOBS: usize = 6;
const STRIPES: usize = 7;
const SOLID: usize = 8;

/// The editor's colours (sRGB). Saved as colours, not indices.
pub const PALETTE: [[u8; 3]; 24] = [
    [24, 24, 22],
    [58, 58, 54],
    [110, 110, 104],
    [176, 174, 166],
    [232, 230, 222],
    [62, 72, 38],
    [96, 104, 58],
    [52, 66, 52],
    [120, 128, 92],
    [82, 62, 40],
    [132, 102, 66],
    [186, 160, 116],
    [216, 196, 152],
    [44, 56, 74],
    [70, 96, 132],
    [130, 158, 186],
    [120, 30, 26],
    [196, 64, 36],
    [226, 140, 40],
    [214, 186, 60],
    [52, 120, 96],
    [86, 52, 112],
    [200, 90, 140],
    [36, 150, 170],
];

/// Pattern scales the editor steps through (bigger is a larger pattern).
pub const SCALES: [f32; 9] = [0.4, 0.55, 0.7, 0.85, 1.0, 1.25, 1.5, 2.0, 2.5];
/// Rotation steps, degrees.
pub const ROTATION_STEP: u16 = 15;
pub const FINISHES: [&str; 3] = ["Matte", "Satin", "Gloss"];

#[derive(Clone, Debug, PartialEq)]
pub struct CustomCamo {
    pub name: String,
    pub pattern: usize,
    /// sRGB colours; the first `count` are used.
    pub colors: [[u8; 3]; 4],
    /// 2..=4 (a solid uses the first).
    pub count: usize,
    /// Index into [`SCALES`].
    pub scale: usize,
    /// Degrees, a multiple of [`ROTATION_STEP`].
    pub rotation: u16,
    /// Index into [`FINISHES`].
    pub finish: usize,
}

impl Default for CustomCamo {
    fn default() -> Self {
        CustomCamo {
            name: "Custom Camo".into(),
            pattern: BLOBS,
            colors: [PALETTE[6], PALETTE[10], PALETTE[0], PALETTE[11]],
            count: 3,
            scale: 4,
            rotation: 0,
            finish: 0,
        }
    }
}

/// Only what the menus' fonts draw and the save file keeps on a line.
pub fn clean_name(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || " _-.".contains(*c))
        .take(NAME_LEN)
        .collect::<String>()
        .trim()
        .to_owned()
}

impl CustomCamo {
    /// `pattern,count,rrggbb.rrggbb.rrggbb.rrggbb,scale,rotation,finish,name`.
    pub fn encode(&self) -> String {
        let colors: Vec<String> = self.colors.iter().map(|c| format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect();
        format!(
            "{},{},{},{},{},{},{}",
            self.pattern,
            self.count,
            colors.join("."),
            self.scale,
            self.rotation,
            self.finish,
            clean_name(&self.name)
        )
    }

    pub fn decode(text: &str) -> Option<CustomCamo> {
        let mut parts = text.splitn(7, ',');
        let mut num = || parts.next()?.trim().parse::<usize>().ok();
        let (pattern, count) = (num()?, num()?);
        let colors_text = parts.next()?;
        let mut num = || parts.next()?.trim().parse::<usize>().ok();
        let (scale, rotation, finish) = (num()?, num()?, num()?);
        let name = clean_name(parts.next().unwrap_or(""));
        let mut colors = [[0u8; 3]; 4];
        for (slot, hex) in colors.iter_mut().zip(colors_text.split('.')) {
            let v = u32::from_str_radix(hex, 16).ok()?;
            *slot = [(v >> 16) as u8, (v >> 8) as u8, v as u8];
        }
        Some(CustomCamo {
            name: if name.is_empty() { "Custom Camo".into() } else { name },
            pattern: pattern.min(PATTERNS.len() - 1),
            colors,
            count: count.clamp(2, 4),
            scale: scale.min(SCALES.len() - 1),
            rotation: (rotation as u16 % 360) / ROTATION_STEP * ROTATION_STEP,
            finish: finish.min(FINISHES.len() - 1),
        })
    }

    #[allow(dead_code)]
    /// The definition as one compact line, for sending to other players
    /// (netplay's PawnInfo; not done yet: remote custom camos show none).
    pub fn to_compact(&self) -> String {
        self.encode()
    }

    #[allow(dead_code)]
    pub fn from_compact(text: &str) -> Option<CustomCamo> {
        Self::decode(text)
    }

    /// What the texture depends on (not the name, scale, turn or finish).
    fn texture_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.pattern, self.count, self.colors).hash(&mut h);
        h.finish()
    }

    /// Everything a gun material depends on.
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.texture_key(), self.scale, self.rotation, self.finish).hash(&mut h);
        h.finish()
    }

    /// The colours used.
    pub fn used(&self) -> &[[u8; 3]] {
        &self.colors[..if self.pattern == SOLID { 1 } else { self.count.clamp(2, 4) }]
    }

    /// Shader settings: the pattern's turn and size over the gun's UVs
    /// (x: cos / scale, y: sin / scale) and its gloss (0..1).
    pub fn transform(&self) -> (Vec2, f32) {
        let a = (self.rotation as f32).to_radians();
        let s = 1.0 / SCALES[self.scale.min(SCALES.len() - 1)];
        (Vec2::new(a.cos(), a.sin()) * s, self.finish as f32 / 2.0)
    }
}

/// The definitions in use: slots, then the draft.
static CAMOS: RwLock<[Option<Arc<CustomCamo>>; SLOTS + 1]> = RwLock::new([const { None }; SLOTS + 1]);

/// Custom camo `camo` (a camo number), if it's defined.
pub fn get(camo: usize) -> Option<Arc<CustomCamo>> {
    if !is_custom(camo) {
        return None;
    }
    CAMOS.read().ok()?[camo - FIRST].clone()
}

/// Set (or clear) custom camo `camo`.
pub fn set(camo: usize, def: Option<CustomCamo>) {
    if !is_custom(camo) {
        return;
    }
    if let Ok(mut camos) = CAMOS.write() {
        let slot = &mut camos[camo - FIRST];
        if slot.as_deref() != def.as_ref() {
            *slot = def.map(Arc::new);
        }
    }
}

/// The texture keys in use (to drop stale baked textures).
pub fn live_keys() -> Vec<u64> {
    CAMOS.read().map(|c| c.iter().flatten().map(|d| d.texture_key()).collect()).unwrap_or_default()
}

/// The material keys ([`CustomCamo::key`]) in use.
pub fn live_material_keys() -> Vec<u64> {
    CAMOS.read().map(|c| c.iter().flatten().map(|d| d.key()).collect()).unwrap_or_default()
}

/// Baked textures, by texture key, for every cache to share (the menus and
/// the match build their gun materials apart).
static TEXTURES: OnceLock<std::sync::Mutex<HashMap<u64, Handle<Image>>>> = OnceLock::new();

/// Camo `def`'s texture: baked the first time, after that shared.
pub fn texture(def: &CustomCamo, images: &mut Assets<Image>) -> Handle<Image> {
    let key = def.texture_key();
    let mut map = TEXTURES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = map.get(&key) {
        return h.clone();
    }
    // Edits in the editor make a texture each; keep the ones in use.
    if map.len() >= 24 {
        let live = live_keys();
        map.retain(|k, _| live.contains(k));
    }
    let h = images.add(image(&bake(def)));
    map.insert(key, h.clone());
    h
}

/// `rgba` (`SIZE`², sRGB) as a repeating image with its mips.
fn image(rgba: &[u8]) -> Image {
    let mut data = rgba.to_vec();
    let mut level = rgba.to_vec();
    let (mut size, mut mips) = (SIZE, 1);
    while size > 1 {
        let half = size / 2;
        let mut next = vec![0u8; half * half * 4];
        for y in 0..half {
            for x in 0..half {
                for c in 0..4 {
                    let at = |xx: usize, yy: usize| level[(yy * size + xx) * 4 + c] as u32;
                    let sum = at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1);
                    next[(y * half + x) * 4 + c] = ((sum + 2) / 4) as u8;
                }
            }
        }
        data.extend_from_slice(&next);
        level = next;
        size = half;
        mips += 1;
    }
    let mut image = Image::new_uninit(
        Extent3d { width: SIZE as u32, height: SIZE as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = mips;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    });
    image
}

/// The texture's pixels: `SIZE`² RGBA, sRGB.
pub fn bake(def: &CustomCamo) -> Vec<u8> {
    let colors = def.used();
    let field = field(def.pattern);
    let bands = bands(&field, colors.len());
    let mut out = Vec::with_capacity(SIZE * SIZE * 4);
    for v in field {
        let band = bands.iter().filter(|&&t| v > t).count().min(colors.len() - 1);
        let c = colors[band];
        out.extend_from_slice(&[c[0], c[1], c[2], 255]);
    }
    out
}

/// Thresholds splitting `field` into `n` bands of equal area.
fn bands(field: &[f32], n: usize) -> Vec<f32> {
    let mut sorted = field.to_vec();
    sorted.sort_by(f32::total_cmp);
    (1..n).map(|i| sorted[(sorted.len() * i / n).min(sorted.len() - 1)]).collect()
}

/// The pattern's brightness field, `SIZE`², tiling.
fn field(pattern: usize) -> Vec<f32> {
    let p = PATTERNS[pattern.min(PATTERNS.len() - 1)];
    if let Some(name) = p.texture
        && let Some(f) = cod4_field(name)
    {
        return f;
    }
    let mut out = vec![0.0; SIZE * SIZE];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (u, v) = (x as f32 / SIZE as f32, y as f32 / SIZE as f32);
            out[y * SIZE + x] = match pattern {
                // Blocks of 8 texels: blobby noise, sampled per block, with
                // smaller blocks breaking the edges.
                DIGITAL => {
                    let (bu, bv) = ((x / 8) as f32 * 8.0 / SIZE as f32, (y / 8) as f32 * 8.0 / SIZE as f32);
                    let (su, sv) = ((x / 4) as f32 * 4.0 / SIZE as f32, (y / 4) as f32 * 4.0 / SIZE as f32);
                    fbm(bu, bv, 4, 3, 11) + 0.35 * noise(su, sv, 32, 23)
                }
                BLOBS => fbm(u, v, 3, 4, 7),
                // Diagonal stripes, warped and broken.
                STRIPES => {
                    let warp = fbm(u, v, 2, 3, 5) * 2.2;
                    let s = ((u * 2.0 + v * 6.0 + warp) * std::f32::consts::TAU).sin();
                    s + 0.45 * fbm(u, v, 8, 2, 9)
                }
                _ => 0.0,
            };
        }
    }
    out
}

/// Tiling value noise over a `cells`² lattice (0..1 in, about 0..1 out).
fn noise(u: f32, v: f32, cells: usize, seed: u32) -> f32 {
    let (x, y) = (u.rem_euclid(1.0) * cells as f32, v.rem_euclid(1.0) * cells as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let at = |i: usize, j: usize| hash((i % cells) as u32, (j % cells) as u32, seed);
    let top = at(x0, y0) + (at(x0 + 1, y0) - at(x0, y0)) * sx;
    let bottom = at(x0, y0 + 1) + (at(x0 + 1, y0 + 1) - at(x0, y0 + 1)) * sx;
    top + (bottom - top) * sy
}

/// Octaves of [`noise`] from `cells` up.
fn fbm(u: f32, v: f32, cells: usize, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut c) = (0.0, 1.0, cells);
    for o in 0..octaves {
        sum += amp * noise(u, v, c, seed + o * 101);
        amp *= 0.5;
        c *= 2;
    }
    sum
}

fn hash(x: u32, y: u32, seed: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6b343) ^ y.wrapping_mul(0xd8163841) ^ seed.wrapping_mul(0xcb1ab31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// CoD4's camo texture `name` as brightness, upscaled to `SIZE`² (smoothly,
/// so the bands' edges are curves, not texel steps). Loaded once.
fn cod4_field(name: &str) -> Option<Vec<f32>> {
    static FIELDS: OnceLock<std::sync::Mutex<HashMap<String, Option<Vec<f32>>>>> = OnceLock::new();
    let mut map = FIELDS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    map.entry(name.to_owned())
        .or_insert_with(|| {
            let (w, h, rgba) = cod4_pixels(name)?;
            let luma = |x: usize, y: usize| {
                let p = &rgba[((y % h) * w + (x % w)) * 4..];
                0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
            };
            let mut out = vec![0.0; SIZE * SIZE];
            for y in 0..SIZE {
                for x in 0..SIZE {
                    let (fx, fy) = (x as f32 * w as f32 / SIZE as f32 - 0.5, y as f32 * h as f32 / SIZE as f32 - 0.5);
                    let (x0, y0) = (fx.floor(), fy.floor());
                    let (tx, ty) = (fx - x0, fy - y0);
                    let (xi, yi) = ((x0 as isize).rem_euclid(w as isize) as usize, (y0 as isize).rem_euclid(h as isize) as usize);
                    let a = luma(xi, yi) + (luma(xi + 1, yi) - luma(xi, yi)) * tx;
                    let b = luma(xi, yi + 1) + (luma(xi + 1, yi + 1) - luma(xi, yi + 1)) * tx;
                    out[y * SIZE + x] = a + (b - a) * ty;
                }
            }
            Some(out)
        })
        .clone()
}

/// The CoD4 install's files, mounted once (the patterns are CoD4's, whichever
/// game's gun is painted).
fn cod4_vfs() -> Option<&'static iw3::iwd::Vfs> {
    static VFS: OnceLock<Option<iw3::iwd::Vfs>> = OnceLock::new();
    VFS.get_or_init(|| {
        let install = iw3::Install::locate().ok()?;
        iw3::iwd::Vfs::mount(&install.iwd_paths().ok()?).ok()
    })
    .as_ref()
}

fn cod4_pixels(name: &str) -> Option<(usize, usize, Vec<u8>)> {
    let bytes = cod4_vfs()?.read(&format!("images/{name}.iwi")).ok()??;
    let iwi = iw3::iwi::Iwi::parse(&bytes).ok()?;
    let (w, h) = (iwi.width as usize, iwi.height as usize);
    let rgba = match iwi.format {
        iw3::iwi::Format::Dxt1 => decode_dxt1(iwi.levels.first()?, w, h)?,
        _ => iwi.to_rgba8(0)?,
    };
    Some((w, h, rgba))
}

/// DXT1 (BC1) blocks to RGBA8.
fn decode_dxt1(data: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    if data.len() < bw * bh * 8 {
        return None;
    }
    let mut out = vec![0u8; w * h * 4];
    let rgb = |c: u16| {
        let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
        [(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8]
    };
    for by in 0..bh {
        for bx in 0..bw {
            let b = &data[(by * bw + bx) * 8..];
            let (c0, c1) = (u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]]));
            let (p0, p1) = (rgb(c0), rgb(c1));
            let mix = |a: u8, b: u8, wa: u32, wb: u32| ((a as u32 * wa + b as u32 * wb) / (wa + wb)) as u8;
            let palette: [[u8; 4]; 4] = if c0 > c1 {
                [
                    [p0[0], p0[1], p0[2], 255],
                    [p1[0], p1[1], p1[2], 255],
                    [mix(p0[0], p1[0], 2, 1), mix(p0[1], p1[1], 2, 1), mix(p0[2], p1[2], 2, 1), 255],
                    [mix(p0[0], p1[0], 1, 2), mix(p0[1], p1[1], 1, 2), mix(p0[2], p1[2], 1, 2), 255],
                ]
            } else {
                [
                    [p0[0], p0[1], p0[2], 255],
                    [p1[0], p1[1], p1[2], 255],
                    [mix(p0[0], p1[0], 1, 1), mix(p0[1], p1[1], 1, 1), mix(p0[2], p1[2], 1, 1), 255],
                    [0, 0, 0, 0],
                ]
            };
            let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
            for i in 0..16 {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < w && y < h {
                    let c = palette[((bits >> (2 * i)) & 3) as usize];
                    out[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&c);
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_round_trip_and_sanitise() {
        let mut c = CustomCamo { name: "My, Camo!\nX".into(), pattern: 3, count: 4, rotation: 45, ..default() };
        c.colors[2] = [1, 2, 3];
        let back = CustomCamo::decode(&c.encode()).unwrap();
        assert_eq!(back.name, "My CamoX");
        assert_eq!((back.pattern, back.count, back.rotation, back.colors[2]), (3, 4, 45, [1, 2, 3]));
        assert!(CustomCamo::decode("junk").is_none());
        let wild = CustomCamo::decode("99,9,ffffff.000000,99,400,9,").unwrap();
        assert_eq!((wild.pattern, wild.count, wild.scale, wild.rotation, wild.finish), (8, 4, 8, 30, 2));
        assert_eq!(wild.name, "Custom Camo");
    }

    #[test]
    fn made_patterns_use_every_colour_about_equally() {
        for pattern in [DIGITAL, BLOBS, STRIPES] {
            let def = CustomCamo { pattern, count: 4, colors: [[0; 3], [80; 3], [160; 3], [240; 3]], ..default() };
            let px = bake(&def);
            assert_eq!(px.len(), SIZE * SIZE * 4);
            for c in [0u8, 80, 160, 240] {
                let share = px.chunks(4).filter(|p| p[0] == c).count() as f32 / (SIZE * SIZE) as f32;
                assert!((0.15..0.35).contains(&share), "pattern {pattern} colour {c}: {share}");
            }
        }
        let solid = bake(&CustomCamo { pattern: SOLID, ..default() });
        assert!(solid.chunks(4).all(|p| p[..3] == PALETTE[6]));
    }

    #[test]
    fn mips_go_down_to_one_texel() {
        let img = image(&bake(&CustomCamo::default()));
        assert_eq!(img.texture_descriptor.mip_level_count, 9);
        assert_eq!(img.data.as_ref().unwrap().len(), (0..9).map(|l| (SIZE >> l).pow(2) * 4).sum::<usize>());
    }

    #[test]
    fn slots_and_draft_are_custom() {
        assert!(!is_custom(0) && !is_custom(200) && !is_custom(FIRST - 1) && !is_custom(DRAFT + 1));
        assert!(is_custom(FIRST) && is_custom(DRAFT));
    }
}
