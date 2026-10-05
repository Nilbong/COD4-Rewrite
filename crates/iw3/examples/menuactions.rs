//! A menu's items with their scripts, dvars and list boxes:
//! menuactions <zone> <menu>
fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("ui_mp".into());
    let name = a.next().unwrap_or_default();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    for m in iw3::menu::UiData::from_zone(&zone).menus.into_iter().filter(|m| m.window.name == name) {
        println!("menu {} onOpen {:?} onClose {:?} onEsc {:?}", m.window.name, m.on_open, m.on_close, m.on_esc);
        for (i, it) in m.items.iter().enumerate() {
            println!(
                "{i}: {:?} type {} ownerdraw {:?} rect {:?} feeder {} text {:?} dvar {:?} action {:?} accept {:?} focus {:?} special {}",
                it.window.name,
                it.ty,
                (it.window.owner_draw, it.window.owner_draw_flags, it.window.dynamic_flags, it.window.static_flags),
                it.window.rect,
                it.special,
                it.text,
                it.dvar,
                it.action,
                it.on_accept,
                it.on_focus,
                it.special
            );
            if !it.dvar_test.is_empty() {
                println!("   dvarTest {:?} flags {} enable {:?}", it.dvar_test, it.dvar_flags, it.enable_dvar);
            }
            match &it.data {
                iw3::menu::ItemData::ListBox(l) => println!("   listbox {l:?}"),
                iw3::menu::ItemData::Multi(mu) => println!("   multi {:?} {:?} {:?}", mu.labels, mu.strings, mu.values),
                _ => {}
            }
        }
    }
    Ok(())
}
