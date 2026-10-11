//! Dump what the mocap retargeter (tools/mocap) needs, as JSON on stdout:
//! the viewhands' and a gun's skeletons (bind pose, model space, CoD axes,
//! inches) and a weapon's viewmodel anims sampled at every frame.
//! mocapref <weapon> <hands model> [zones...]
use anyhow::Result;
use iw3::xanim::XAnim;
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let weapon = args.next().unwrap_or("radar_mp".into());
    let hands = args.next().unwrap_or("viewhands_black_kit".into());
    let mut names: Vec<String> = args.collect();
    if names.is_empty() {
        names = vec!["common_mp".into(), "mp_crash".into()];
    }
    let install = iw3::Install::locate()?;
    let zones: Vec<Zone> = names.iter().map(|n| Zone::parse(&iw3::fastfile::load(&install.zone_path(n)).unwrap(), ParseOptions::default()).unwrap()).collect();
    let w = zones
        .iter()
        .find_map(|z| z.assets.iter().find_map(|a| match a { Asset::Generic(g) if g.ty == AssetType::Weapon && g.name == weapon => Some((z, &g.root)), _ => None }))
        .expect("weapon");
    let gun = w.1.assets("gunXModel").into_iter().flatten().next().map(|i| w.0.get(i).name().to_owned()).unwrap_or_default();
    let model = |name: &str| -> String {
        for z in &zones {
            let Some(id) = z.assets.iter().position(|a| matches!(a, Asset::XModel(m) if m.name == name)) else { continue };
            let x = z.xmodel(id).unwrap();
            let roots = x.num_root_bones as usize;
            let bones: Vec<String> = (0..x.num_bones as usize)
                .map(|i| {
                    let parent = if i < roots { -1 } else { i as i32 - x.parent_list[i - roots] as i32 };
                    let b = x.base_mat.get(i);
                    let q = b.map_or([0.0, 0.0, 0.0, 1.0], |b| b.quat);
                    let t = b.map_or([0.0; 3], |b| b.trans);
                    let base = if i >= roots { x.trans[i - roots] } else { t };
                    format!(r#"{{"name":"{}","parent":{parent},"q":{q:?},"t":{t:?},"anim_base":{base:?}}}"#, x.bone_names[i])
                })
                .collect();
            return format!(r#"{{"name":"{name}","bones":[{}]}}"#, bones.join(","));
        }
        format!(r#"{{"name":"{name}","bones":[]}}"#)
    };
    let mut anims = Vec::new();
    for (slot, n) in w.1.strings("szXAnims").into_iter().enumerate() {
        let n = n.unwrap_or("");
        if n.is_empty() {
            continue;
        }
        let Some((z, g)) = zones.iter().find_map(|z| z.assets.iter().find_map(|a| match a { Asset::Generic(g) if g.ty == AssetType::XAnimParts && g.name.eq_ignore_ascii_case(n) => Some((z, g)), _ => None })) else { continue };
        let a = XAnim::from_node(n, &g.root, &z.script_strings)?;
        let tracks: Vec<String> = a
            .bones
            .iter()
            .map(|b| {
                let r: Vec<String> = (0..=a.num_frames).map(|f| b.rot.sample(f as f32).map_or("null".into(), |q| format!("{q:?}"))).collect();
                let t: Vec<String> = (0..=a.num_frames).map(|f| b.trans.sample(f as f32).map_or("null".into(), |q| format!("{q:?}"))).collect();
                format!(r#"{{"name":"{}","rot":[{}],"trans":[{}]}}"#, b.name, r.join(","), t.join(","))
            })
            .collect();
        anims.push(format!(r#"{{"slot":{slot},"name":"{n}","frames":{},"fps":{},"bones":[{}]}}"#, a.num_frames, a.framerate, tracks.join(",")));
    }
    println!(r#"{{"weapon":"{weapon}","hands":{},"gun":{},"anims":[{}]}}"#, model(&hands), model(&gun), anims.join(","));
    Ok(())
}
