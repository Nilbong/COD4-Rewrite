//! The materials a menu and its items draw: menumats <zone> <menu>
fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("ui_mp".into());
    let name = a.next().unwrap_or_default();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    for m in iw3::menu::UiData::from_zone(&zone).menus.into_iter().filter(|m| m.window.name == name) {
        println!("menu {} background {:?} rect {:?}", m.window.name, m.window.background, m.window.rect);
        for (i, it) in m.items.iter().enumerate() {
            if it.window.background.is_some() {
                println!("  {i} {:?} rect {:?}", it.window.background, it.window.rect);
            }
        }
    }
    Ok(())
}
