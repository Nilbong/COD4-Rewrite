//! Parse a fastfile and print what was found.
//!
//! cargo run -p iw3 --example zoneinfo -- mp_killhouse

use anyhow::Result;
use iw3::zone::{Asset, ParseOptions, Zone};
use std::collections::BTreeMap;
use std::time::Instant;

fn main() -> Result<()> {
    let name = std::env::args().nth(1).unwrap_or_else(|| "mp_killhouse".into());
    let install = iw3::Install::locate()?;
    let t = Instant::now();
    let data = iw3::fastfile::load(&install.zone_path(&name))?;
    println!("decompressed {} bytes in {:?}", data.len(), t.elapsed());

    let t = Instant::now();
    let stop_after =
        std::env::var("STOP").ok().and_then(|s| s.parse::<u32>().ok()).and_then(iw3::zone::AssetType::from_u32);
    let zone = Zone::parse(&data, ParseOptions { stop_after })?;
    println!("parsed in {:?}: {:#?}", t.elapsed(), zone.stats);

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut generic: BTreeMap<String, usize> = BTreeMap::new();
    for a in &zone.assets {
        let k = match a {
            Asset::Generic(g) => {
                *generic.entry(format!("{:?}", g.ty)).or_default() += 1;
                continue;
            }
            Asset::TechniqueSet(_) => "techset",
            Asset::Material(_) => "material",
            Asset::Image(_) => "image",
            Asset::XModel(_) => "xmodel",
            Asset::XModelPieces(_) => "xmodelpieces",
            Asset::PhysPreset(_) => "physpreset",
            Asset::LightDef(_) => "lightdef",
            Asset::ComWorld(_) => "comworld",
            Asset::GameWorldMp(_) => "gameworldmp",
            Asset::MapEnts(_) => "mapents",
            Asset::GfxWorld(_) => "gfxworld",
            Asset::ClipMap(_) => "clipmap",
            Asset::RawFile(_) => "rawfile",
        };
        *counts.entry(k).or_default() += 1;
    }
    println!("assets: {counts:?}");
    println!("generic assets: {generic:?}");

    if let Some(w) = zone.gfx_world() {
        println!(
            "gfxworld {} ({}): {} verts, {} indices, {} surfaces, {} static models, {} lightmaps",
            w.name,
            w.base_name,
            w.vertices.len(),
            w.indices.len(),
            w.surfaces.len(),
            w.static_models.len(),
            w.lightmaps.len()
        );
        println!("  bounds {:?} .. {:?}", w.mins, w.maxs);
        println!("  sun {:?}", w.sun);
        let with_mat = w.surfaces.iter().filter(|s| s.material.is_some()).count();
        println!("  surfaces with resolved material: {with_mat}/{}", w.surfaces.len());
        let with_model = w.static_models.iter().filter(|s| s.model.is_some()).count();
        println!("  static models with resolved model: {with_model}/{}", w.static_models.len());
        for s in w.surfaces.iter().take(5) {
            let m = s.material.and_then(|m| zone.material(m));
            println!(
                "  surf {:?} mat={:?}",
                (s.first_vertex, s.vertex_count, s.tri_count, s.base_index),
                m.map(|m| &m.name)
            );
        }
        for sm in w.static_models.iter().take(5) {
            let m = sm.model.and_then(|m| zone.xmodel(m));
            println!("  smodel {:?} at {:?} scale {}", m.map(|m| &m.name), sm.origin, sm.scale);
        }
    }
    if let Some(c) = zone.clip_map() {
        println!(
            "clipmap {}: {} planes, {} brushes, {} verts, {} tris, {} cmodels, {} materials",
            c.name,
            c.planes.len(),
            c.brushes.len(),
            c.verts.len(),
            c.tri_indices.len() / 3,
            c.cmodels.len(),
            c.materials.len()
        );
        // Sanity check: every non-axial side plane should touch its brush's AABB.
        let mut bad = 0;
        for b in &c.brushes {
            for &p in &b.side_planes {
                let pl = c.planes[p as usize];
                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                for i in 0..8 {
                    let corner = [
                        if i & 1 == 0 { b.mins[0] } else { b.maxs[0] },
                        if i & 2 == 0 { b.mins[1] } else { b.maxs[1] },
                        if i & 4 == 0 { b.mins[2] } else { b.maxs[2] },
                    ];
                    let d = corner[0] * pl.normal[0] + corner[1] * pl.normal[1] + corner[2] * pl.normal[2] - pl.dist;
                    lo = lo.min(d);
                    hi = hi.max(d);
                }
                if !(lo <= 0.5 && hi >= -0.5) {
                    bad += 1;
                }
            }
        }
        let total: usize = c.brushes.iter().map(|b| b.side_planes.len()).sum();
        println!("  brush side planes not touching their AABB: {bad}/{total}");
    }
    if let Some(e) = zone.map_ents() {
        let ents = iw3::ents::parse(&e.entity_string);
        let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
        for ent in &ents {
            *classes.entry(ent.classname()).or_default() += 1;
        }
        println!("entities ({}): {classes:?}", ents.len());
    }
    Ok(())
}
