//! Convert a Modern Warfare 2 map zone to `iw3` assets and summarise it:
//! `convcheck [map]`.
fn main() -> anyhow::Result<()> {
    let name = std::env::args().nth(1).unwrap_or("mp_terminal".into());
    let install = iw4::Install::locate()?;
    let z = iw4::load_map(&install, &name)?;
    let w = z.gfx_world().expect("world");
    println!("world {}: {} verts, {} indices, {} surfaces ({} with material), {} static models, {} lightmaps, {} probes, {} brush models",
        w.name, w.vertices.len(), w.indices.len(), w.surfaces.len(), w.surfaces.iter().filter(|s| s.material.is_some()).count(),
        w.static_models.len(), w.lightmaps.len(), w.reflection_probes.len(), w.models.len());
    println!("  bounds {:?} {:?}; lit {:?} decal {:?} emissive {:?}; sun {:?} {:?}", w.mins, w.maxs, w.lit_surfs, w.decal_surfs, w.emissive_surfs, w.sun.angles, w.sun.sun_color);
    println!("  light grid: {} entries, {} colours, rows {}", w.light_grid.entries.len(), w.light_grid.colors.len(), w.light_grid.row_data_start.len());
    let c = z.clip_map().expect("clip map");
    let sided = c.brushes.iter().filter(|b| !b.side_planes.is_empty()).count();
    println!("clip {}: {} planes, {} brushes ({} with sides), {} materials, {} verts, {} tris, {} partitions, {} aabb, {} cmodels, {} leaf nodes ({} with brushes), {} dyn ents, {} static models",
        c.name, c.planes.len(), c.brushes.len(), sided, c.materials.len(), c.verts.len(), c.tri_indices.len() / 3, c.partitions.len(), c.aabb_trees.len(),
        c.cmodels.len(), c.leaf_brush_nodes.len(), c.leaf_brush_nodes.iter().filter(|n| !n.brushes.is_empty()).count(), c.dyn_ents.len(), c.static_models.len());
    if let Some(b) = c.brushes.iter().find(|b| !b.side_planes.is_empty()) {
        println!("  first sided brush {:?}..{:?} planes {:?}", b.mins, b.maxs, b.side_planes.iter().map(|&p| c.planes[p as usize]).collect::<Vec<_>>());
    }
    let no_model: Vec<_> = c.dyn_ents.iter().filter(|d| d.model.is_none()).collect();
    println!("dyn ents without model: {} (kinds {:?}, brush models {:?})", no_model.len(),
        no_model.iter().map(|d| d.kind).collect::<std::collections::BTreeSet<_>>(), no_model.iter().take(8).map(|d| (d.brush_model, d.physics_brush_model)).collect::<Vec<_>>());
    let empty: std::collections::BTreeSet<String> = w.static_models.iter().filter_map(|s| s.model.and_then(|m| z.xmodel(m))).filter(|m| m.surfs.iter().all(|s| s.verts.is_empty())).map(|m| m.name.clone()).collect();
    println!("static models without geometry: {} {:?}", empty.len(), empty.iter().take(12).collect::<Vec<_>>());
    let ent_list = iw3::ents::parse(&z.map_ents().unwrap().entity_string);
    let missing: std::collections::BTreeSet<String> = ent_list.iter().filter_map(|e| e.get("model")).filter(|m| !m.starts_with('*'))
        .filter(|m| z.assets.iter().all(|a| !matches!(a, iw3::zone::Asset::XModel(x) if x.name == *m && x.surfs.iter().any(|s| !s.verts.is_empty())))).map(str::to_owned).collect();
    println!("entity models missing or empty: {} {:?}", missing.len(), missing);
    let mut cd: Vec<f32> = w.static_models.iter().map(|s| s.cull_dist).collect();
    cd.sort_by(f32::total_cmp);
    println!("cull dists: min {} p10 {} median {} max {}; zero {}", cd[0], cd[cd.len()/10], cd[cd.len()/2], cd[cd.len()-1], cd.iter().filter(|c| **c == 0.0).count());
    let ents = z.map_ents().map_or(0, |e| e.entity_string.len());
    println!("ents {} bytes; primary lights {}", ents, z.com_world().map_or(0, |c| c.primary_lights.len()));
    let models: Vec<_> = z.assets.iter().filter_map(|a| match a { iw3::zone::Asset::XModel(m) => Some(m), _ => None }).collect();
    println!("{} models; first {} with {} surfs, {} lods", models.len(), models[0].name, models[0].surfs.len(), models[0].lods.len());
    let mats: Vec<_> = z.assets.iter().filter_map(|a| match a { iw3::zone::Asset::Material(m) => Some(m), _ => None }).collect();
    let m = mats.iter().find(|m| m.name.starts_with("wc/")).unwrap();
    println!("{} materials; {} textures {:?} techset {:?} bits {:?}", mats.len(), m.name,
        m.textures.iter().map(|t| (t.semantic, t.image.map(|i| z.image(i).map(|x| x.name.clone())))).collect::<Vec<_>>(),
        m.technique_set.and_then(|t| z.technique_set(t)).map(|t| t.name.clone()), m.state_bits_for(7));
    // A streamed texture through the vfs.
    let vfs = install.vfs()?;
    let img = m.textures.iter().find_map(|t| t.image.and_then(|i| z.image(i)));
    if let Some(img) = img {
        let bytes = vfs.read(&format!("images/{}.iwi", img.name))?;
        println!("texture {}: {:?}", img.name, bytes.as_ref().map(|b| iw3::iwi::Iwi::parse(b).map(|i| (i.format, i.width, i.height, i.levels.len())).map_err(|e| e.to_string())));
    }
    Ok(())
}
