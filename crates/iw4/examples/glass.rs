//! A MW2 map's breakable glass (FxWorld glass system): `glass <map>`.
fn main() -> anyhow::Result<()> {
    let map = std::env::args().nth(1).unwrap_or("mp_terminal".into());
    let install = iw4::Install::locate()?;
    let z = iw4::zone::Zone::parse(&iw4::fastfile::load(&install.zone_path(&map))?, Default::default())?;
    let fx = z.assets.iter().find(|a| a.ty == iw4::zone::AssetType::FxWorld).unwrap();
    let g = fx.root.node("glassSys").unwrap();
    let f = |b: &[u8], o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    println!("defs {} init pieces {} init geo {}", g.int("defCount"), g.int("initPieceCount"), g.int("initGeoDataCount"));
    for d in g.nodes("defs") {
        let mat = d.asset("material").map(|i| z.assets[i].name.clone());
        let sh = d.asset("materialShattered").map(|i| z.assets[i].name.clone());
        println!("def thick {} tex {:?} color {:08x} {:?} {:?} phys {:?}", f(&d.data, 0), (f(&d.data, 4), f(&d.data, 8), f(&d.data, 12), f(&d.data, 16)),
            u32::from_le_bytes(d.data[20..24].try_into().unwrap()), mat, sh, d.asset("physPreset").map(|i| z.assets[i].name.clone()));
    }
    let geo = g.bytes("initGeoData").to_vec();
    let geo: Vec<u8> = if geo.is_empty() { g.nodes("initGeoData").iter().flat_map(|n| n.data.clone()).collect() } else { geo };
    let mut at = 0usize;
    for (i, p) in g.nodes("initPieceStates").iter().enumerate().take(4) {
        let b = &p.data;
        let (vc, fan) = (b[49] as usize, b[50] as usize);
        let verts: Vec<(i16, i16)> = (0..vc).map(|k| { let o = (at + k) * 4; (i16::from_le_bytes([geo[o], geo[o+1]]), i16::from_le_bytes([geo[o+2], geo[o+3]])) }).collect();
        println!("piece {i}: quat {:?} origin {:?} radius {} tc {:?} support {:#x} area {} def {} verts {} fans {} -> {:?}",
            (f(b,0),f(b,4),f(b,8),f(b,12)), (f(b,16),f(b,20),f(b,24)), f(b,28), (f(b,32),f(b,36)), u32::from_le_bytes(b[40..44].try_into().unwrap()), f(b,44), b[48], vc, fan, verts);
        at += vc + fan;
    }
    let (_, panes) = iw4::load_map_with_glass(&install, &map)?;
    if let Some(at) = std::env::args().nth(2) {
        let p: Vec<f32> = at.split(',').map(|v| v.parse().unwrap()).collect();
        for (i, g) in panes.iter().enumerate() {
            let c: Vec<f32> = (0..3).map(|k| g.corners.iter().map(|v| v[k]).sum::<f32>() / g.corners.len() as f32).collect();
            if (0..3).all(|k| (c[k] - p[k]).abs() < 120.0) { println!("pane {i} centre {c:?} corners {:?} normal {:?}", g.corners, g.normal); }
        }
    }
    println!("geo used by all: {}", g.nodes("initPieceStates").iter().map(|p| p.data[49] as usize + p.data[50] as usize).sum::<usize>());
    Ok(())
}
