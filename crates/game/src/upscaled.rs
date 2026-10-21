//! Remastered textures (`tools/upscale/remaster.py`): CoD4 images upscaled
//! 4x (colour by Real-ESRGAN; normal and specular maps bicubic and
//! sharpened) with made-up material maps, as CoD4 v6 `.iwi` files in
//! `data/upscaled/<map>/`. On (`COD4RW_UPSCALED=1`, or knob `upscale.on`
//! = 1, read when a map loads) they're loaded in place of the originals
//! (`crate::textures`), by image name, on any map that uses the image.
//!
//! Beside each image `<name>.iwi` there may be:
//! - `<colour>.rough.iwi`: roughness in green (from the material's gloss
//!   where it has a specular map, else from the colour), on every material
//!   (`crate::content`); the world shader's own gloss wins where there's a
//!   specular map;
//! - `<colour>.gen_n.iwi`: a normal map made from the colour, for world
//!   materials that have none (IW3's slope packing);
//! - `<normal or colour>.height.iwi`: heights for parallax occlusion
//!   mapping (`shaders/world.wgsl`'s `relief_uv`), used on large flat
//!   opaque world surfaces only, at depth `upscale.parallax`.
//!
//! `COD4RW_UPSCALED_DIR` points elsewhere than `data/upscaled`.

use bevy::prelude::*;
use iw3::iwi::Iwi;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Whether remastered textures replace the originals.
pub fn on() -> bool {
    std::env::var("COD4RW_UPSCALED").is_ok_and(|v| v == "1") || crate::tune::get("upscale.on", 0.0) > 0.5
}

/// Parallax depth (texture widths) on large flat surfaces; 0 for none.
pub fn parallax_depth() -> f32 {
    crate::tune::get("upscale.parallax", 0.012).max(0.0)
}

/// Remastered files by lower-case name (`ch_plywood01_col`,
/// `ch_plywood01_col.rough`), from every map folder.
fn index() -> &'static HashMap<String, PathBuf> {
    static INDEX: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut map = HashMap::new();
        let Some(root) = root() else { return map };
        let Ok(dirs) = std::fs::read_dir(&root) else { return map };
        for dir in dirs.flatten().map(|d| d.path()).filter(|p| p.is_dir()) {
            for f in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = f.path();
                if let Some(stem) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".iwi")) {
                    map.insert(stem.to_lowercase(), p);
                }
            }
        }
        info!("upscaled textures: {} files under {}", map.len(), root.display());
        map
    })
}

/// `data/upscaled`, looked for above the exe and the working directory.
fn root() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("COD4RW_UPSCALED_DIR") {
        return Some(PathBuf::from(d));
    }
    let exe = std::env::current_exe().ok();
    let cwd = std::env::current_dir().ok();
    [exe, cwd].into_iter().flatten().find_map(|start| start.ancestors().map(|a| a.join("data").join("upscaled")).find(|p| p.is_dir()))
}

/// The remastered image `name` (an image name, optionally with a suffix
/// such as `.rough`), when there is one.
pub fn load(name: &str, srgb: bool) -> Option<Image> {
    let path = index().get(&name.to_lowercase())?;
    let bytes = std::fs::read(path).inspect_err(|e| warn!("upscaled {}: {e}", path.display())).ok()?;
    let iwi = Iwi::parse(&bytes).inspect_err(|e| warn!("upscaled {}: {e}", path.display())).ok()?;
    crate::textures::to_image(&iwi, srgb)
}

/// Whether a world material's surfaces are big flat planes (floors, walls)
/// worth parallax: most of their area in surfaces whose triangles all face
/// one way, and plenty of it.
pub fn large_flat(world: &iw3::zone::GfxWorld, surfs: &[usize]) -> bool {
    let (mut flat, mut total) = (0.0f32, 0.0f32);
    for &si in surfs {
        let Some(s) = world.surfaces.get(si) else { continue };
        let first = s.first_vertex.max(0) as usize;
        let start = s.base_index.max(0) as usize;
        let Some(tris) = world.indices.get(start..start + s.tri_count as usize * 3) else { continue };
        let mut normals = Vec::with_capacity(s.tri_count as usize);
        let mut sum = Vec3::ZERO;
        for t in tris.chunks_exact(3) {
            let p = |i: u16| world.vertices.get(first + i as usize).map(|v| Vec3::from(v.xyz));
            let (Some(a), Some(b), Some(c)) = (p(t[0]), p(t[1]), p(t[2])) else { continue };
            let n = (b - a).cross(c - a);
            sum += n;
            normals.push(n);
        }
        let area: f32 = normals.iter().map(|n| n.length() * 0.5).sum();
        let mean = sum.normalize_or_zero();
        // Within ~8 degrees of the surface's mean facing.
        let planar = normals.iter().all(|n| n.length() < 1e-6 || n.normalize().dot(mean) > 0.99);
        total += area;
        if planar {
            flat += area;
        }
    }
    // CoD4 units are inches: at least ~4 m² in all, ~90% of it flat.
    total > 6000.0 && flat > 0.9 * total
}
