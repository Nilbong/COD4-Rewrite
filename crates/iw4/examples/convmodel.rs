//! A converted model's LODs, surfaces and materials: `convmodel <map> <model>`.
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let z = iw4::load_map(&iw4::Install::locate()?, &a[0])?;
    for m in z.assets.iter().filter_map(|x| match x { iw3::zone::Asset::XModel(m) if m.name == a[1] => Some(m), _ => None }) {
        println!("{} lods {:?} bones {} root {} names {:?} mins {:?} maxs {:?}", m.name, m.lods, m.num_bones, m.num_root_bones, m.bone_names, m.mins, m.maxs);
        for (i, b) in m.base_mat.iter().enumerate() { println!("  bone {i} quat {:?} trans {:?}", b.quat, b.trans); }
        for (i, s) in m.surfs.iter().enumerate() {
            let mat = m.materials.get(i).copied().flatten().and_then(|id| z.material(id));
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in &s.verts { for k in 0..3 { lo[k] = lo[k].min(v.xyz[k]); hi[k] = hi[k].max(v.xyz[k]); } }
            println!("     vert box {lo:?} {hi:?} vertlists {:?}", s.vert_lists.iter().map(|l| (l.bone_offset, l.vert_count)).collect::<Vec<_>>());
            println!("  surf {i}: {} verts {} tris {:?} [{}] bits {:x?}", s.verts.len(), s.tris.len(), mat.map(|m| &m.name),
                mat.and_then(|m| m.technique_set).and_then(|t| z.technique_set(t)).map_or("", |t| t.name.as_str()), mat.and_then(|m| m.state_bits_for(7)));
        }
    }
    Ok(())
}
