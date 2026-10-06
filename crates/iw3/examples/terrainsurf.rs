//! Terrain (triangle) collision by surface type, from the clip map's AABB
//! tree leaves: terrainsurf <map>...
const SURFACES: [&str; 29] = [
    "default", "bark", "brick", "carpet", "cloth", "concrete", "dirt", "flesh", "foliage", "glass", "grass", "gravel",
    "ice", "metal", "mud", "paper", "plaster", "rock", "sand", "snow", "water", "wood", "asphalt", "ceramic", "plastic",
    "rubber", "cushion", "fruit", "paintedmetal",
];

fn main() -> anyhow::Result<()> {
    let install = iw3::Install::locate()?;
    for map in std::env::args().skip(1) {
        let zone = iw3::zone::Zone::parse(&iw3::fastfile::load(&install.zone_path(&map))?, iw3::zone::ParseOptions::default())?;
        let Some(clip) = zone.clip_map() else { continue };
        let mut tris = [0usize; 29];
        for t in clip.aabb_trees.iter().filter(|t| t.child_count == 0) {
            let Some(p) = usize::try_from(t.index).ok().and_then(|i| clip.partitions.get(i)) else { continue };
            let s = clip.materials.get(t.material_index as usize).map_or(0, |m| ((m.surface_flags >> 20) & 31) as usize);
            tris[s.min(28)] += p.tri_count as usize;
        }
        let total: usize = tris.iter().sum();
        let mut top: Vec<(usize, &str)> = tris.iter().zip(SURFACES).filter(|(n, _)| **n > 0).map(|(n, s)| (*n, s)).collect();
        top.sort_by(|a, b| b.0.cmp(&a.0));
        println!("{map:16} {total:6} tris: {}", top.iter().take(6).map(|(n, s)| format!("{s} {}%", n * 100 / total.max(1))).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}
