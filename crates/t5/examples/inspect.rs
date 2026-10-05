//! `cargo run --release -p t5 --example inspect -- <zone> <model>`: bones,
//! surfaces and materials of one model (after conversion).
fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let (zone_name, model) = (a.next().unwrap_or("common_mp".into()), a.next().unwrap_or("t5_weapon_ak47_viewmodel".into()));
    let install = t5::Install::locate()?;
    let z = t5::load_iw3(&install, &zone_name)?;
    let xm = z.xmodel(z.find(&model).expect("model")).unwrap();
    println!("{model}: {} bones ({} root), {} surfs, lods {:?}", xm.num_bones, xm.num_root_bones, xm.surfs.len(), xm.lods);
    println!("parent_list {:?}", xm.parent_list);
    for (i, n) in xm.bone_names.iter().enumerate() {
        let b = &xm.base_mat[i];
        let t = xm.trans.get(i.wrapping_sub(xm.num_root_bones as usize)).copied();
        println!("  bone {i:3} {n:<28} base t {:?} q {:?} local t {:?}", b.trans.map(|v| (v * 10.0).round() / 10.0), b.quat.map(|v| (v * 100.0).round() / 100.0), t.map(|t| t.map(|v| (v * 10.0).round() / 10.0)));
    }
    for (si, s) in xm.surfs.iter().enumerate() {
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &s.verts {
            for k in 0..3 {
                lo[k] = lo[k].min(v.xyz[k]);
                hi[k] = hi[k].max(v.xyz[k]);
            }
        }
        let lists: Vec<(String, u16, u16)> = s.vert_lists.iter().map(|l| (xm.bone_names.get(l.bone_offset as usize / 64).cloned().unwrap_or(format!("?{}", l.bone_offset)), l.vert_count, l.tri_count)).collect();
        let mat = xm.materials.get(si).copied().flatten().and_then(|m| z.material(m));
        let tex: Vec<String> = mat.map(|m| m.textures.iter().map(|t| format!("{:?}={}", t.semantic, t.image.and_then(|i| z.image(i)).map(|i| i.name.clone()).unwrap_or_default())).collect()).unwrap_or_default();
        println!("surf {si}: {} verts {} tris blend {:?} lo {:?} hi {:?} lists {:?} mat {} {:?}", s.verts.len(), s.tris.len(), s.blend_counts, lo.map(|v| v.round()), hi.map(|v| v.round()), lists, mat.map_or("", |m| m.name.as_str()), tex);
    }
    Ok(())
}
