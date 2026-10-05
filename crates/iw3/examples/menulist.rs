//! Menu names in a zone matching a filter: menulist <zone> [filter]
fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("ui_mp".into());
    let filter = a.next().unwrap_or_default();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    for m in iw3::menu::UiData::from_zone(&zone).menus {
        if m.window.name.contains(&filter) {
            println!("{}", m.window.name);
        }
    }
    Ok(())
}
