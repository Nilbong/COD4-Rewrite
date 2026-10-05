//! An xanim's delta (root motion) part as loaded: deltainfo <zone> <anim>
use iw3::zone::generic::{GNode, GVal};
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn dump(n: &GNode, depth: usize) {
    let pad = "  ".repeat(depth);
    println!("{pad}{} ({} bytes): {:02x?}", n.ty, n.data.len(), &n.data[..n.data.len().min(48)]);
    for (name, v) in &n.fields {
        match v {
            GVal::Nodes(ns) => {
                println!("{pad}  {name}: {} nodes", ns.len());
                for c in ns.iter().take(2) {
                    dump(c, depth + 2);
                }
            }
            GVal::Bytes(b) => println!("{pad}  {name}: {} bytes {:02x?}", b.len(), &b[..b.len().min(64)]),
            other => println!("{pad}  {name}: {other:?}"),
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("common_mp".into());
    let anim = a.next().unwrap_or("mp_mantle_up_57".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    for asset in &zone.assets {
        if let Asset::Generic(g) = asset {
            if g.ty == AssetType::XAnimParts && g.name == anim {
                println!("numframes {} framerate {} bDelta {}", g.root.int("numframes"), g.root.float("framerate"), g.root.int("bDelta"));
                if let Some(d) = g.root.node("deltaPart") {
                    dump(d, 0);
                }
                let x = iw3::xanim::XAnim::from_node(&g.name, &g.root, &zone.script_strings)?;
                if let iw3::xanim::TransTrack::Keys { frames, values } = &x.delta_trans {
                    for (f, v) in frames.iter().zip(values) {
                        println!("key {f}: {v:?}");
                    }
                }
                for f in [0.0, 0.25, 0.5, 0.75, 1.0] {
                    println!("delta at {f}: {:?}", x.delta_at(f));
                }
            }
        }
    }
    Ok(())
}
