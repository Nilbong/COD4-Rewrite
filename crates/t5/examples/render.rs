//! `cargo run --release -p t5 --example render -- <out dir>`: render a sample
//! of Black Ops models in their bind pose to PNGs, to check geometry,
//! textures and attachments (a gun variant hides its weapon file's
//! `hideTags`). A standalone check; the game itself does not use this.

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
    ("ak47", &["t5_weapon_ak47_viewmodel"], "ak47_mp"),
    ("ak47_reflex", &["t5_weapon_ak47_viewmodel"], "ak47_reflex_mp"),
    ("ak47_acog", &["t5_weapon_ak47_viewmodel"], "ak47_acog_mp"),
    ("ak47_gl", &["t5_weapon_ak47_viewmodel"], "ak47_gl_mp"),
    ("ak47_silencer", &["t5_weapon_ak47_viewmodel"], "ak47_silencer_mp"),
    ("ak47_world", &["t5_weapon_ak47_world"], "ak47_mp"),
    ("famas_extclip", &["t5_weapon_famas_viewmodel"], "famas_extclip_mp"),
    ("spas", &["t5_weapon_spas_viewmodel"], "spas_mp"),
    ("python", &["t5_weapon_coltpython_viewmodel"], "python_mp"),
    ("l96a1_vzoom", &["t5_weapon_l96a1_viewmodel"], "l96a1_vzoom_mp"),
    ("viewhands", &["viewhands_usmc"], ""),
    ("cia", &["c_usa_cia_mp_body_standard", "c_usa_cia_mp_head_1"], ""),
    ("spetsnaz", &["c_rus_spet_mp_body_flak", "c_rus_spet_mp_head_2"], ""),
    ("nva", &["c_vtn_nva_mp_body_armor", "c_vtn_nva_mp_head_3"], ""),
    ("cuba", &["c_cub_rebels_mp_body_utility", "c_cub_rebels_mp_head_4"], ""),
    ("ciawin", &["c_usa_ciawin_mp_body_camo", "c_usa_ciawin_mp_head_5"], ""),
];

#[derive(Resource)]
struct Data {
    zones: Vec<Zone>,
    vfs: iw3::iwd::Vfs,
    weapons: Vec<t5::weapons::WeaponFile>,
    out: PathBuf,
    next: usize,
    frames: u32,
    current: Vec<Entity>,
}

fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "t5_renders".into()));
    std::fs::create_dir_all(&out)?;
    let install = t5::Install::locate()?;
    // `code_post_gfx_mp` has the view hands' materials.
    let zones = ["common_mp", "code_post_gfx_mp", "mp_nuked", "mp_array", "mp_cracked", "mp_firingrange"]
        .iter()
        .map(|z| t5::load_iw3(&install, z))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let vfs = install.vfs()?;
    let weapons = t5::weapons::mp_weapons(&vfs);
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window { title: "t5 render".into(), resolution: (1024u32, 640u32).into(), ..default() }),
            ..default()
        }))
        .insert_resource(Data { zones, vfs, weapons, out, next: 0, frames: 0, current: Vec::new() })
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
            let name = SHOTS[data.next - 1].0;
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(data.out.join(format!("{name}.png"))));
        }
        if data.frames < wait + 10 {
            return;
        }
        for e in std::mem::take(&mut data.current) {
            commands.entity(e).despawn();
        }
    }
    let Some(&(_, models, weapon)) = SHOTS.get(data.next) else {
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
    for model in models {
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
            let normals: Vec<[f32; 3]> = s.verts.iter().map(|v| to_bevy(iw3::unpack::unit_vec(v.normal)).normalize_or(Vec3::Y).to_array()).collect();
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
