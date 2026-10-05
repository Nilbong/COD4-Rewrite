//! `cargo run --release -p t4 --example sounds -- [out dir]`: load World at
//! War's multiplayer sound aliases and the mixer, decode every sound file
//! they use and every music file, and report. With an output dir, also write
//! a few decoded samples there as plain WAVs.

use std::collections::{BTreeMap, BTreeSet};
use t4::sound::{Encoding, SoundFile};

/// Zones with multiplayer sounds: shared ones, then every map's.
const ZONES: [&str; 3] = ["code_post_gfx_mp", "common_mp", "ui_mp"];
/// Samples written to the output dir.
const SAMPLES: [&str; 6] = ["music_mainmenu", "mouse_click", "mouse_over", "weap_thompson_fire_plr", "MP_hit_alert", "mp_player_join"];

fn main() -> anyhow::Result<()> {
    let out = std::env::args().nth(1).map(std::path::PathBuf::from);
    let install = t4::Install::locate()?;
    let vfs = install.vfs()?;
    let t0 = std::time::Instant::now();
    let mut mixer = None;
    let mut aliases = BTreeMap::new();
    let maps: Vec<String> = std::fs::read_dir(install.zone_path("common_mp").parent().unwrap())?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".ff").map(str::to_owned))
        .filter(|n| n.starts_with("mp_") && !n.ends_with("_load"))
        .collect();
    for name in ZONES.iter().map(|z| z.to_string()).chain(maps) {
        let zone = t4::zone::Zone::parse(&t4::fastfile::load(&install.zone_path(&name))?, Default::default())?;
        if mixer.is_none() {
            mixer = t4::sound::Mixer::from_zone(&zone);
        }
        for a in t4::sound::aliases(&zone) {
            aliases.entry(a.name.to_ascii_lowercase()).or_insert(a);
        }
    }
    let mixer = mixer.expect("code_post_gfx_mp has the mixer");
    let variants: usize = aliases.values().map(|a| a.variants.len()).sum();
    println!("{} aliases, {variants} variations in {:.2?}", aliases.len(), t0.elapsed());
    let buses: Vec<&str> = mixer.buses.iter().map(|b| b.name.as_str()).filter(|n| !n.is_empty()).collect();
    println!("buses: {}", buses.join(" "));

    // Every loaded sound (by its data) and streamed file, once.
    let (mut loaded, mut streamed) = (BTreeMap::new(), BTreeSet::new());
    for v in aliases.values().flat_map(|a| &a.variants) {
        match &v.file {
            SoundFile::Loaded { name, data } => {
                let e = loaded.entry(name.clone()).or_insert(None);
                if e.is_none() {
                    *e = data.clone();
                }
            }
            SoundFile::Streamed(path) => {
                streamed.insert(path.clone());
            }
        }
    }
    let music = t4::sound::music_files(&vfs);
    let mut tally: BTreeMap<String, (usize, f64)> = BTreeMap::new();
    let mut problems = Vec::new();
    let mut check = |what: &str, name: &str, wav: Option<Vec<u8>>| {
        let Some(wav) = wav else {
            problems.push(format!("{what} {name}: not found"));
            return;
        };
        let enc = t4::sound::encoding(&wav);
        let key = format!("{what} {:?}", enc.unwrap_or(Encoding::Other(0)));
        if enc == Some(Encoding::Xwma) && std::env::var_os("T4_XWMA").is_some() {
            println!("xwma {what} {name} {} bytes", wav.len());
        }
        let e = tally.entry(key).or_default();
        e.0 += 1;
        match t4::sound::decode_wav(&wav) {
            Ok(pcm) => {
                let rate = u32::from_le_bytes(pcm[24..28].try_into().unwrap()) as f64;
                let channels = u16::from_le_bytes(pcm[22..24].try_into().unwrap()) as f64;
                e.1 += (pcm.len() - 44) as f64 / 2.0 / channels / rate;
            }
            Err(err) if enc != Some(Encoding::Xwma) => problems.push(format!("{what} {name}: {err}")),
            Err(_) => {}
        }
    };
    for (name, data) in &loaded {
        check("loaded", name, data.as_ref().map(|d| d.to_vec()));
    }
    for path in streamed.iter().filter(|p| !p.starts_with("sound/stream/music/")) {
        check("streamed", path, vfs.read(path).ok().flatten());
    }
    for path in &music {
        check("music", path, vfs.read(path).ok().flatten());
    }
    for (k, (n, secs)) in &tally {
        println!("{k:<26} {n:>6} files, {:>7.1} min decoded", secs / 60.0);
    }
    let music_aliases: Vec<&str> = aliases
        .values()
        .filter(|a| a.variants.iter().any(|v| mixer.buses.get(v.bus).is_some_and(|b| b.is_music)))
        .map(|a| a.name.as_str())
        .collect();
    println!("{} music files; aliases on the music bus: {}", music.len(), music_aliases.join(" "));
    for p in problems.iter().take(30) {
        println!("  problem: {p}");
    }
    println!("{} problems", problems.len());

    if let Some(out) = out {
        std::fs::create_dir_all(&out)?;
        for name in SAMPLES {
            let Some(v) = aliases.get(&name.to_ascii_lowercase()).and_then(|a| a.variants.first()) else {
                println!("no alias {name}");
                continue;
            };
            let wav = match &v.file {
                SoundFile::Loaded { name, .. } => loaded.get(name).cloned().flatten().map(|d| d.to_vec()),
                SoundFile::Streamed(path) => vfs.read(path).ok().flatten(),
            };
            if let Some(pcm) = wav.and_then(|w| t4::sound::decode_wav(&w).ok()) {
                let bus = mixer.buses.get(v.bus).map_or("?", |b| b.name.as_str());
                println!("wrote {name}.wav ({:.1}s, bus {bus}, {})", (pcm.len() - 44) as f64 / 2.0 / u16::from_le_bytes(pcm[22..24].try_into().unwrap()) as f64 / u32::from_le_bytes(pcm[24..28].try_into().unwrap()) as f64, if v.spatial { "3D" } else { "2D" });
                std::fs::write(out.join(format!("{name}.wav")), pcm)?;
            }
        }
    }
    Ok(())
}
