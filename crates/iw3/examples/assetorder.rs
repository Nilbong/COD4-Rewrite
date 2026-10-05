//! Print the run-length encoded top-level asset type sequence of a zone.
use anyhow::Result;
use iw3::zone::AssetType;

fn main() -> Result<()> {
    let name = std::env::args().nth(1).unwrap_or_else(|| "common_mp".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&name))?;
    let (types, _) = iw3::zone::asset_list(&data)?;
    let mut out: Vec<(String, usize)> = Vec::new();
    for t in types {
        let n = AssetType::from_u32(t).map(|t| format!("{t:?}")).unwrap_or(t.to_string());
        match out.last_mut() {
            Some((p, c)) if *p == n => *c += 1,
            _ => out.push((n, 1)),
        }
    }
    let mut counts = std::collections::BTreeMap::new();
    for (n, c) in &out {
        *counts.entry(n.clone()).or_insert(0) += c;
    }
    println!("{counts:?}");
    println!("{}", out.iter().map(|(n, c)| format!("{n}x{c}")).collect::<Vec<_>>().join(" "));
    Ok(())
}
