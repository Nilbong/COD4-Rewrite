//! Dump an xmodel's skeleton and surfaces: modelinfo <zone> <model>
use anyhow::Result;
use iw3::zone::{ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone_name = args.next().unwrap_or("mp_killhouse".into());
    let model = args.next().unwrap_or("body_mp_sas_urban_assault".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    if model == "list" {
        for a in &zone.assets {
            if let iw3::zone::Asset::XModel(x) = a {
                println!("{} bones {} surfs {}", x.name, x.num_bones, x.surfs.len());
            }
        }
        return Ok(());
    }
    let id = zone.find(&model).expect("model not found");
    let x = zone.xmodel(id).unwrap();
    println!("{}: bones {} (root {}), surfs {}, lods {:?}", x.name, x.num_bones, x.num_root_bones, x.surfs.len(), x.lods);
    for (i, name) in x.bone_names.iter().enumerate() {
        let parent = if i < x.num_root_bones as usize { -1 } else { i as i32 - x.parent_list[i - x.num_root_bones as usize] as i32 };
        let bm = x.base_mat.get(i);
        let local = if i >= x.num_root_bones as usize { Some((x.quats[i - x.num_root_bones as usize], x.trans[i - x.num_root_bones as usize])) } else { None };
        println!("  {i:3} {name:<24} parent {parent:3} base q {:?} t {:?} local {:?}", bm.map(|b| b.quat), bm.map(|b| b.trans), local);
    }
    for (i, s) in x.surfs.iter().enumerate() {
        let mat = x.materials.get(i).copied().flatten().and_then(|m| zone.material(m)).map(|m| m.name.clone());
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &s.verts { for k in 0..3 { lo[k] = lo[k].min(v.xyz[k]); hi[k] = hi[k].max(v.xyz[k]); } }
        println!("  surf {i}: bounds {lo:.1?}..{hi:.1?}");
        println!("  surf {i}: verts {} tris {} blend {:?} vertlists {:?} mat {:?}", s.verts.len(), s.tris.len(), s.blend_counts, s.vert_lists.iter().map(|v| (v.bone_offset, v.vert_count)).collect::<Vec<_>>(), mat);
    }
    Ok(())
}
