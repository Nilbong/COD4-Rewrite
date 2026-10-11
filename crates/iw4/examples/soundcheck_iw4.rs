//! Read MW2's multiplayer sound aliases and decode every loaded sound:
//! `soundcheck_iw4 <out dir> [zone ...]` (default zones
//! `localized_common_mp common_mp`). Writes a few gun sounds as `.wav`
//! files to the directory.
use iw4::sound::{self, SoundFile};
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(a.first().map(String::as_str).unwrap_or("mw2snd"));
    std::fs::create_dir_all(&out)?;
    let zones: Vec<&str> = if a.len() > 1 { a[1..].iter().map(String::as_str).collect() } else { vec!["localized_common_mp", "common_mp"] };
    let install = iw4::Install::locate()?;
    let load = |n: &str| -> anyhow::Result<iw4::zone::Zone> { iw4::zone::Zone::parse(&iw4::fastfile::load(&install.zone_path(n))?, Default::default()) };
    let chans = sound::channels(&load("code_post_gfx_mp")?);
    println!("channels: {}", chans.as_ref().map_or(0, Vec::len));
    let vfs = install.vfs()?;
    let mut all = Vec::new();
    for name in &zones {
        let z = load(name)?;
        let curves = sound::curves(&z);
        let al = sound::aliases(&z, chans.as_deref());
        let variants: usize = al.iter().map(|a| a.variants.len()).sum();
        let streamed: Vec<&str> = al.iter().flat_map(|a| &a.variants).filter_map(|v| if let SoundFile::Streamed(p) = &v.file { Some(p.as_str()) } else { None }).collect();
        let found = streamed.iter().filter(|p| vfs.contains(p)).count();
        let weap_streamed = al.iter().filter(|a| a.name.starts_with("weap_") && a.variants.iter().any(|v| matches!(v.file, SoundFile::Streamed(_)))).count();
        println!(
            "{name}: {} aliases, {variants} variants, {} streamed ({found} found in iwds; {weap_streamed} weap_ aliases streamed), curves {:?}",
            al.len(),
            streamed.len(),
            curves.values().map(|c| format!("{} {:?}", c.name, c.knots)).collect::<Vec<_>>()
        );
        if let Some(p) = streamed.iter().find(|p| !vfs.contains(p)) {
            println!("  e.g. missing streamed file {p}");
        }
        // Decode every loaded sound once.
        let mut per_format: BTreeMap<(u16, u16, u16, u32), (usize, usize)> = BTreeMap::new();
        let mut bad = 0;
        for (_, asset) in z.of_type(iw4::zone::AssetType::LoadedSound) {
            let Some(s) = sound::LoadedSound::from_asset(&asset.root) else { continue };
            let e = per_format.entry((s.format, s.bits, s.channels, s.rate)).or_default();
            e.0 += 1;
            match s.pcm() {
                Ok(pcm) if pcm.len() == s.frames as usize * s.channels as usize && pcm.iter().any(|&x| x != 0) => e.1 += 1,
                Ok(_) | Err(_) => bad += 1,
            }
        }
        for ((f, b, c, r), (n, ok)) in per_format {
            println!("  format {f:#x} {b}-bit {c}ch {r} Hz: {ok}/{n} decoded");
        }
        if bad > 0 {
            println!("  {bad} failed or silent");
        }
        all.extend(al);
    }
    for want in ["weap_m4carbine_fire_plr", "weap_m4carbine_fire_npc", "weap_mech_layer_l_plr", "weap_m4carbine_clipout_plr", "weap_m4carbine_clipin_plr", "weap_m4carbine_chamber_close_plr"] {
        let Some(alias) = all.iter().find(|x| x.name == want) else {
            println!("{want}: not found");
            continue;
        };
        for (i, v) in alias.variants.iter().enumerate() {
            let ch = chans.as_ref().and_then(|c| c.get(v.channel)).map_or("?", |c| c.name.as_str());
            let SoundFile::Loaded { name, sound: Some(s) } = &v.file else {
                println!("{want}[{i}]: {:?}", v.file);
                continue;
            };
            let pcm = s.pcm()?;
            let peak = pcm.iter().map(|x| x.unsigned_abs()).max().unwrap_or(0);
            let rms = (pcm.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / pcm.len().max(1) as f64).sqrt();
            let clipped = pcm.iter().filter(|x| x.unsigned_abs() >= 32767).count();
            println!(
                "{want}[{i}]: {name} {}ch {} Hz {:.3}s peak {peak} rms {rms:.0} clipped {clipped}; channel {ch} spatial {} loop {} vol {:?} pitch {:?} dist {:?} curve {:?} secondary {:?}",
                s.channels,
                s.rate,
                pcm.len() as f32 / s.channels as f32 / s.rate as f32,
                v.spatial,
                v.looping,
                v.volume,
                v.pitch,
                v.dist,
                v.curve,
                v.secondary
            );
            std::fs::write(out.join(format!("{want}_{i}.wav")), sound::decode(s)?)?;
        }
    }
    Ok(())
}
