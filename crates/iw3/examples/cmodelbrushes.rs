//! A clipMap brush model's brushes and their contents: cmodelbrushes <zone> <index>
use iw3::zone::{ParseOptions, Zone};

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&a[0]))?, ParseOptions::default())?;
    let n: usize = a[1].parse()?;
    let clip = zone.clip_map().expect("clipmap");
    println!("{} cmodels", clip.cmodels.len());
    let m = &clip.cmodels[n];
    println!("cmodel {n}: mins {:?} maxs {:?} leaf_brush_node {} aabbs {}+{}", m.mins, m.maxs, m.leaf_brush_node, m.first_coll_aabb, m.coll_aabb_count);
    let mut stack = vec![(m.leaf_brush_node, 0)];
    while let Some((i, depth)) = stack.pop() {
        let Some(node) = usize::try_from(i).ok().and_then(|i| clip.leaf_brush_nodes.get(i)) else { continue };
        if node.leaf_brush_count > 0 {
            for &b in &node.brushes {
                println!("  brush {b}: contents {:#x}", clip.brushes[b as usize].contents);
            }
        } else if depth < 64 {
            for off in node.child_offsets.iter().filter(|&&o| o > 0) {
                stack.push((i + *off as i32, depth + 1));
            }
        }
    }
    Ok(())
}
