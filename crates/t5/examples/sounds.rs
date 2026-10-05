//! `cargo run --release -p t5 --example sounds -- <out dir> [weapon|alias ...] [--all]`:
//! load Black Ops' multiplayer sound banks, then for each weapon (`ak47`,
//! ...; default: a few assault rifles and SMGs) list its fire, dry fire and
//! raise aliases (with the secondary aliases they bring in) and the sound
//! notetracks of its reload animations, and write every variant's audio to
//! `<out dir>/<alias>_<n>.wav`. Arguments
//! that are not weapons are taken as alias names. `--all` decodes every
//! sound in the banks and reports what fails.

use std::collections::{BTreeMap, HashSet};
use t5::sound::{MP_ZONES, SoundBank, Source};
use t5::zone::{AssetType, ParseOptions, Zone};

/// Weapon file fields naming sound aliases.
const SOUND_FIELDS: [&str; 6] = [
    "fireSound",
    "fireSoundPlayer",
    "emptyFireSound",
    "emptyFireSoundPlayer",
    "raiseSoundPlayer",
    "firstRaiseSoundPlayer",
];

/// Viewmodel animations whose notetracks play sounds.
const ANIM_FIELDS: [&str; 3] = ["reloadAnim", "reloadEmptyAnim", "rechamberAnim"];

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let all = args.iter().any(|a| a == "--all");
    let mut rest = args.iter().filter(|a| *a != "--all");
    let out = std::path::PathBuf::from(rest.next().map_or("bo1snd", String::as_str));
    let mut names: Vec<&str> = rest.map(String::as_str).collect();
    if names.is_empty() {
        names = vec!["ak47", "famas", "galil", "mp5k", "commando"];
    }
    std::fs::create_dir_all(&out)?;

    let install = t5::Install::locate()?;
    let t0 = std::time::Instant::now();
    let vfs = std::sync::Arc::new(install.vfs()?);
    let mut bank = SoundBank::new(Some(vfs.clone()));
    let mut common = None;
    for name in MP_ZONES {
        let zone = Zone::parse(&t5::fastfile::load(&install.zone_path(name))?, ParseOptions::default())?;
        bank.add_zone(&zone);
        if name == "common_mp" {
            common = Some(zone);
        }
    }
    let common = common.unwrap();
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    for s in &bank.sounds {
        let key = match &s.source {
            Source::Loaded { header, .. } => format!("loaded {:?} {}ch", header.format, header.channels),
            Source::Streamed => "streamed".into(),
        };
        *formats.entry(key).or_default() += 1;
    }
    let variants: usize = bank.alias_names().map(|n| bank.variants(n).map_or(0, |v| v.len())).sum();
    println!(
        "{} aliases ({variants} variants), {} sound files, {} unresolved, loaded in {:.2?}",
        bank.alias_names().count(),
        bank.sounds.len(),
        bank.unresolved,
        t0.elapsed()
    );
    for (k, n) in &formats {
        println!("  {k}: {n}");
    }

    let weapons: BTreeMap<String, t5::weapons::WeaponFile> =
        t5::weapons::mp_weapons(&vfs).into_iter().map(|w| (w.name.clone(), w)).collect();
    let mut seen = HashSet::new();
    for name in names {
        let Some(w) = weapons.get(&format!("{name}_mp")) else {
            dump(&bank, name, &out, &mut seen);
            continue;
        };
        println!("\n== {}", w.name);
        for field in SOUND_FIELDS {
            let alias = w.get(field);
            if !alias.is_empty() {
                println!("{field}:");
                dump(&bank, alias, &out, &mut seen);
            }
        }
        let mut anim_aliases = std::collections::BTreeSet::new();
        for field in ANIM_FIELDS {
            let Some(anim) = w.anim(field) else { continue };
            let Some(id) = common.find(AssetType::XAnimParts, anim) else { continue };
            let notes: Vec<String> = common.assets[id]
                .root
                .nodes("notify")
                .iter()
                .map(|n| {
                    let note = common.script_strings.get(n.int("name") as usize).map_or("?", String::as_str);
                    // `sndnt#<alias>` plays an alias; `rmbnt#` is rumble.
                    let mut known = "";
                    if let Some(alias) = note.strip_prefix("sndnt#") {
                        anim_aliases.insert(alias);
                        if bank.variants(alias).is_none() {
                            known = " (no alias)";
                        }
                    }
                    format!("{note}@{:.2}{known}", n.float("time"))
                })
                .collect();
            println!("{field} {anim}: {}", notes.join(", "));
        }
        for alias in anim_aliases {
            if !seen.contains(alias) {
                println!("{alias}:");
                dump(&bank, alias, &out, &mut seen);
            }
        }
    }

    if all {
        let t0 = std::time::Instant::now();
        let mut results: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        let mut errors = Vec::new();
        for s in &bank.sounds {
            let format = bank.open(s).map_or("unreadable".to_owned(), |(h, _)| format!("{:?}", h.format));
            let e = results.entry(format).or_default();
            match bank.decode(s) {
                Ok(_) => e.0 += 1,
                Err(err) => {
                    e.1 += 1;
                    errors.push(format!("{err:#}"));
                }
            }
        }
        println!("\ndecoded every sound in {:.2?} (format: ok, failed): {results:?}", t0.elapsed());
        for e in errors.iter().take(10) {
            println!("  {e}");
        }
    }
    Ok(())
}

/// Print an alias's variants and write their audio, then the same for the
/// aliases that play along with it.
fn dump(bank: &SoundBank, name: &str, out: &std::path::Path, seen: &mut HashSet<String>) {
    if !seen.insert(name.to_owned()) {
        return;
    }
    let Some(variants) = bank.alias(name) else {
        println!("  {name}: no such alias");
        return;
    };
    for (i, v) in variants.iter().enumerate() {
        let a = v.alias;
        print!(
            "  {name}[{i}] vol {:.2}..{:.2} pitch {:.2}..{:.2} dist {}..{} {}{} flags {:#010x} then {}: {}",
            a.volume[0],
            a.volume[1],
            a.pitch[0],
            a.pitch[1],
            a.distance[0],
            a.distance[1],
            if a.spatialized() { "3d" } else { "2d" },
            if a.looping() { " loop" } else { "" },
            a.flags,
            a.secondary.as_deref().unwrap_or("-"),
            v.sound.map_or("(silent)", |s| s.name.as_str()),
        );
        match &v.audio {
            Some(Ok(pcm)) => {
                let path = out.join(format!("{name}_{i}.wav"));
                let peak = pcm.samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                println!(" -> {} Hz {}ch {:.3}s peak {peak}", pcm.sample_rate, pcm.channels, pcm.seconds());
                if let Err(e) = std::fs::write(&path, pcm.to_wav()) {
                    println!("    writing {}: {e}", path.display());
                }
            }
            Some(Err(e)) => println!(" -> {e:#}"),
            None => println!(),
        }
    }
    // The layers that play along (a gunshot's tail, distant report, shell).
    let mut secondaries: Vec<&str> = variants.iter().filter_map(|v| v.alias.secondary.as_deref()).collect();
    secondaries.dedup();
    for s in secondaries {
        if !seen.contains(s) {
            println!("  {name} then:");
            dump(bank, s, out, seen);
        }
    }
}
