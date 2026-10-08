//! A menu's items: menudump <zone> <menu>
fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("ui_mp".into());
    let name = a.next().unwrap_or_default();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    for m in iw3::menu::UiData::from_zone(&zone).menus.into_iter().filter(|m| m.window.name == name) {
        let full = std::env::var_os("MENUDUMP_FULL").is_some();
        if full {
            println!("menu {} open {:?} close {:?} esc {:?}", m.window.name, m.on_open, m.on_close, m.on_esc);
        }
        for (i, it) in m.items.iter().enumerate() {
            if full {
                let r = it.window.rect;
                println!(
                    "{i:3} ty {:2} rect ({:.0},{:.0} {:.0}x{:.0} a{}/{}) style {} flags {:#x} text {:?}/{:?} dvar {:?} vis {:?} action {:?} enter {:?} focus {:?} data {:?}",
                    it.ty, r.x, r.y, r.w, r.h, r.horz_align, r.vert_align, it.window.style, it.window.static_flags, it.text, it.text_exp, it.dvar,
                    it.visible_exp, it.action, it.mouse_enter, it.on_focus, it.data
                );
            } else {
                println!("item {:?} text {:?} textexp {:?} w {:?} visible {:?}", it.window.name, it.text, it.text_exp, it.rect_w_exp, it.visible_exp);
            }
        }
    }
    Ok(())
}
