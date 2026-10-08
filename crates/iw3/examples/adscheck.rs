//! Which guns' ADS animations would make the gun jump: adscheck [zone]
//!
//! The game layers `ads_up` (raising) and `ads_down` (lowering) over the
//! gun's other animations, posed by the ADS fraction, and drops the layer
//! at the hip. Reports guns with either missing, `ads_up`'s end not matching
//! `ads_down`'s start (a jump when aiming out), and bones whose ADS pose
//! differs between the two (in CoD units / degrees).
use anyhow::Result;
use iw3::xanim::XAnim;
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> Result<()> {
    let zone_name = std::env::args().nth(1).unwrap_or("common_mp".into());
    let zone = Zone::parse(&iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?, ParseOptions::default())?;
    let anim = |name: &str| -> Option<XAnim> {
        zone.assets.iter().find_map(|a| match a {
            Asset::Generic(g) if g.ty == AssetType::XAnimParts && g.name.eq_ignore_ascii_case(name) => XAnim::from_node(name, &g.root, &zone.script_strings).ok(),
            _ => None,
        })
    };
    let pose = |a: &XAnim, f: f32| -> Vec<(String, [f32; 4], [f32; 3])> {
        a.bones.iter().map(|b| (b.name.clone(), b.rot.sample(f).unwrap_or([0.0, 0.0, 0.0, 1.0]), b.trans.sample(f).unwrap_or([0.0; 3]))).collect()
    };
    let angle = |a: [f32; 4], b: [f32; 4]| {
        let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs().min(1.0);
        2.0 * d.acos().to_degrees()
    };
    for a in &zone.assets {
        let Asset::Generic(g) = a else { continue };
        if g.ty != AssetType::Weapon {
            continue;
        }
        let names = g.root.strings("szXAnims");
        let get = |i: usize| names.get(i).copied().flatten().filter(|s| !s.is_empty());
        let (up_name, down_name) = (get(31), get(32));
        let (up, down) = (up_name.and_then(anim), down_name.and_then(anim));
        let mut issues = Vec::new();
        match (&up, &down) {
            (None, None) => issues.push("no ads_up or ads_down".to_string()),
            (None, _) => issues.push(format!("no ads_up ({up_name:?})")),
            (_, None) => issues.push(format!("no ads_down ({down_name:?})")),
            (Some(u), Some(d)) => {
                let (ue, ds, de) = (pose(u, u.num_frames.saturating_sub(1) as f32), pose(d, 0.0), pose(d, d.num_frames.saturating_sub(1) as f32));
                let us = pose(u, 0.0);
                for (bone, r, t) in &ue {
                    if let Some((_, r2, t2)) = ds.iter().find(|(n, ..)| n == bone) {
                        let (da, dt) = (angle(*r, *r2), ((t[0] - t2[0]).powi(2) + (t[1] - t2[1]).powi(2) + (t[2] - t2[2]).powi(2)).sqrt());
                        if da > 1.0 || dt > 0.2 {
                            issues.push(format!("{bone}: up end vs down start {da:.1} deg {dt:.2} u"));
                        }
                    }
                }
                for (bone, r, t) in &de {
                    if let Some((_, r2, t2)) = us.iter().find(|(n, ..)| n == bone) {
                        let (da, dt) = (angle(*r, *r2), ((t[0] - t2[0]).powi(2) + (t[1] - t2[1]).powi(2) + (t[2] - t2[2]).powi(2)).sqrt());
                        if da > 1.0 || dt > 0.2 {
                            issues.push(format!("{bone}: down end vs up start (hip) {da:.1} deg {dt:.2} u"));
                        }
                    }
                }
            }
        }
        if !issues.is_empty() {
            println!("{} [{:?} / {:?}]: {}", g.name, up_name, down_name, issues.join("; "));
        }
    }
    Ok(())
}
