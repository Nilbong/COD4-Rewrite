//! Parse a Modern Warfare 2 zone and report how far it got:
//! `zonecheck <zone name or .ff path> [types]` (with `types`, a count per
//! asset type).
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let name = a.first().map(String::as_str).unwrap_or("mp_terminal");
    let path = if name.ends_with(".ff") { std::path::PathBuf::from(name) } else { iw4::Install::locate()?.zone_path(name) };
    let t = std::time::Instant::now();
    let data = iw4::fastfile::load(&path)?;
    println!("{}: {} bytes decompressed in {:.1}s", path.display(), data.len(), t.elapsed().as_secs_f32());
    let t = std::time::Instant::now();
    let z = iw4::zone::Zone::parse(&data, iw4::zone::ParseOptions::default())?;
    let s = &z.stats;
    println!(
        "parsed {}/{} top-level assets ({} loaded) in {:.1}s; unresolved {} aliases, {} strings; stopped at {:?}",
        s.top_level_parsed,
        s.top_level_total,
        z.assets.len(),
        t.elapsed().as_secs_f32(),
        s.unresolved_aliases,
        s.unresolved_strings,
        s.stopped_at
    );
    println!("block positions {:?}\nblock sizes     {:?}", s.block_positions, z.block_sizes);
    if let Some(ty) = a.get(1).and_then(|x| x.strip_prefix("dump=")) {
        for x in z.assets.iter().filter(|x| format!("{:?}", x.ty) == ty).take(1) {
            println!("{} {}", x.name, x.root.ty);
            dump(&x.root, 1, 3);
        }
    }
    if a.get(1).is_some_and(|x| x == "surfs") {
        for x in z.assets.iter().filter(|x| x.ty == iw4::zone::AssetType::GfxWorld) {
            let dpvs = x.root.node("dpvs").unwrap();
            for sfc in dpvs.nodes("surfaces").iter().take(5) {
                let raw = u32::from_le_bytes(sfc.data[16..20].try_into().unwrap());
                println!("surface material ptr {raw:#x} -> key {:#x}", raw.wrapping_sub(1));
            }
            for (i, mm) in x.root.nodes("materialMemory").iter().take(5).enumerate() {
                let raw = u32::from_le_bytes(mm.data[0..4].try_into().unwrap());
                println!("materialMemory[{i}] ptr {raw:#x} -> {:?}", mm.asset("material").map(|id| &z.assets[id].name));
            }
        }
    }
    if a.get(1).is_some_and(|x| x == "lmaps") {
        let show = |id: Option<usize>| {
            let Some(img) = id.map(|i| &z.assets[i]) else { return String::from("-") };
            let r = &img.root;
            let ld = r.node("texture").and_then(|t| t.node("loadDef"));
            format!(
                "{} {}x{} map {} sem {} ld {:?}",
                img.name,
                r.int("width"),
                r.int("height"),
                r.int("mapType"),
                r.int("semantic"),
                ld.map(|l| (l.int("levelCount"), l.int("flags"), format!("{:#x}", l.int("format")), l.int("resourceSize"), l.data.len()))
            )
        };
        for x in z.assets.iter().filter(|x| x.ty == iw4::zone::AssetType::GfxWorld) {
            let d = x.root.node("draw").unwrap();
            for lm in d.nodes("lightmaps") {
                println!("primary {}
  secondary {}", show(lm.asset("primary")), show(lm.asset("secondary")));
            }
            for p in d.assets("reflectionProbes").into_iter().take(2) {
                println!("probe {}", show(p));
            }
            println!("sky {}", show(x.root.nodes("skies").first().and_then(|s| s.asset("skyImage"))));
            println!("outdoor {}", show(x.root.asset("outdoorImage")));
        }
    }
    if a.get(1).is_some_and(|x| x == "top") {
        let mut counts = std::collections::BTreeMap::new();
        for (t, id) in &z.top_level {
            *counts.entry((*t, id.is_some())).or_insert(0) += 1;
        }
        for ((t, loaded), n) in counts {
            println!("  type {t:2} {:?} loaded {loaded}: {n}", iw4::zone::AssetType::from_u32(t));
        }
    }
    if let Some(ty) = a.get(1).and_then(|x| x.strip_prefix("names=")) {
        for x in z.assets.iter().filter(|x| format!("{:?}", x.ty) == ty) {
            println!("  {}", x.name);
        }
    }
    if let Some(name) = a.get(1).and_then(|x| x.strip_prefix("table=")) {
        for x in z.assets.iter().filter(|x| x.ty == iw4::zone::AssetType::StringTable && x.name == name) {
            let cols = x.root.int("columnCount") as usize;
            let cells: Vec<String> = x.root.nodes("values").iter().map(|c| c.string("string").unwrap_or("").to_owned()).collect();
            for row in cells.chunks(cols.max(1)) {
                println!("{}", row.join(","));
            }
        }
    }
    if let Some(name) = a.get(1).and_then(|x| x.strip_prefix("asset=")) {
        let depth: usize = a.get(2).and_then(|d| d.parse().ok()).unwrap_or(3);
        for x in z.assets.iter().filter(|x| x.name == name) {
            println!("{} {:?} {}", x.name, x.ty, x.root.ty);
            dump(&x.root, 1, depth);
        }
    }
    if a.get(1).is_some_and(|x| x == "types") {
        let mut counts = std::collections::BTreeMap::new();
        for x in &z.assets {
            *counts.entry(format!("{:?}", x.ty)).or_insert(0) += 1;
        }
        for (k, v) in counts {
            println!("  {k:20} {v}");
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn dump(n: &iw4::zone::GNode, depth: usize, max_depth: usize) {
    let pad = "  ".repeat(depth);
    for (name, v) in &n.fields {
        match v {
            iw4::zone::GVal::Nodes(ns) => {
                println!("{pad}{name}: {} x {}", ns.len(), ns.first().map_or("", |x| x.ty.as_str()));
                if depth < max_depth {
                    if let Some(c) = ns.first() {
                        dump(c, depth + 1, max_depth);
                    }
                }
            }
            iw4::zone::GVal::Bytes(b) => println!("{pad}{name}: {} bytes", b.len()),
            other => println!("{pad}{name}: {:?}", format!("{other:?}").chars().take(80).collect::<String>()),
        }
    }
}
