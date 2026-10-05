//! Solid collision with nothing drawn there: invisible <map>...
//!
//! Lists the world's solid brushes (not brush entities, not player clip) whose
//! bounds no drawn world surface or static model overlaps, with any map
//! entity (`script_model` and the like) standing in them: props that block
//! players but that the game doesn't draw.
use std::collections::HashMap;

type Aabb = ([f32; 3], [f32; 3]);

fn overlaps(a: &Aabb, b: &Aabb, slack: f32) -> bool {
    (0..3).all(|k| a.0[k] - slack <= b.1[k] && b.0[k] <= a.1[k] + slack)
}

fn main() -> anyhow::Result<()> {
    let install = iw3::Install::locate()?;
    for map in std::env::args().skip(1) {
        let data = iw3::fastfile::load(&install.zone_path(&map))?;
        let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
        let (Some(clip), Some(gfx)) = (zone.clip_map(), zone.gfx_world()) else { continue };
        // Everything drawn, bucketed on a coarse grid.
        const CELL: f32 = 256.0;
        let cell = |v: f32| (v / CELL).floor() as i32;
        let mut grid: HashMap<(i32, i32, i32), Vec<usize>> = HashMap::new();
        let mut drawn: Vec<Aabb> = Vec::new();
        let mut add = |b: Aabb, grid: &mut HashMap<(i32, i32, i32), Vec<usize>>| {
            let i = drawn.len();
            drawn.push(b);
            for x in cell(b.0[0])..=cell(b.1[0]) {
                for y in cell(b.0[1])..=cell(b.1[1]) {
                    for z in cell(b.0[2])..=cell(b.1[2]) {
                        grid.entry((x, y, z)).or_default().push(i);
                    }
                }
            }
        };
        // Each drawn triangle (surfaces are batches spanning wide areas).
        for s in &gfx.surfaces {
            if s.material.is_none() {
                continue;
            }
            for t in 0..s.tri_count as usize {
                let mut b: Aabb = ([f32::MAX; 3], [f32::MIN; 3]);
                for c in 0..3 {
                    let Some(&i) = gfx.indices.get(s.base_index as usize + t * 3 + c) else { continue };
                    let Some(v) = gfx.vertices.get(s.first_vertex as usize + i as usize) else { continue };
                    for k in 0..3 {
                        b.0[k] = b.0[k].min(v.xyz[k]);
                        b.1[k] = b.1[k].max(v.xyz[k]);
                    }
                }
                if b.0[0] <= b.1[0] {
                    add(b, &mut grid);
                }
            }
        }
        for sm in &gfx.static_models {
            let Some(xm) = sm.model.and_then(|m| zone.xmodel(m)) else { continue };
            let r = xm.radius * sm.scale;
            add(([sm.origin[0] - r, sm.origin[1] - r, sm.origin[2] - r], [sm.origin[0] + r, sm.origin[1] + r, sm.origin[2] + r]), &mut grid);
        }
        let ents = iw3::ents::parse(&zone.map_ents().map(|e| e.entity_string.clone()).unwrap_or_default());
        let entity = clip.entity_brushes();
        let mut found: Vec<(f32, usize, String)> = Vec::new();
        for (i, b) in clip.brushes.iter().enumerate() {
            if entity.contains(&(i as u32)) || b.contents & 0x11 == 0 || b.contents & 0x1000000 != 0 {
                continue;
            }
            let bb: Aabb = (b.mins, b.maxs);
            let mut seen = false;
            'cells: for x in cell(b.mins[0])..=cell(b.maxs[0]) {
                for y in cell(b.mins[1])..=cell(b.maxs[1]) {
                    for z in cell(b.mins[2])..=cell(b.maxs[2]) {
                        if grid.get(&(x, y, z)).is_some_and(|v| v.iter().any(|&d| overlaps(&drawn[d], &bb, 1.0))) {
                            seen = true;
                            break 'cells;
                        }
                    }
                }
            }
            if seen {
                continue;
            }
            let size: Vec<f32> = (0..3).map(|k| b.maxs[k] - b.mins[k]).collect();
            let what: Vec<String> = ents
                .iter()
                .filter(|e| e.origin().is_some_and(|o| (0..3).all(|k| b.mins[k] - 48.0 <= o[k] && o[k] <= b.maxs[k] + 48.0)))
                .map(|e| format!("{} {}", e.classname(), e.get("model").unwrap_or("")))
                .collect();
            let mats: Vec<String> = b
                .side_materials
                .iter()
                .map(|&m| m as i64)
                .chain(b.axial_materials.iter().flatten().map(|&m| m as i64))
                .filter_map(|m| usize::try_from(m).ok().and_then(|m| clip.materials.get(m)).map(|m| m.name.clone()))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            found.push((
                size[0] * size[1] * size[2],
                i,
                format!(
                    "brush {i} contents {:#x} at ({:.0}, {:.0}, {:.0}) size {:.0}x{:.0}x{:.0} mats {mats:?} ents {what:?}",
                    b.contents,
                    (b.mins[0] + b.maxs[0]) / 2.0,
                    (b.mins[1] + b.maxs[1]) / 2.0,
                    b.mins[2],
                    size[0],
                    size[1],
                    size[2]
                ),
            ));
        }
        // Collision triangles (terrain, curved patches) with nothing drawn
        // near them, in 128-unit clumps.
        let mut clumps: HashMap<(i32, i32, i32), (usize, Aabb)> = HashMap::new();
        for t in clip.tri_indices.chunks_exact(3) {
            let Some(v) = t.iter().map(|&i| clip.verts.get(i as usize).copied()).collect::<Option<Vec<_>>>() else { continue };
            let mut b: Aabb = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &v {
                for k in 0..3 {
                    b.0[k] = b.0[k].min(p[k]);
                    b.1[k] = b.1[k].max(p[k]);
                }
            }
            let near = (cell(b.0[0])..=cell(b.1[0])).any(|x| {
                (cell(b.0[1])..=cell(b.1[1]))
                    .any(|y| (cell(b.0[2])..=cell(b.1[2])).any(|z| grid.get(&(x, y, z)).is_some_and(|l| l.iter().any(|&d| overlaps(&drawn[d], &b, 4.0)))))
            });
            if !near {
                let key = ((b.0[0] / 128.0) as i32, (b.0[1] / 128.0) as i32, (b.0[2] / 128.0) as i32);
                let e = clumps.entry(key).or_insert((0, b));
                e.0 += 1;
                for k in 0..3 {
                    e.1.0[k] = e.1.0[k].min(b.0[k]);
                    e.1.1[k] = e.1.1[k].max(b.1[k]);
                }
            }
        }
        let mut clumps: Vec<_> = clumps.into_values().collect();
        clumps.sort_by(|a, b| b.0.cmp(&a.0));
        println!("== {map}: {} collision triangles in {} clumps with nothing drawn", clumps.iter().map(|c| c.0).sum::<usize>(), clumps.len());
        for (n, b) in clumps.iter().take(10) {
            let what: Vec<String> = ents
                .iter()
                .filter(|e| e.origin().is_some_and(|o| (0..3).all(|k| b.0[k] - 64.0 <= o[k] && o[k] <= b.1[k] + 64.0)))
                .map(|e| format!("{} {}", e.classname(), e.get("model").unwrap_or("")))
                .collect();
            println!("  {n} tris at ({:.0}, {:.0}, {:.0}) to ({:.0}, {:.0}, {:.0}) ents {what:?}", b.0[0], b.0[1], b.0[2], b.1[0], b.1[1], b.1[2]);
        }
        found.sort_by(|a, b| b.0.total_cmp(&a.0));
        println!("== {map}: {} invisible solid brushes", found.len());
        for (_, _, line) in found.iter().take(25) {
            println!("  {line}");
        }
    }
    Ok(())
}
