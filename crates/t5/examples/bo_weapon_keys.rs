//! Print chosen keys of Black Ops's multiplayer weapon files.
//!
//! `cargo run --release -p t5 --example bo_weapon_keys -- <key,key,...> [name filter]`
//! A key ending in `*` prints every key with that prefix.

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let keys: Vec<String> = args.next().unwrap_or_default().split(',').map(str::to_owned).collect();
    let filter = args.next().unwrap_or_default().to_ascii_lowercase();
    let vfs = t5::Install::locate()?.vfs()?;
    let mut weapons = t5::weapons::mp_weapons(&vfs);
    weapons.sort_by(|a, b| a.name.cmp(&b.name));
    for w in weapons.iter().filter(|w| w.name.to_ascii_lowercase().contains(&filter)) {
        let mut out = Vec::new();
        for k in &keys {
            match k.strip_suffix('*') {
                Some(prefix) => {
                    for key in w.keys().filter(|key| key.starts_with(prefix)) {
                        out.push(format!("{key}={}", w.get(key)));
                    }
                }
                None => {
                    let v = w.get(k);
                    if !v.is_empty() {
                        out.push(format!("{k}={v}"));
                    }
                }
            }
        }
        if !out.is_empty() {
            println!("{:<28} {}", w.name, out.join("  "));
        }
    }
    Ok(())
}
