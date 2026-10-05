//! Print text around matches of a string in a zone's raw data: zonegrep <zone> <text>
fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (zone, needle) = (args.next().expect("zone"), args.next().expect("text"));
    let install = iw3::Install::locate()?;
    let data = iw3::fastfile::load(&install.zone_path(&zone))?;
    let n = needle.to_ascii_lowercase();
    let lower: Vec<u8> = data.iter().map(|b| b.to_ascii_lowercase()).collect();
    let mut i = 0;
    let mut shown = 0;
    while let Some(p) = lower[i..].windows(n.len()).position(|w| w == n.as_bytes()) {
        let at = i + p;
        let (a, b) = (at.saturating_sub(120), (at + 160).min(data.len()));
        let text: String = data[a..b].iter().map(|&c| if (32..127).contains(&c) { c as char } else { '|' }).collect();
        println!("@{at}: {text}\n");
        i = at + n.len();
        shown += 1;
        if shown >= 12 {
            break;
        }
    }
    Ok(())
}
