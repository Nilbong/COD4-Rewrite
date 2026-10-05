//! A menu's items: menudump <zone> <menu>
fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("ui_mp".into());
    let name = a.next().unwrap_or_default();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    for m in iw3::menu::UiData::from_zone(&zone).menus.into_iter().filter(|m| m.window.name == name) {
        for it in &m.items {
            println!("item {:?} text {:?} textexp {:?} w {:?} visible {:?}", it.window.name, it.text, it.text_exp, it.rect_w_exp, it.visible_exp);
        }
    }
    Ok(())
}
