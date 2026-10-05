//! `cargo run --release -p t4 --example ui`: load World at War's multiplayer
//! menus, fonts and strings as `iw3::menu` types, summarise them, and check
//! that every material they draw has a decodable image.

use std::collections::{BTreeMap, HashSet};

fn main() -> anyhow::Result<()> {
    let install = t4::Install::locate()?;
    let vfs = install.vfs()?;
    let t0 = std::time::Instant::now();
    let mut menus = BTreeMap::new();
    let (mut fonts, mut strings, mut tables) = (Vec::new(), BTreeMap::new(), Vec::new());
    let mut images = BTreeMap::new();
    for name in t4::ui::MP_ZONES {
        let zone = t4::zone::Zone::parse(&t4::fastfile::load(&install.zone_path(name))?, Default::default())?;
        let data = t4::ui::ui_data(&zone);
        println!("{name:<28} {:>4} menus {:>2} fonts {:>5} strings {:>3} tables", data.menus.len(), data.fonts.len(), data.strings.len(), data.tables.len());
        for m in data.menus {
            menus.insert(m.window.name.to_ascii_lowercase(), m);
        }
        fonts.extend(data.fonts);
        strings.extend(data.strings);
        tables.extend(data.tables);
        // Material -> colour image, from the converted zone.
        let converted = t4::convert::to_iw3(&zone);
        for a in &converted.assets {
            if let iw3::zone::Asset::Material(m) = a {
                let image = m.textures.iter().find(|t| t.semantic == iw3::zone::TextureSemantic::Color).or(m.textures.first());
                let image = image.and_then(|t| converted.image(t.image?));
                if let Some(img) = image {
                    images.entry(m.name.trim_start_matches(',').to_owned()).or_insert((img.name.clone(), img.load_def.is_some()));
                }
            }
        }
    }
    println!("\n{} menus, {} fonts, {} strings, {} tables in {:.2?}", menus.len(), fonts.len(), strings.len(), tables.len(), t0.elapsed());
    for f in &fonts {
        println!("  font {:<28} {:>3}px {:>3} glyphs  {}", f.name, f.pixel_height, f.glyphs.len(), f.material);
    }
    let main: Vec<&str> = menus.keys().map(String::as_str).filter(|n| n.starts_with("main")).collect();
    println!("main menus: {}", main.join(" "));
    for m in main.iter().filter_map(|n| menus.get(*n)) {
        let texts: Vec<String> = m.items.iter().filter(|i| !i.text.is_empty()).map(|i| strings.get(i.text.trim_start_matches('@')).cloned().unwrap_or(i.text.clone())).collect();
        println!("`{}`: {} items, background {:?}, texts {:?}", m.window.name, m.items.len(), m.window.background, texts);
        if std::env::var_os("T4_ITEMS").is_some() {
            for it in m.items.iter().take(40) {
                println!("  {:<24} ty {:>2} al {}/{} rect {:?} bg {:?} text {:?} exp {:?} action {:?} vis {:?}", it.window.name, it.ty, it.window.rect.horz_align, it.window.rect.vert_align, (it.window.rect.x, it.window.rect.y, it.window.rect.w, it.window.rect.h), it.window.background, it.text, it.text_exp, it.action, it.visible_exp);
            }
        }
    }

    // Every material the menus and fonts draw.
    let mut wanted = HashSet::new();
    for m in menus.values() {
        wanted.extend(m.window.background.clone());
        for it in &m.items {
            wanted.extend(it.window.background.clone());
        }
    }
    for f in &fonts {
        wanted.insert(f.material.clone());
    }
    let (mut ok, mut problems) = (0, Vec::new());
    for mat in &wanted {
        match images.get(mat) {
            Some((_, true)) => ok += 1,
            Some((img, false)) => match vfs.read(&format!("images/{}.iwi", img.trim_start_matches(','))).ok().flatten().map(|d| iw3::iwi::Iwi::parse(&d)) {
                Some(Ok(_)) => ok += 1,
                Some(Err(e)) => problems.push(format!("{mat}: {img}: {e}")),
                None => problems.push(format!("{mat}: image {img} not found")),
            },
            None => problems.push(format!("{mat}: no material")),
        }
    }
    println!("{ok}/{} menu and font materials decode", wanted.len());
    for p in problems.iter().take(30) {
        println!("  problem: {p}");
    }
    Ok(())
}
