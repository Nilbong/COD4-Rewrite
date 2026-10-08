//! Find world surfaces whose lightmap reads black: lmapblack <zone> [material part]
//! For each material (or the one named), sample the secondary lightmap (both
//! halves) at its surfaces' vertices and report the share near black, the
//! lightmap index and the UV range. Also reports vertex colours there.
use anyhow::Result;
use iw3::zone::{ParseOptions, Zone};
use std::collections::BTreeMap;

fn main() -> Result<()> {
    let zone_name = std::env::args().nth(1).unwrap_or("mp_carentan".into());
    let part = std::env::args().nth(2);
    let install = iw3::Install::locate()?;
    let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path(&zone_name))?, ParseOptions::default())?;
    let w = zone.gfx_world().expect("gfxworld");
    // Each lightmap's secondary (ARGB8: B, G, R, A in memory), its size.
    let pages: Vec<Option<(u32, u32, &[u8])>> = w
        .lightmaps
        .iter()
        .map(|l| {
            let img = l.secondary.and_then(|id| zone.image(id))?;
            let def = img.load_def.as_ref()?;
            Some((img.width as u32, img.height as u32, def.data.as_slice()))
        })
        .collect();
    let texel = |page: usize, u: f32, v: f32| -> Option<[f32; 3]> {
        let (w, h, d) = pages.get(page).copied().flatten()?;
        let x = ((u.clamp(0.0, 0.9999)) * w as f32) as u32;
        let y = ((v.clamp(0.0, 0.9999)) * h as f32) as u32;
        let i = ((y * w + x) * 4) as usize;
        d.get(i..i + 3).map(|p| [p[2] as f32 / 255.0, p[1] as f32 / 255.0, p[0] as f32 / 255.0])
    };
    #[derive(Default)]
    struct Stat {
        verts: u32,
        black: u32,
        outside: u32,
        uv_min: [f32; 2],
        uv_max: [f32; 2],
        pages: BTreeMap<u8, u32>,
        sum: f32,
        colour: [f32; 4],
    }
    let mut stats: BTreeMap<String, Stat> = BTreeMap::new();
    for s in &w.surfaces {
        let Some(name) = s.material.and_then(|m| zone.material(m)).map(|m| m.name.clone()) else { continue };
        if part.as_ref().is_some_and(|p| !name.contains(p.as_str())) {
            continue;
        }
        if s.lightmap_index == 31 || s.lightmap_index == 255 {
            continue;
        }
        let st = stats.entry(name).or_insert_with(|| Stat { uv_min: [f32::MAX; 2], uv_max: [f32::MIN; 2], ..Default::default() });
        *st.pages.entry(s.lightmap_index).or_default() += 1;
        // Near a point (`LMAP_NEAR=x,y,z,r`, CoD units), if given.
        if let Some(n) = std::env::var("LMAP_NEAR").ok().map(|v| v.split(',').filter_map(|x| x.parse::<f32>().ok()).collect::<Vec<_>>()).filter(|n| n.len() == 4) {
            let c = [0, 1, 2].map(|k| (s.bounds[0][k] + s.bounds[1][k]) * 0.5);
            if (0..3).map(|k| (c[k] - n[k]).powi(2)).sum::<f32>().sqrt() > n[3] {
                continue;
            }
        }
        let first = s.first_vertex.max(0) as usize;
        let start = s.base_index.max(0) as usize;
        let idx = &w.indices[start.min(w.indices.len())..(start + s.tri_count as usize * 3).min(w.indices.len())];
        let mut seen = std::collections::HashSet::new();
        for v in idx.iter().filter(|&&i| seen.insert(i)).filter_map(|&i| w.vertices.get(first + i as usize)) {
            let [u, vv] = v.lmap_coord;
            st.verts += 1;
            if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&vv) {
                st.outside += 1;
            }
            for k in 0..2 {
                st.uv_min[k] = st.uv_min[k].min(v.lmap_coord[k]);
                st.uv_max[k] = st.uv_max[k].max(v.lmap_coord[k]);
            }
            let a = texel(s.lightmap_index as usize, u, vv * 0.5).unwrap_or([0.0; 3]);
            let b = texel(s.lightmap_index as usize, u, vv * 0.5 + 0.5).unwrap_or([0.0; 3]);
            let lum = (a.iter().sum::<f32>() + b.iter().sum::<f32>()) / 6.0;
            st.sum += lum;
            if lum < 0.02 {
                st.black += 1;
            }
            let c = iw3::unpack::color(v.color);
            for k in 0..4 {
                st.colour[k] += c[k];
            }
        }
    }
    for (name, st) in &stats {
        let share = st.black as f32 / st.verts.max(1) as f32;
        if part.is_some() || (share > 0.5 && st.verts > 20) {
            let n = st.verts.max(1) as f32;
            println!(
                "{name}: {} verts, {:.0}% black, {} outside 0..1, mean {:.3}, pages {:?}, uv {:?}..{:?}, vertex colour {:?}",
                st.verts,
                share * 100.0,
                st.outside,
                st.sum / n,
                st.pages,
                st.uv_min,
                st.uv_max,
                st.colour.map(|c| (c / n * 100.0).round() / 100.0)
            );
        }
    }
    Ok(())
}
