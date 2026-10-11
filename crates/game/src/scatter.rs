//! The showcase's ground clutter on Wet Work (`COD4RW_SHOWCASE=1`): small
//! things scattered on the decks the way a working ship gathers them, from
//! the single-player Crew Expendable level's own models (cans, bottles,
//! broken glass, wire, barrel scraps, cardboard, papers), plus grime, rust
//! and puddles of our own. Placed by where they'd collect:
//! - corners (two walls close by) get small heaps of debris;
//! - along walls, grime and rust flakes;
//! - under overhangs and in the open, puddles (as wet as the weather);
//! - never within a few metres of a spawn, nothing taller than an ankle,
//!   and no collision: nothing reads as cover or trips anyone.
//!
//! On Downpour (mp_farm) only puddles: in the dips of open ground, bigger
//! and more of them, the ground round about giving them their edges (no
//! ship's debris on a farm).
//!
//! Each kind of thing is one mesh and material, so the renderer instances
//! them; tiny things stop drawing past 30 m.

use crate::characters::Game;
use crate::content::Content;
use crate::wardrobe::{Source, load};
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::VisibilityRange;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::thread::JoinHandle;

pub struct ScatterPlugin;

impl Plugin for ScatterPlugin {
    fn build(&self, app: &mut App) {
        // (`COD4RW_NOSCATTER`: without it, for telling its pieces from the map's.)
        if !crate::atmos::climate::enabled() || std::env::var_os("COD4RW_NOSCATTER").is_some() {
            return;
        }
        app.add_systems(OnEnter(crate::state::GameState::InGame), start.in_set(crate::state::Setup::Spawn).run_if(crate::atmos::climate::on))
            .add_systems(Update, (place, wet_puddles).chain().run_if(crate::state::in_game).run_if(crate::atmos::climate::on));
    }
}

/// The level its models come from.
const ZONE: &str = "cargoship";

/// Debris for corners: model, how many to a heap (most), and a turn about
/// a random axis (lying on its side).
const DEBRIS: &[(&str, u32, bool)] = &[
    ("com_pallet", 1, false),
    ("com_pallet", 1, false),
    ("me_plastic_crate1", 2, false),
    ("me_plastic_crate3", 2, false),
    ("me_plastic_crate5", 1, false),
    ("com_red_toolbox", 1, false),
    ("com_plastic_bucket", 1, true),
    ("com_cardboardbox01", 2, false),
    ("com_cardboardbox04", 2, false),
    ("com_cardboardbox05", 1, false),
    ("com_pail_metal1", 1, true),
    ("com_soup_can", 5, true),
    ("com_bottle1", 4, true),
    ("cs_vodkabottle_broke01", 1, false),
    ("cs_vodkabottle_broke02", 1, false),
    ("cs_coffeemug01", 1, true),
    ("com_milk_carton", 1, true),
    ("fx_glass_piece_large_01", 2, false),
    ("fx_glass_piece_large_02", 2, false),
    ("com_barrel_piece", 1, false),
    ("com_barrel_piece2", 1, false),
    ("cs_iron_wire", 1, false),
    ("fx_rock_small", 6, false),
    ("fx_rifle_shell", 6, true),
    ("com_clipboard_wpaper", 1, false),
];

/// The level's models loading (none wanted: `Farm`).
#[derive(Resource)]
enum Loading {
    Ship(Option<JoinHandle<anyhow::Result<crate::wardrobe::load::Loaded>>>),
    Farm,
}

#[derive(Component)]
struct Puddle;

fn start(mut commands: Commands, map: Res<crate::world::MapName>) {
    match map.0.as_str() {
        "mp_cargoship" => commands.insert_resource(Loading::Ship(Some(load(Source::zone(Game::Cod4, ZONE))))),
        "mp_farm" => commands.insert_resource(Loading::Farm),
        _ => {}
    }
}

/// Deterministic noise from a spot (so the same deck looks the same).
fn hash(p: Vec2, salt: u32) -> f32 {
    let h = (p.x * 127.1 + p.y * 311.7 + salt as f32 * 74.7).sin() * 43_758.547;
    h - h.floor()
}

#[allow(clippy::too_many_arguments)]
fn place(
    mut commands: Commands,
    loading: Option<ResMut<Loading>>,
    content: Res<Content>,
    map: Option<Res<crate::world::MapInfo>>,
    spatial: avian3d::prelude::SpatialQuery,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Some(mut loading) = loading else { return };
    let handle = match &mut *loading {
        Loading::Ship(h) if !h.as_ref().is_some_and(|h| h.is_finished()) => return,
        Loading::Ship(h) => h.take(),
        Loading::Farm => None,
    };
    commands.remove_resource::<Loading>();
    let farm = handle.is_none();
    let Some(map) = map else { return };
    let t0 = std::time::Instant::now();
    let debris: Vec<(Vec<(Handle<StandardMaterial>, Handle<Mesh>)>, u32, bool)> = match handle.map(|h| h.join()) {
        None => Vec::new(),
        Some(Ok(Ok((zones, vfs)))) => {
            let mut theirs = Content::new(zones, vfs.unwrap_or_else(|| content.vfs.clone()));
            DEBRIS
                .iter()
                .filter_map(|&(name, n, tumble)| crate::props::static_parts(&mut theirs, name, &mut meshes, &mut materials, &mut images).map(|p| (p, n, tumble)))
                .collect()
        }
        Some(_) => {
            warn!("scatter: {ZONE} didn't load");
            return;
        }
    };
    let quad = meshes.add(Plane3d::default().mesh().size(1.0, 1.0));
    let decal = |images: &mut Assets<Image>, materials: &mut Assets<StandardMaterial>, kind: Decal| {
        let image = images.add(decal_image(kind));
        let (color, rough, reflect) = match kind {
            Decal::Puddle => (Color::srgba(0.03, 0.035, 0.04, 0.85), 0.03, 0.9),
            Decal::Grime => (Color::srgba(0.09, 0.07, 0.05, 0.7), 0.9, 0.2),
            Decal::Rust => (Color::srgba(0.42, 0.2, 0.08, 0.9), 0.85, 0.2),
        };
        materials.add(StandardMaterial {
            base_color: color,
            base_color_texture: Some(image),
            perceptual_roughness: rough,
            reflectance: reflect,
            alpha_mode: AlphaMode::Blend,
            depth_bias: 4.0,
            ..default()
        })
    };
    let puddle_mat = decal(&mut images, &mut materials, Decal::Puddle);
    let grime_mat = decal(&mut images, &mut materials, Decal::Grime);
    let rust_mat = decal(&mut images, &mut materials, Decal::Rust);

    // The decks: over the spawns' spread, from above.
    let (lo, hi) = map.spawns.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), s| (lo.min(s.pos), hi.max(s.pos)));
    if lo.x > hi.x {
        return;
    }
    // The heights players walk at (the spawns', and a deck or two above).
    let walk = (lo.y - 1.5, hi.y + 4.0);
    let (lo, hi) = (lo - Vec3::splat(8.0), hi + Vec3::splat(8.0));
    let filter = crate::collision::movement_filter();
    // The map's models' own bounds (world space, a little grown): a cage
    // crate's collision is only its frame, which a piece got inside.
    let model_boxes = model_bounds(&content);
    let in_model = |p: Vec3, half: f32| {
        let lo = p - Vec3::new(half, 0.0, half);
        let hi = p + Vec3::new(half, 0.6, half);
        model_boxes.iter().any(|(a, b)| lo.x < b.x && hi.x > a.x && lo.y < b.y && hi.y > a.y && lo.z < b.z && hi.z > a.z)
    };
    // What a piece mustn't stand in: the world, clip, and the static
    // models' collision (only bullets see that: crates, cars).
    let solid = avian3d::prelude::SpatialQueryFilter::from_mask([crate::collision::Layer::World, crate::collision::Layer::PlayerClip, crate::collision::Layer::ShotClip]);
    let step = 0.7;
    let root = commands.spawn((Name::new("scatter"), Transform::default(), Visibility::default())).id();
    let near = VisibilityRange { start_margin: 0.0..0.0, end_margin: 28.0..32.0, use_aabb: false };
    let mut counts = [0u32; 4];
    // Farm puddles placed (one to a dip: overlapping ones would flicker).
    let mut pools: Vec<Vec3> = Vec::new();
    let mut x = lo.x;
    while x < hi.x {
        let mut z = lo.z;
        while z < hi.z {
            let cell = Vec2::new(x, z);
            let at = cell + Vec2::new(hash(cell, 1) - 0.5, hash(cell, 2) - 0.5) * step;
            z += step;
            let from = Vec3::new(at.x, hi.y + 12.0, at.y);
            // Every floor under this spot (container roofs come first from
            // above): level, with head room, at the heights players walk.
            let floors: Vec<(Vec3, Vec3)> = spatial
                .ray_hits(from, Dir3::NEG_Y, (hi.y - lo.y) + 30.0, 8, true, &filter)
                .into_iter()
                .filter(|h| h.normal.y >= if farm { 0.9 } else { 0.96 })
                .map(|h| (from - Vec3::Y * h.distance, h.normal))
                .filter(|(g, _)| (walk.0..walk.1).contains(&g.y))
                .filter(|(g, _)| spatial.cast_ray(*g + Vec3::Y * 0.1, Dir3::Y, 1.8, true, &filter).is_none())
                .collect();
            for (ground, normal) in floors {
            if map.spawns.iter().any(|s| s.pos.xz().distance(ground.xz()) < 3.0 && (s.pos.y - ground.y).abs() < 2.0) {
                continue;
            }
            // Room for a piece here: nothing (a wall, a crate, a car, a
            // brush entity) in a knee-high box just above the floor.
            let room = |at: Vec3, half: f32| {
                !in_model(at, half) && spatial
                    .shape_intersections(
                        &avian3d::prelude::Collider::cuboid(half * 2.0, 0.5, half * 2.0),
                        at + Vec3::Y * 0.32,
                        Quat::IDENTITY,
                        &solid,
                    )
                    .is_empty()
            };
            // Walls about: eight ways at shin height.
            let eye = ground + Vec3::Y * 0.25;
            let mut walls = Vec::new();
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let d = Vec3::new(a.cos(), 0.0, a.sin());
                if let Some(h) = spatial.cast_ray(eye, Dir3::new(d).unwrap_or(Dir3::X), 1.2, true, &filter) {
                    walls.push((d, h.distance));
                }
            }
            let covered = spatial.cast_ray(ground + Vec3::Y * 0.3, Dir3::Y, 6.0, true, &filter).is_some();
            let nearest = walls.iter().map(|w| w.1).fold(f32::MAX, f32::min);
            // A corner: two walls close by, at an angle.
            let corner = walls.iter().any(|a| walls.iter().any(|b| a.1 < 0.7 && b.1 < 0.7 && a.0.dot(b.0).abs() < 0.3));
            let roll = hash(at, 3);
            let up = Quat::from_rotation_arc(Vec3::Y, normal);
            let spin = Quat::from_rotation_y(hash(at, 4) * std::f32::consts::TAU);
            if farm {
                // A dip in open ground: higher all round, a metre or so off.
                if covered || roll > 0.35 {
                    continue;
                }
                let rim = (0..6).all(|k| {
                    let a = k as f32 * std::f32::consts::TAU / 6.0 + hash(at, 14);
                    let p = ground + Vec3::new(a.cos(), 0.0, a.sin()) * 1.1 + Vec3::Y * 1.0;
                    spatial.cast_ray(p, Dir3::NEG_Y, 1.5, true, &filter).is_some_and(|h| (1.0 - h.distance) > 0.02)
                });
                if !rim || pools.iter().any(|p| p.distance(ground) < 2.2) {
                    continue;
                }
                pools.push(ground);
                let size = 1.2 + hash(at, 8) * 1.8;
                let tf = Transform::from_translation(ground + Vec3::Y * 0.012).with_rotation(up * spin).with_scale(Vec3::new(size, 1.0, size * (0.6 + 0.4 * hash(at, 9))));
                commands.spawn((Puddle, Mesh3d(quad.clone()), MeshMaterial3d(puddle_mat.clone()), tf, NotShadowCaster, ChildOf(root)));
                counts[2] += 1;
                continue;
            }
            if corner && roll < 0.25 && !debris.is_empty() {
                if !room(ground, 0.3) {
                    continue;
                }
                // A small heap: one kind, a few of it.
                let (parts, most, tumble) = &debris[(hash(at, 5) * debris.len() as f32) as usize % debris.len()];
                let n = 1 + (hash(at, 6) * *most as f32) as u32;
                for i in 0..n {
                    let off = Vec3::new(hash(at, 10 + i) - 0.5, 0.0, hash(at, 20 + i) - 0.5) * 0.6;
                    let turn = if *tumble { Quat::from_rotation_z(std::f32::consts::FRAC_PI_2 * (hash(at, 30 + i) > 0.4) as u32 as f32) } else { Quat::IDENTITY };
                    let tf = Transform::from_translation(ground + off).with_rotation(up * Quat::from_rotation_y(hash(at, 40 + i) * std::f32::consts::TAU) * turn);
                    for (m, mesh) in parts {
                        commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(m.clone()), tf, NotShadowCaster, near.clone(), ChildOf(root)));
                    }
                }
                counts[0] += n;
            } else if nearest < 0.6 && roll < 0.2 {
                // Along the wall: a strip of grime, with rust flakes now and then.
                let wall = walls.iter().min_by(|a, b| a.1.total_cmp(&b.1)).map_or(Vec3::X, |w| w.0);
                // Now and then a thing left against the wall instead.
                if hash(at, 12) < 0.4 && !debris.is_empty() {
                    // (No room: the spot stays empty, not a grime strip.)
                    if !room(ground + wall * (nearest - 0.55).max(0.0), 0.25) {
                        continue;
                    }
                    let (parts, _, _) = &debris[(hash(at, 13) * debris.len() as f32) as usize % debris.len()];
                    let tf = Transform::from_translation(ground + wall * (nearest - 0.3).max(0.0)).with_rotation(up * spin);
                    for (m, mesh) in parts {
                        commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(m.clone()), tf, NotShadowCaster, near.clone(), ChildOf(root)));
                    }
                    counts[0] += 1;
                    continue;
                }
                let along = Quat::from_rotation_arc(Vec3::Z, Vec3::new(-wall.z, 0.0, wall.x));
                let rust = hash(at, 7) < 0.3;
                let size = if rust { Vec3::new(0.6, 1.0, 0.6) } else { Vec3::new(0.7, 1.0, 2.6) };
                let tf = Transform::from_translation(ground + wall * (nearest - 0.2).max(0.0) + Vec3::Y * 0.006).with_rotation(up * along).with_scale(size);
                commands.spawn((Mesh3d(quad.clone()), MeshMaterial3d(if rust { rust_mat.clone() } else { grime_mat.clone() }), tf, NotShadowCaster, ChildOf(root)));
                counts[1] += 1;
            } else if (covered && roll < 0.025) || (!covered && walls.is_empty() && roll < 0.008) {
                // A puddle where the deck dips (as wet as the weather).
                let size = 0.8 + hash(at, 8) * 1.6;
                let tf = Transform::from_translation(ground + Vec3::Y * 0.008).with_rotation(up * spin).with_scale(Vec3::new(size, 1.0, size * (0.6 + 0.4 * hash(at, 9))));
                commands.spawn((Puddle, Mesh3d(quad.clone()), MeshMaterial3d(puddle_mat.clone()), tf, NotShadowCaster, ChildOf(root)));
                counts[2] += 1;
            } else if !covered && nearest > 1.0 && roll < 0.012 && !debris.is_empty() && room(ground, 0.3) {
                // A stray bit in the open (a can, a shell case).
                let (parts, _, _) = &debris[(hash(at, 11) * 3.0) as usize % debris.len()];
                let tf = Transform::from_translation(ground).with_rotation(up * spin * Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
                for (m, mesh) in parts {
                    commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(m.clone()), tf, NotShadowCaster, near.clone(), ChildOf(root)));
                }
                counts[3] += 1;
            }
            }
        }
        x += step;
    }
    info!(
        "scatter: {} debris in corners, {} grime and rust strips, {} puddles, {} strays ({} kinds) in {:.0} ms",
        counts[0],
        counts[1],
        counts[2],
        counts[3],
        debris.len(),
        t0.elapsed().as_secs_f32() * 1000.0
    );
}

/// World-space boxes (Bevy space) round the map's static models and
/// script_model props, from each model's own bounds, grown 5 cm.
fn model_bounds(content: &Content) -> Vec<(Vec3, Vec3)> {
    let mut out = Vec::new();
    let mut add = |name_or_id: Option<&iw3::zone::XModel>, origin: [f32; 3], axis: [[f32; 3]; 3], scale: f32| {
        let Some(xm) = name_or_id else { return };
        let (mn, mx) = (Vec3::from(xm.mins), Vec3::from(xm.maxs));
        if mx.cmple(mn).any() {
            return;
        }
        let (mut lo, mut hi) = (Vec3::MAX, Vec3::MIN);
        for i in 0..8 {
            let c = Vec3::new(if i & 1 == 0 { mn.x } else { mx.x }, if i & 2 == 0 { mn.y } else { mx.y }, if i & 4 == 0 { mn.z } else { mx.z }) * scale;
            let w = Vec3::from(origin) + Vec3::from(axis[0]) * c.x + Vec3::from(axis[1]) * c.y + Vec3::from(axis[2]) * c.z;
            let p = crate::units::pos(w.to_array());
            lo = lo.min(p);
            hi = hi.max(p);
        }
        out.push((lo - Vec3::splat(0.05), hi + Vec3::splat(0.05)));
    };
    let zone = content.map();
    if let Some(w) = zone.gfx_world() {
        for sm in &w.static_models {
            add(sm.model.and_then(|id| zone.xmodel(id)), sm.origin, sm.axis, sm.scale);
        }
    }
    let ents = zone.map_ents().map(|e| iw3::ents::parse(&e.entity_string)).unwrap_or_default();
    for e in ents.iter().filter(|e| e.classname() == "script_model") {
        let (Some(name), Some(origin)) = (e.get("model"), e.origin()) else { continue };
        let Some((zi, id)) = content.find(name) else { continue };
        let r = crate::modes::koth::cod_rotation(e.angles());
        // Bevy rotation back to CoD axes: forward x, left y, up z.
        let ax = |v: Vec3| {
            let b = r * crate::units::dir(v.to_array());
            [b.x, -b.z, b.y]
        };
        add(content.zone(zi).xmodel(id), origin, [ax(Vec3::X), ax(Vec3::Y), ax(Vec3::Z)], 1.0);
    }
    out
}

/// Puddles as wet as the weather (`crate::atmos::climate::Weather`):
/// gone when the deck is dry.
fn wet_puddles(weather: Option<Res<crate::atmos::climate::Weather>>, mut puddles: Query<&mut Visibility, With<Puddle>>, mut was: Local<Option<bool>>) {
    let wet = weather.map_or(true, |w| !w.enabled || w.wetness > 0.15);
    if *was == Some(wet) {
        return;
    }
    *was = Some(wet);
    for mut v in &mut puddles {
        *v = if wet { Visibility::Inherited } else { Visibility::Hidden };
    }
}

#[derive(Clone, Copy)]
enum Decal {
    Puddle,
    Grime,
    Rust,
}

/// A decal's shape: soft-edged and broken up (alpha), white otherwise (the
/// material colours it).
fn decal_image(kind: Decal) -> Image {
    const N: usize = 64;
    let mut data = Vec::with_capacity(N * N * 4);
    for j in 0..N {
        for i in 0..N {
            let p = Vec2::new(i as f32, j as f32) / (N - 1) as f32 * 2.0 - 1.0;
            let n = value_noise(p * 4.0) * 0.6 + value_noise(p * 11.0) * 0.4;
            let a = match kind {
                // A blob with a ragged edge.
                Decal::Puddle => smooth(1.0 - (p.length() + (n - 0.5) * 0.5), 0.0, 0.18),
                // A long smear fading out along it, dense near the wall (x -1).
                Decal::Grime => smooth(1.0 - p.y.abs(), 0.0, 0.5) * smooth(1.0 - (p.x + 1.0) * 0.5, 0.0, 0.6) * (0.5 + n * 0.8),
                // Flakes: specks.
                Decal::Rust => smooth(n - 0.55, 0.0, 0.08) * smooth(1.0 - p.length(), 0.0, 0.3),
            };
            data.extend_from_slice(&[255, 255, 255, (a.clamp(0.0, 1.0) * 255.0) as u8]);
        }
    }
    Image::new(Extent3d { width: N as u32, height: N as u32, depth_or_array_layers: 1 }, TextureDimension::D2, data, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD)
}

fn smooth(x: f32, lo: f32, width: f32) -> f32 {
    let t = ((x - lo) / width).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn value_noise(p: Vec2) -> f32 {
    let c = p.floor();
    let f = p - c;
    let u = f * f * (Vec2::splat(3.0) - 2.0 * f);
    let h = |q: Vec2| hash(q, 99);
    let a = h(c);
    let b = h(c + Vec2::X);
    let d = h(c + Vec2::Y);
    let e = h(c + Vec2::ONE);
    a + (b - a) * u.x + (d - a) * u.y + (a - b - d + e) * u.x * u.y
}
