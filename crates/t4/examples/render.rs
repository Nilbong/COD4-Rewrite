//! `cargo run --release -p t4 --example render -- <out dir> [--campaign
//! [character...]]`: render a sample of World at War models in their bind
//! pose to PNGs, to check geometry, textures and attachments (a gun variant
//! hides its weapon file's `hideTags`). `--campaign` renders campaign
//! characters instead (the first choice of each part). A standalone check;
//! the game itself does not use this.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use iw3::iwi::{Format, Iwi};
use iw3::zone::{TextureSemantic, XModel, Zone};
use std::collections::HashSet;
use std::path::PathBuf;

/// (output name, models drawn together, weapon file whose hidden tags apply)
const SHOTS: &[(&str, &[&str], &str)] = &[
    ("thompson", &["viewmodel_mp_thompson"], "thompson_mp"),
    ("thompson_aperture", &["viewmodel_mp_thompson"], "thompson_aperture_mp"),
    ("thompson_silenced", &["viewmodel_mp_thompson"], "thompson_silenced_mp"),
    ("thompson_bigammo", &["viewmodel_mp_thompson"], "thompson_bigammo_mp"),
    ("thompson_world", &["weapon_mp_thompson_smg"], ""),
    ("kar98k_scoped", &["viewmodel_mp_k98"], "kar98k_scoped_mp"),
    ("kar98k_gl", &["viewmodel_mp_k98"], "kar98k_gl_mp"),
    ("kar98k_bayonet", &["viewmodel_mp_k98"], "kar98k_bayonet_mp"),
    ("m1garand", &["viewmodel_mp_garand"], "m1garand_mp"),
    ("stg44_telescopic", &["viewmodel_mp_stg44"], "stg44_telescopic_mp"),
    ("ppsh", &["viewmodel_mp_ppsh"], "ppsh_mp"),
    ("mg42", &["viewmodel_mp_mg42_mg"], "mg42_mp"),
    ("doublebarrel_sawoff", &["viewmodel_mp_double_barrel"], "doublebarreledshotgun_sawoff_mp"),
    ("357magnum", &["viewmodel_mp_357magnum"], "357magnum_mp"),
    ("bazooka", &["viewmodel_usa_bazooka_at"], "bazooka_mp"),
    ("marine_arms", &["viewmodel_usa_marine_arms"], ""),
    ("usa_raider_rifle", &["char_usa_raider_player_body_rifle", "char_usa_raider_player_head_rifle"], ""),
    ("jap_impinf_rifle", &["char_jap_impinf_player_body_rifle", "char_jap_impinf_player_head_rifle"], ""),
    ("rus_guard_smg", &["char_rus_guard_player_body_smg", "char_rus_guard_player_head_smg"], ""),
    ("ger_hnrgd_lmg", &["char_ger_hnrgd_player_body_lmg", "char_ger_hnrgd_player_head_lmg"], ""),
];

/// Campaign characters drawn by `--campaign` without names.
const CAMPAIGN_SHOTS: &[&str] = &[
    "usa_raider_h_roebuck",
    "usa_raider_h_sullivan",
    "usa_marine_h_polonsky",
    "rus_h_reznov",
    "rus_p_chernova",
    "usa_raider_r_rifle",
    "usa_marine_r_corpsman",
    "usa_marine_player1",
    "rus_player1",
    "jap_off",
    "jap_makpel_rifle",
    "ger_wrmcht_k98",
    "ger_ansel",
    "usa_pbycrew_pilot",
];

#[derive(Clone)]
struct Shot {
    name: String,
    models: Vec<String>,
    weapon: String,
}

#[derive(Resource)]
struct Data {
    shots: Vec<Shot>,
    zones: Vec<Zone>,
    vfs: iw3::iwd::Vfs,
    weapons: Vec<t4::weapons::WeaponFile>,
    out: PathBuf,
    next: usize,
    frames: u32,
    current: Vec<Entity>,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = PathBuf::from(args.first().cloned().unwrap_or_else(|| "t4_renders".into()));
    std::fs::create_dir_all(&out)?;
    let install = t4::Install::locate()?;
    let (shots, zone_names) = match args.iter().position(|a| a == "--campaign") {
        Some(i) => campaign_shots(&install, &args[i + 1..])?,
        None => {
            let shots = SHOTS
                .iter()
                .map(|&(name, models, weapon)| Shot {
                    name: name.to_owned(),
                    models: models.iter().map(|m| m.to_string()).collect(),
                    weapon: weapon.to_owned(),
                })
                .collect();
            (shots, std::iter::once("common_mp").chain(t4::catalog::CHARACTER_ZONES).map(str::to_owned).collect())
        }
    };
    let zones = zone_names.iter().map(|z| t4::load_iw3(&install, z)).collect::<anyhow::Result<Vec<_>>>()?;
    let vfs = install.vfs()?;
    let weapons = t4::weapons::mp_weapons(&vfs);
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window { title: "t4 render".into(), resolution: (1024u32, 640u32).into(), ..default() }),
            ..default()
        }))
        .insert_resource(Data { shots, zones, vfs, weapons, out, next: 0, frames: 0, current: Vec::new() })
        .insert_resource(ClearColor(Color::srgb(0.32, 0.34, 0.38)))
        .add_systems(Startup, |mut commands: Commands| {
            commands.spawn((DirectionalLight { illuminance: 9000.0, ..default() }, Transform::default().looking_to(Vec3::new(-0.4, -0.7, -0.6), Vec3::Y)));
            commands.spawn((DirectionalLight { illuminance: 2500.0, ..default() }, Transform::default().looking_to(Vec3::new(0.6, -0.2, 0.8), Vec3::Y)));
            commands.spawn((Camera3d::default(), Transform::default()));
        })
        .add_systems(Update, step)
        .run();
    Ok(())
}

/// The campaign characters to draw (`names`, or [`CAMPAIGN_SHOTS`]) and the
/// zones holding them.
fn campaign_shots(install: &t4::Install, names: &[String]) -> anyhow::Result<(Vec<Shot>, Vec<String>)> {
    use t4::zone::{AssetType, ParseOptions};
    let mut contents = Vec::new();
    for name in t4::campaign::CAMPAIGN_ZONES {
        let zone = t4::zone::Zone::parse(&t4::fastfile::load(&install.zone_path(name))?, ParseOptions::default())?;
        let models: Vec<String> = zone.of_type(AssetType::XModel).map(|(_, a)| a.name.clone()).collect();
        let raw: Vec<(String, String)> = zone
            .of_type(AssetType::RawFile)
            .map(|(_, a)| (a.name.clone(), String::from_utf8_lossy(a.root.bytes("buffer")).into_owned()))
            .collect();
        contents.push((name, models, raw));
    }
    let zones: Vec<t4::campaign::ZoneContents> = contents
        .iter()
        .map(|(n, m, r)| (*n, m.iter().map(String::as_str).collect(), r.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect()))
        .collect();
    let characters = t4::campaign::characters(&zones);
    let wanted: Vec<String> = if names.is_empty() { CAMPAIGN_SHOTS.iter().map(|n| n.to_string()).collect() } else { names.to_vec() };
    let mut shots = Vec::new();
    let mut zone_names: Vec<String> = Vec::new();
    for name in wanted {
        let Some(c) = characters.iter().find(|c| c.name == name) else {
            anyhow::bail!("no campaign character {name}");
        };
        let l = &c.look;
        let models = [&l.body, &l.head, &l.hat, &l.gear].into_iter().filter_map(|p| p.first().cloned()).collect();
        shots.push(Shot { name: format!("campaign_{name}"), models, weapon: String::new() });
        if !zone_names.contains(&c.zone) {
            zone_names.push(c.zone.clone());
        }
    }
    Ok((shots, zone_names))
}

/// CoD axes (x forward, y left, z up) to Bevy's (y up, model forward +X).
fn to_bevy(v: [f32; 3]) -> Vec3 {
    Vec3::new(v[0], v[2], -v[1])
}

#[allow(clippy::too_many_arguments)]
fn step(
    mut commands: Commands,
    mut data: ResMut<Data>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut camera: Single<&mut Transform, With<Camera3d>>,
    mut exit: MessageWriter<AppExit>,
) {
    data.frames += 1;
    // Give the renderer time to compile its pipelines before the first shot.
    let wait = if data.next <= 1 { 240 } else { 20 };
    if !data.current.is_empty() {
        if data.frames == wait {
            let name = &data.shots[data.next - 1].name;
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(data.out.join(format!("{name}.png"))));
        }
        if data.frames < wait + 10 {
            return;
        }
        for e in std::mem::take(&mut data.current) {
            commands.entity(e).despawn();
        }
    }
    let Some(Shot { models, weapon, .. }) = data.shots.get(data.next).cloned() else {
        exit.write(AppExit::Success);
        return;
    };
    data.next += 1;
    data.frames = 0;
    let hidden: Vec<String> =
        data.weapons.iter().find(|w| w.name == weapon).map(|w| w.hide_tags()).unwrap_or_default();
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let mut spawned = Vec::new();
    // Later models (heads) hang off the first one's bones: line up the
    // first bone they share.
    let first = models.first().and_then(|m| data.zones.iter().find_map(|z| z.xmodel(z.find(m)?)));
    for model in &models {
        let Some((zone, xm)) = data.zones.iter().find_map(|z| {
            let id = z.find(model)?;
            z.xmodel(id).map(|xm| (z, xm))
        }) else {
            warn!("no model {model}");
            continue;
        };
        let dropped = hidden_bones(xm, &hidden);
        let place = first.filter(|f| !std::ptr::eq(*f, xm)).and_then(|f| {
            let (bi, fi) = xm.bone_names.iter().enumerate().find_map(|(i, n)| Some((i, f.bone_names.iter().position(|x| x == n)?)))?;
            let mat = |b: &iw3::zone::BoneMat| {
                Transform::from_translation(to_bevy(b.trans)).with_rotation(Quat::from_xyzw(b.quat[0], b.quat[2], -b.quat[1], b.quat[3]).normalize())
            };
            Some(mat(&f.base_mat[fi]).compute_affine() * mat(&xm.base_mat[bi]).compute_affine().inverse())
        });
        for (si, s) in xm.surfs.iter().enumerate().take(xm.lods.first().map_or(xm.surfs.len(), |l| l.num_surfs as usize)) {
            let positions: Vec<[f32; 3]> =
                s.verts.iter().map(|v| place.map_or(to_bevy(v.xyz), |p| p.transform_point3(to_bevy(v.xyz))).to_array()).collect();
            // Attached parts are modelled around their own root bone, so
            // `place` rotates them: their normals turn with them.
            let normals: Vec<[f32; 3]> = s
                .verts
                .iter()
                .map(|v| {
                    let n = to_bevy(iw3::unpack::unit_vec(v.normal));
                    place.map_or(n, |p| p.transform_vector3(n)).normalize_or(Vec3::Y).to_array()
                })
                .collect();
            let uvs: Vec<[f32; 2]> = s.verts.iter().map(|v| iw3::unpack::tex_coords(v.tex_coord)).collect();
            // Triangles of rigid parts bound to hidden bones are left out.
            let mut skip = HashSet::new();
            for l in &s.vert_lists {
                if dropped.contains(&(l.bone_offset as usize / 64)) {
                    skip.extend(l.tri_offset as usize..(l.tri_offset + l.tri_count) as usize);
                }
            }
            let indices: Vec<u32> = s
                .tris
                .iter()
                .enumerate()
                .filter(|(i, _)| !skip.contains(i))
                .flat_map(|(_, t)| [t[0] as u32, t[2] as u32, t[1] as u32])
                .collect();
            if indices.is_empty() {
                continue;
            }
            for &i in &indices {
                let p = Vec3::from(positions[i as usize]);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_indices(Indices::U32(indices));
            // A `,name` material is a reference to another zone's copy.
            let material = xm.materials.get(si).copied().flatten().and_then(|m| zone.material(m));
            let material = material.and_then(|m| {
                let name = m.name.trim_start_matches(',');
                data.zones.iter().find_map(|z| z.material(z.find(name)?).map(|m| (z, m)))
            });
            let color = material.and_then(|(z, m)| {
                let t = m.textures.iter().find(|t| t.semantic == TextureSemantic::Color).or(m.textures.first())?;
                z.image(t.image?).map(|i| i.name.clone())
            });
            // Alpha testing as the lit technique's state bits say.
            let bits = material.and_then(|(_, m)| m.state_bits_for(iw3::zone::TECHNIQUE_LIT).or(m.state_bits.first().copied()));
            let alpha_test = bits.is_some_and(|[b0, _]| b0 & 0x800 == 0 && b0 & 0x3000 != 0);
            let texture = color.and_then(|n| texture(&data.vfs, &n)).map(|i| images.add(i));
            let mat = materials.add(StandardMaterial {
                base_color_texture: texture,
                perceptual_roughness: 0.6,
                alpha_mode: if alpha_test { AlphaMode::Mask(0.5) } else { AlphaMode::Opaque },
                double_sided: true,
                cull_mode: None,
                ..default()
            });
            spawned.push(commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(mat), Transform::default())).id());
        }
    }
    data.current = spawned;
    if lo.x <= hi.x {
        let (center, size) = ((lo + hi) * 0.5, hi - lo);
        let dist = size.max_element() * 1.25 + 1.0;
        // Guns from the side, characters from three-quarters on.
        let dir = if size.y > size.x { Vec3::new(0.8, 0.1, 0.6) } else { Vec3::new(0.0, 0.15, 1.0) };
        **camera = Transform::from_translation(center + dir.normalize() * dist).looking_at(center, Vec3::Y);
    }
}

/// Bones named in `tags` (any case) and every bone below them.
fn hidden_bones(xm: &XModel, tags: &[String]) -> HashSet<usize> {
    let roots = xm.num_root_bones as usize;
    let parent = |i: usize| (i >= roots).then(|| i - xm.parent_list.get(i - roots).copied().unwrap_or(0) as usize);
    let mut out = HashSet::new();
    for i in 0..xm.bone_names.len() {
        let mut b = Some(i);
        while let Some(j) = b {
            if tags.iter().any(|t| t.eq_ignore_ascii_case(&xm.bone_names[j])) {
                out.insert(i);
                break;
            }
            b = parent(j).filter(|&p| p != j);
        }
    }
    out
}

fn texture(vfs: &iw3::iwd::Vfs, name: &str) -> Option<Image> {
    let data = vfs.read(&format!("images/{}.iwi", name.trim_start_matches(','))).ok()??;
    let iwi = Iwi::parse(&data).ok()?;
    let size = Extent3d { width: iwi.width, height: iwi.height, depth_or_array_layers: 1 };
    let (format, bytes) = match iwi.format {
        Format::Dxt1 => (TextureFormat::Bc1RgbaUnormSrgb, iwi.levels[0].clone()),
        Format::Dxt3 => (TextureFormat::Bc2RgbaUnormSrgb, iwi.levels[0].clone()),
        Format::Dxt5 => (TextureFormat::Bc3RgbaUnormSrgb, iwi.levels[0].clone()),
        _ => (TextureFormat::Rgba8UnormSrgb, iwi.to_rgba8(0)?),
    };
    if iwi.format.is_compressed() && (iwi.width % 4 != 0 || iwi.height % 4 != 0) {
        return None;
    }
    let mut image = Image::new_uninit(size, TextureDimension::D2, format, RenderAssetUsages::RENDER_WORLD);
    image.data = Some(bytes);
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..default()
    });
    Some(image)
}
