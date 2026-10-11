//! Collision triangles (terrain and patches) of a converted MW2 map near a
//! point: `trisat <map> x y z r`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let z = iw4::load_map(&iw4::Install::locate()?, &a[0])?;
    let p: Vec<f32> = a[1..4].iter().map(|v| v.parse().unwrap()).collect();
    let r: f32 = a[4].parse()?;
    let c = z.clip_map().unwrap();
    let mut contents = vec![-1i32; c.tri_indices.len() / 3];
    for node in c.aabb_trees.iter().filter(|n| n.child_count == 0) {
        let Some(p) = c.partitions.get(node.index as usize) else { continue };
        let m = c.materials.get(node.material_index as usize).map_or(-2, |m| m.content_flags);
        for t in p.first_tri as usize..p.first_tri as usize + p.tri_count as usize { if t < contents.len() { contents[t] = m; } }
    }
    let mut kinds = std::collections::BTreeMap::new();
    for x in &contents { *kinds.entry(format!("{x:#x}")).or_insert(0) += 1; }
    println!("all tris by contents: {kinds:?}");
    let mut n = 0;
    for t in c.tri_indices.chunks_exact(3) {
        let v: Vec<[f32; 3]> = t.iter().map(|&i| c.verts[i as usize]).collect();
        let mid: Vec<f32> = (0..3).map(|k| (v[0][k] + v[1][k] + v[2][k]) / 3.0).collect();
        if (0..3).all(|k| (mid[k] - p[k]).abs() < r) {
            n += 1;
            if n <= 12 { println!("{:#x} {v:?}", contents[(t.as_ptr() as usize - c.tri_indices.as_ptr() as usize) / 6]); }
        }
    }
    println!("{n} triangles; {} partitions, first {:?}", c.partitions.len(), c.partitions.first().map(|p| (p.tri_count, p.first_tri)));
    Ok(())
}
