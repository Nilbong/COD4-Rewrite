//! Convert MW2's `common_mp` animations to CoD4's form and decode them the
//! way the game does (`iw3::xanim::XAnim::from_node`): a few M4 viewmodel
//! anims in detail, then every one for a pass count.
//!
//! `animcheck_iw4 [anim ...]`

use iw3::xanim::{RotTrack, TransTrack, XAnim};
use iw3::zone::Asset;

fn main() -> anyhow::Result<()> {
    let install = iw4::Install::locate()?;
    let t = std::time::Instant::now();
    let parsed = iw4::zone::Zone::parse(&iw4::fastfile::load(&install.zone_path("common_mp"))?, iw4::zone::ParseOptions::default())?;
    let parse_time = t.elapsed();
    let t = std::time::Instant::now();
    let zone = iw4::convert::to_iw3_anims(&parsed);
    println!("parsed common_mp in {parse_time:.1?}, converted {} anims in {:.1?}", zone.assets.len(), t.elapsed());

    let args: Vec<String> = std::env::args().skip(1).collect();
    let detail: Vec<&str> = if args.is_empty() {
        vec!["viewmodel_m4_idle", "viewmodel_m4_reload", "viewmodel_m4_reload_empty", "viewmodel_m4_fire", "viewmodel_m4_sprint_loop"]
    } else {
        args.iter().map(|s| s.as_str()).collect()
    };

    // Root motion: MW2's 3D deltas (flag 4) lose their full rotation track.
    let delta_3d = parsed.assets.iter().filter(|a| a.ty == iw4::zone::AssetType::XAnimParts && a.root.data.get(16).is_some_and(|f| f & 4 != 0)).count();
    let (mut ok, mut failed, mut flagged) = (0, Vec::new(), Vec::new());
    let (mut delta, mut moving, mut looping) = (0, 0, 0);
    for a in &zone.assets {
        let Asset::Generic(g) = a else { continue };
        let x = match XAnim::from_node(&g.name, &g.root, &zone.script_strings) {
            Ok(x) => x,
            Err(e) => {
                failed.push(format!("{e:#}"));
                continue;
            }
        };
        ok += 1;
        delta += x.delta as usize;
        looping += x.looping as usize;
        moving += (x.delta_at(1.0).iter().map(|c| c * c).sum::<f32>() > 1.0) as usize;
        if let Some(why) = implausible(&x) {
            flagged.push(format!("{}: {why}", x.name));
        }
        if detail.iter().any(|d| d.eq_ignore_ascii_case(&g.name)) {
            report(&x);
        }
    }
    println!("\n{ok}/{} decoded, {} failed, {} implausible", zone.assets.len(), failed.len(), flagged.len());
    println!("{looping} looping, {delta} delta ({moving} move over 1 unit, {delta_3d} 3D)");
    for f in failed.iter().take(15) {
        println!("  FAIL {f}");
    }
    for f in flagged.iter().take(15) {
        println!("  ODD  {f}");
    }
    Ok(())
}

/// A reason the decoded data looks wrong, if any: notetracks out of 0..1,
/// key frames past the end or out of order, translations far off.
fn implausible(x: &XAnim) -> Option<String> {
    if !(x.framerate > 0.0 && x.framerate <= 1000.0) {
        return Some(format!("framerate {}", x.framerate));
    }
    if let Some(n) = x.notifies.iter().find(|n| !(0.0..=1.0).contains(&n.time) || n.name.is_empty()) {
        return Some(format!("notify {:?} at {}", n.name, n.time));
    }
    let frames_ok = |f: &[u16]| f.windows(2).all(|w| w[0] < w[1]) && f.last().is_none_or(|&l| l <= x.num_frames);
    for b in &x.bones {
        if b.name.is_empty() {
            return Some("unnamed bone".into());
        }
        if let RotTrack::Keys { frames, values } = &b.rot {
            if !frames_ok(frames) {
                return Some(format!("{} rotation frames {:?}..", b.name, &frames[..frames.len().min(8)]));
            }
            if values.iter().flatten().any(|c| !c.is_finite()) {
                return Some(format!("{} rotation not finite", b.name));
            }
        }
        if let TransTrack::Keys { frames, values } = &b.trans {
            if !frames_ok(frames) {
                return Some(format!("{} translation frames {:?}..", b.name, &frames[..frames.len().min(8)]));
            }
            // (Bones sit within a few metres of their parents, in inches.)
            if values.iter().flatten().any(|c| !c.is_finite() || c.abs() > 1000.0) {
                return Some(format!("{} translation {:?}", b.name, values.first()));
            }
        }
    }
    None
}

fn report(x: &XAnim) {
    let rot = x.bones.iter().filter(|b| matches!(b.rot, RotTrack::Keys { .. })).count();
    let trans = x.bones.iter().filter(|b| matches!(b.trans, TransTrack::Keys { .. })).count();
    println!(
        "\n{}: {} frames @ {} fps ({:.2}s) loop {} delta {}, {} bones ({rot} rotated, {trans} translated)",
        x.name,
        x.num_frames,
        x.framerate,
        x.duration(),
        x.looping,
        x.delta,
        x.bones.len()
    );
    println!("  bones: {}", x.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(" "));
    for n in &x.notifies {
        println!("  note {:.3} {}", n.time, n.name);
    }
    // A few tracks sampled at the start, middle and end, with how far each
    // moves over the anim.
    let span = |f: &dyn Fn(f32) -> Option<Vec<f32>>| -> Option<(Vec<f32>, Vec<f32>, Vec<f32>, f32)> {
        let n = x.num_frames as f32;
        let (a, m, e) = (f(0.0)?, f(n / 2.0)?, f(n)?);
        let mut range = 0.0f32;
        for i in 0..=x.num_frames {
            let v = f(i as f32)?;
            range = range.max(v.iter().zip(&a).map(|(p, q)| (p - q).abs()).fold(0.0, f32::max));
        }
        Some((a, m, e, range))
    };
    let fmt = |v: &[f32]| v.iter().map(|c| format!("{c:7.3}")).collect::<Vec<_>>().join(" ");
    for b in x.bones.iter().filter(|b| matches!(b.rot, RotTrack::Keys { ref frames, .. } if !frames.is_empty())).take(4) {
        if let Some((a, m, e, r)) = span(&|f| b.rot.sample(f).map(|q| q.to_vec())) {
            let len = (a.iter().map(|c| c * c).sum::<f32>()).sqrt();
            println!("  rot   {:22} [{}] -> [{}] -> [{}] |q| {len:.3} max change {r:.3}", b.name, fmt(&a), fmt(&m), fmt(&e));
        }
    }
    for b in x.bones.iter().filter(|b| matches!(b.trans, TransTrack::Keys { ref frames, .. } if !frames.is_empty())).take(4) {
        if let Some((a, m, e, r)) = span(&|f| b.trans.sample(f).map(|t| t.to_vec())) {
            println!("  trans {:22} [{}] -> [{}] -> [{}] max change {r:.3}", b.name, fmt(&a), fmt(&m), fmt(&e));
        }
    }
}
