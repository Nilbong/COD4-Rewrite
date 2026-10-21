//! Which Modern Warfare 2 zones the game finds, and whether each parses:
//! `mw2zones [MW2 folder] [--quick]`. Without a folder it looks where the
//! game does. `--quick` stops after the zones' headers (no full parse).
//! The report is printed and written to `mw2zones.txt` beside the program.

use std::fmt::Write as _;

const ZONES: &[&str] = &["common_mp", "code_post_gfx_mp", "localized_code_post_gfx_mp", "localized_common_mp", "mp_rust"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let quick = args.iter().any(|a| a == "--quick");
    let folder = args.iter().find(|a| !a.starts_with("--"));
    let mut out = String::new();
    report(&mut out, folder.map(String::as_str), quick);
    print!("{out}");
    let file = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("mw2zones.txt"))).unwrap_or_else(|| "mw2zones.txt".into());
    match std::fs::write(&file, &out) {
        Ok(()) => println!("\nreport written to {}", file.display()),
        Err(e) => println!("\ncould not write {}: {e}", file.display()),
    }
    if std::env::var_os("MW2ZONES_NO_PAUSE").is_none() {
        println!("press Enter to close");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
}

fn report(out: &mut String, folder: Option<&str>, quick: bool) {
    let _ = writeln!(out, "mw2zones {}", env!("CARGO_PKG_VERSION"));
    let install = match folder {
        Some(f) => iw4::Install::at(f),
        None => iw4::Install::locate(),
    };
    let install = match install {
        Ok(i) => i,
        Err(e) => {
            let _ = writeln!(out, "no Modern Warfare 2 install: {e:#}");
            return;
        }
    };
    let _ = writeln!(out, "install: {}", install.root.display());
    let _ = writeln!(out, "localization.txt language: {}", install.language().as_deref().unwrap_or("(none or no such zone folder)"));
    let _ = writeln!(out, "zone folders:");
    for (dir, language) in install.zone_dirs() {
        let count = std::fs::read_dir(&dir).map(|r| r.filter_map(Result::ok).filter(|e| e.path().extension().is_some_and(|x| x == "ff")).count()).unwrap_or(0);
        let _ = writeln!(out, "  {} ({} .ff, {})", dir.display(), count, if language { "language" } else { "NOT a language folder" });
    }
    let _ = writeln!(out, "other zone files outside language folders:");
    for (dir, language) in install.zone_dirs() {
        if language {
            continue;
        }
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if e.path().extension().is_some_and(|x| x == "ff") {
                let _ = writeln!(out, "  {} ({} bytes)", e.path().display(), e.metadata().map_or(0, |m| m.len()));
            }
        }
    }
    let markers: Vec<String> = ["iw4x.exe", "iw4x.dll", "iw4x", "zone/dlc", "zone/patch", "mods", "zonebuilder"]
        .iter()
        .filter(|m| install.root.join(m).exists())
        .map(|m| m.to_string())
        .collect();
    let _ = writeln!(out, "IW4x / mod markers present: {}", if markers.is_empty() { "none".into() } else { markers.join(", ") });
    for name in ZONES {
        let candidates = install.zone_candidates(name);
        let _ = writeln!(out, "\n{name}: {} cop{}", candidates.len(), if candidates.len() == 1 { "y" } else { "ies" });
        let mut chosen = false;
        for (i, path) in candidates.iter().enumerate() {
            let _ = writeln!(out, "  [{i}] {}", path.display());
            let ok = check(out, path, quick);
            if ok && !chosen {
                let _ = writeln!(out, "      -> the game uses this copy");
                chosen = true;
            }
        }
        if !chosen {
            let _ = writeln!(out, "  NO USABLE COPY");
        }
    }
}

/// Reports one zone file; whether it parsed.
fn check(out: &mut String, path: &std::path::Path, quick: bool) -> bool {
    let raw = match std::fs::read(path) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(out, "      cannot read: {e}");
            return false;
        }
    };
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let _ = writeln!(out, "      {} bytes, modified {modified} (unix), {}", raw.len(), iw4::fastfile::describe(&raw));
    let _ = writeln!(out, "      auth header: {}", auth_header(&raw));
    let data = match iw4::fastfile::decompress(&raw) {
        Ok(d) => d,
        Err(e) => {
            let _ = writeln!(out, "      DECOMPRESS FAILED: {e:#}");
            return false;
        }
    };
    let _ = writeln!(out, "      {}", iw4::zone::header_summary(&data));
    for (i, row) in data[..data.len().min(0x80)].chunks(16).enumerate() {
        let hex: Vec<String> = row.iter().map(|b| format!("{b:02x}")).collect();
        let _ = writeln!(out, "      {:04x}: {}", i * 16, hex.join(" "));
    }
    if quick {
        return true;
    }
    let started = std::time::Instant::now();
    match iw4::zone::Zone::parse(&data, Default::default()) {
        Ok(z) => {
            let s = &z.stats;
            let _ = writeln!(
                out,
                "      parsed {}/{} assets in {:.1}s{}",
                s.top_level_parsed,
                s.top_level_total,
                started.elapsed().as_secs_f32(),
                s.stopped_at.as_ref().map(|w| format!(", stopped at {w}")).unwrap_or_default()
            );
            true
        }
        Err(e) => {
            let _ = writeln!(out, "      PARSE FAILED: {e:#}");
            false
        }
    }
}

/// The signed header's creation time and the zone name it carries.
fn auth_header(raw: &[u8]) -> String {
    let word = |o: usize| raw.get(o..o + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
    // FILETIME, high word first: 100 ns ticks since 1601.
    let ticks = (u64::from(word(0x0d)) << 32) | u64::from(word(0x11));
    let days = (ticks / 10_000_000).saturating_sub(11_644_473_600) / 86_400;
    let (y, m, d) = civil(days as i64);
    let name: String = raw.get(0x141..0x161).unwrap_or(&[]).iter().take_while(|&&c| c != 0).map(|&c| if c.is_ascii_graphic() { c as char } else { '?' }).collect();
    format!("created {y:04}-{m:02}-{d:02}, name {name:?}")
}

/// Days since 1970-01-01 to a calendar date.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}
