//! Models out of a zone, converted on their own with all they use:
//! `onemodel <zone> <model>...`.
use iw4::zone::{Asset, AssetType, GNode, Zone};
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let install = iw4::Install::locate()?;
    let t = std::time::Instant::now();
    let src = Zone::parse(&iw4::fastfile::load(&install.zone_path(&a[0]))?, Default::default())?;
    println!("parsed in {:?}, {} assets", t.elapsed(), src.assets.len());
    let stand_in = |n: &String| Asset { ty: AssetType::XModel, name: format!(",{n}"), root: GNode { ty: String::new(), data: Vec::new(), fields: Vec::new(), locs: Vec::new() } };
    let want = Zone { script_strings: Vec::new(), assets: a[1..].iter().map(stand_in).collect(), top_level: Vec::new(), stats: Default::default(), block_sizes: [0; 8] };
    let c = iw4::convert::to_iw3(&want, Some(&src));
    println!("converted in {:?}", t.elapsed());
    for x in &c.assets {
        match x {
            iw3::zone::Asset::XModel(m) => println!("xmodel {} lods {} bones {:?} parents {:?} trans {:?} mins {:?} maxs {:?}", m.name, m.lods.len(), m.bone_names, m.parent_list, m.trans, m.mins, m.maxs),
            // (and its surfaces)

            other => println!("  {}", other.name()),
        }
        if let iw3::zone::Asset::XModel(m) = x {
            println!("  lods {:?} surfs {:?}", m.lods, m.surfs.iter().map(|s| (s.vert_count, s.tri_count, s.verts.len(), s.tris.len())).collect::<Vec<_>>());
        }
    }
    Ok(())
}
