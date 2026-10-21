//! A single-player level's path nodes as its GameWorldSp holds them:
//! pathnodes <zone> [n]: the structure's fields, then the first n nodes.
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};
use iw3::zone::generic::{GNode, GVal};

fn show(n: &GNode, depth: usize, max: usize) {
    if depth > 6 {
        return;
    }
    for (name, v) in &n.fields {
        match v {
            GVal::Nodes(list) => {
                println!("{}{name}: {} x {}", "  ".repeat(depth), list.len(), list.first().map_or("", |x| x.ty.as_str()));
                for c in list.iter().take(max) {
                    show(c, depth + 1, max);
                }
            }
            GVal::Bytes(b) => println!("{}{name}: {} bytes", "  ".repeat(depth), b.len()),
            other => println!("{}{name}: {other:?}", "  ".repeat(depth)),
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("killhouse".into());
    let max: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    for asset in &zone.assets {
        if let Asset::Generic(g) = asset
            && g.ty == AssetType::GameWorldSp
        {
            println!("{} ({})", g.name, g.root.ty);
            show(&g.root, 1, max);
        }
    }
    Ok(())
}
