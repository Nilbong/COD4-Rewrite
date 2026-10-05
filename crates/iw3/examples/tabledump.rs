//! Rows of a string table matching a filter: tabledump <zone> <table> [filter]
fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let zone_name = a.next().unwrap_or("ui_mp".into());
    let table = a.next().unwrap_or("mp/statstable.csv".into());
    let filter = a.next().unwrap_or_default();
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = iw3::zone::Zone::parse(&data, iw3::zone::ParseOptions::default())?;
    for t in iw3::menu::UiData::from_zone(&zone).tables {
        if !t.name.eq_ignore_ascii_case(&table) {
            continue;
        }
        println!("{}: {} rows x {} columns", t.name, t.rows, t.columns);
        for r in 0..t.rows {
            let row: Vec<&str> = (0..t.columns).map(|c| t.get(r, c).unwrap_or("")).collect();
            if filter.is_empty() || row.iter().any(|c| c.contains(&filter)) {
                println!("{r}: {}", row.join(" | "));
            }
        }
    }
    Ok(())
}
