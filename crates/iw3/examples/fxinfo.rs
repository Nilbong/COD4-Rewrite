//! Print effects: fxinfo <zone> [effect name | weapon:<weapon> | impacts | list]
use anyhow::Result;
use iw3::fx::{FxEffectDef, ImpactTable};
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let zone_name = args.next().unwrap_or("common_mp".into());
    let what = args.next().unwrap_or("list".into());
    let data = iw3::fastfile::load(&iw3::Install::locate()?.zone_path(&zone_name))?;
    let zone = Zone::parse(&data, ParseOptions::default())?;
    println!("zone {zone_name}: stopped at {:?}", zone.stats.stopped_at);
    let generic = |ty: AssetType| {
        zone.assets.iter().filter_map(move |a| match a {
            Asset::Generic(g) if g.ty == ty => Some(g),
            _ => None,
        })
    };
    match what.as_str() {
        "list" => {
            for g in generic(AssetType::Fx) {
                let def = FxEffectDef::from_node(&zone, &g.root);
                let types: Vec<String> = def.elems.iter().map(|e| format!("{:?}", e.elem_type)).collect();
                println!("{} [{}]", g.name, types.join(", "));
            }
        }
        "impacts" => {
            for g in generic(AssetType::ImpactFx) {
                let t = ImpactTable::from_node(&zone, &g.root);
                println!("impact table {}", t.name);
                for (i, e) in t.entries.iter().enumerate() {
                    println!("  [{i}] flesh {:?}", e.flesh);
                    for (s, n) in e.nonflesh.iter().enumerate() {
                        if let Some(n) = n {
                            println!("      surf {s:2} {n}");
                        }
                    }
                }
            }
        }
        w if w.starts_with("weapon:") => {
            let name = &w[7..];
            let g = generic(AssetType::Weapon).find(|g| g.name == name).expect("weapon");
            for f in ["viewFlashEffect", "worldFlashEffect", "viewShellEjectEffect", "worldShellEjectEffect"] {
                let fx = g.root.asset(f).map(|i| zone.get(i).name().to_owned());
                println!("{f}: {fx:?}");
            }
        }
        name => {
            let g = generic(AssetType::Fx).find(|g| g.name == name).expect("effect");
            let def = FxEffectDef::from_node(&zone, &g.root);
            println!(
                "{}: flags {:#x} looping life {} looping {:?} one-shot {:?} emission {:?}",
                def.name,
                def.flags,
                def.msec_looping_life,
                def.looping(),
                def.one_shot(),
                def.emission()
            );
            for (i, e) in def.elems.iter().enumerate() {
                print_elem(i, e);
            }
        }
    }
    Ok(())
}

/// One element, compactly: ranges as `base+amp`, graphs one sample a line.
fn print_elem(i: usize, e: &iw3::fx::FxElemDef) {
    let r = |f: &iw3::fx::FloatRange| format!("{}+{}", f.base, f.amplitude);
    let v3 = |v: &iw3::fx::Vec3Range| format!("{:?}+{:?}", v.base, v.amplitude);
    println!(
        "elem {i}: {:?} flags {:#x} spawn {:?} delay {:?} life {:?} visuals {:?}",
        e.elem_type, e.flags, e.spawn, e.spawn_delay_msec, e.life_span_msec, e.visuals
    );
    println!(
        "  origin [{}, {}, {}] offset r {} h {} angles [{}, {}, {}] angvel [{}, {}, {}] rot {} gravity {} atlas {:?}",
        r(&e.spawn_origin[0]),
        r(&e.spawn_origin[1]),
        r(&e.spawn_origin[2]),
        r(&e.spawn_offset_radius),
        r(&e.spawn_offset_height),
        r(&e.spawn_angles[0]),
        r(&e.spawn_angles[1]),
        r(&e.spawn_angles[2]),
        r(&e.angular_velocity[0]),
        r(&e.angular_velocity[1]),
        r(&e.angular_velocity[2]),
        r(&e.initial_rotation),
        r(&e.gravity),
        e.atlas
    );
    println!(
        "  fade in {} out {} sort {} lighting {} impact {:?} death {:?} emitted {:?}",
        r(&e.fade_in_range),
        r(&e.fade_out_range),
        e.sort_order,
        e.lighting_frac,
        e.effect_on_impact,
        e.effect_on_death,
        e.effect_emitted
    );
    for (k, s) in e.vel_samples.iter().enumerate() {
        println!(
            "  vel {k}: local {} total {} world {} total {}",
            v3(&s.local_velocity),
            v3(&s.local_total),
            v3(&s.world_velocity),
            v3(&s.world_total)
        );
    }
    for (k, s) in e.vis_samples.iter().enumerate() {
        let (b, a) = (&s.base, &s.amplitude);
        println!(
            "  vis {k}: color {:?}/{:?} size {:?}+{:?} rot {}+{} total {}+{} scale {}+{}",
            b.color,
            a.color,
            b.size,
            a.size,
            b.rotation_delta,
            a.rotation_delta,
            b.rotation_total,
            a.rotation_total,
            b.scale,
            a.scale
        );
    }
}
