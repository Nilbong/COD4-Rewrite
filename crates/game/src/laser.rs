//! The IR laser: an attachment on any gun (`irlaser` in its spec, on top
//! of the others): CoD4's own laser unit (the single-player M4's, see
//! [`mount`]) on the gun, and its beam drawn as CoD4's single-player lasers
//! are (`CG_Laser_Add`, the `gfx_laser` material): from the unit's emitter
//! to where the gun's shots go (the laser is the point of aim), up to 1500
//! units, fading towards its far end and widening with distance from the
//! viewer, and a glow where it lands. Only through night vision: a viewer sees every
//! laser (their own, other players', bots') while their goggles are on,
//! and none otherwise (`CG_LookingThroughNightVision`).
//!
//! With a laser fitted and night vision on, aiming doesn't go to the
//! sights: the gun comes up to the cheek, canted inward (Modern Warfare
//! 2019's night-vision laser aim), with a little zoom, spread between the
//! hip's and the sights', and nearly the sights' walking speed
//! ([`crate::weapons::WeaponState::laser_aim`]; `lasergun.*` knobs in
//! `tuning.txt`).
//!
//! Bots wear night vision on night maps ([`night_map`]) and carry lasers
//! there ([`crate::bots`]' classes).
//!
//! Debug aids: `COD4RW_LASER=1` fits one to the local player's guns;
//! `COD4RW_LASERTEST=<dir>` (see [`test`]).

use crate::combat::{Dead, Hitbox};
use crate::models::Skeleton;
use crate::units::u;
use avian3d::prelude::SpatialQuery;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use std::collections::HashMap;

/// The attachment's name in a gun spec (`ak47:reflex+irlaser`).
pub const ATTACHMENT: &str = "irlaser";

/// `cg_laserRange` / `cg_laserRangePlayer`, `cg_laserRadius`,
/// `cg_laserEndOffset`, `cg_laserFlarePct`, `cg_laserLightRadius` (CoD
/// units, percent per unit).
const RANGE: f32 = 1500.0;
const RADIUS: f32 = 0.8;
const END_OFFSET: f32 = 0.5;
const FLARE_PCT: f32 = 0.2;
const LIGHT_RADIUS: f32 = 3.0;

/// Render layers of each local player's lasers in splitscreen (past the
/// players' body layers, 31..35).
pub fn layer(slot: usize) -> usize {
    40 + slot
}

/// Does this gun spec have the laser?
pub fn fitted(spec: &str) -> bool {
    spec.split_once(':').is_some_and(|(_, a)| a.split('+').any(|a| a == ATTACHMENT))
}

/// The spec with the laser put on, or taken off.
pub fn with_laser(spec: &str, on: bool) -> String {
    let (weapon, list) = spec.split_once(':').unwrap_or((spec, ""));
    let mut parts: Vec<&str> = list.split('+').filter(|a| !a.is_empty() && *a != ATTACHMENT).collect();
    if on {
        parts.push(ATTACHMENT);
    }
    format!("{weapon}:{}", parts.join("+"))
}

/// Maps dark enough that their soldiers wear night vision (bots put theirs
/// on there): Bog, and Wet Work's showcase night.
pub fn night_map(map: &str) -> bool {
    matches!(map, "mp_bog") || (map == "mp_cargoship" && crate::atmos::climate::showcase())
}

pub struct LaserPlugin;

impl Plugin for LaserPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Beams>()
            .init_resource::<UnitAssets>()
            .add_systems(Update, (mark.before(crate::weapons::WeaponSet), mount).chain().run_if(crate::state::in_game))
            .add_systems(
                PostUpdate,
                (rays, draw).after(bevy::transform::TransformSystems::Propagate).run_if(crate::state::in_game),
            )
            .add_systems(OnExit(crate::state::GameState::InGame), clear);
        if let Ok(dir) = std::env::var("COD4RW_LASERTEST") {
            app.insert_resource(TestDir(dir.into()))
                .add_systems(Update, test.after(crate::player::InputSet).before(crate::weapons::WeaponSet).run_if(crate::state::in_game))
                .add_systems(PostUpdate, test_close_up.before(bevy::transform::TransformSystems::Propagate).run_if(crate::state::in_game));
        }
    }
}

/// On a pawn: the gun in hand has a laser (it shines).
#[derive(Component, Clone, Copy, Debug)]
pub struct LaserSight;

/// On a pawn: laser fitted and seeing through night vision, so aiming is
/// the laser aim rather than the sights.
#[derive(Component, Clone, Copy, Debug)]
pub struct LaserMode;

/// On a bot: wearing its night vision (a night map).
#[derive(Component, Clone, Copy, Debug)]
pub struct BotNightVision;

/// Who has a laser shining, and who aims with it.
#[allow(clippy::type_complexity)]
fn mark(
    mut commands: Commands,
    time: Res<Time>,
    night: Option<Res<crate::vision::NightVisions>>,
    pawns: Query<(
        Entity,
        Option<&crate::loadout::Loadout>,
        Option<&crate::netplay::Puppet>,
        Option<&crate::splitscreen::LocalSlot>,
        Has<BotNightVision>,
        Has<LaserSight>,
        Has<LaserMode>,
        Has<Dead>,
        &crate::combat::Pawn,
        Option<&crate::weapons::WeaponState>,
    )>,
) {
    let now = time.elapsed_secs();
    for (e, loadout, puppet, slot, bot_nv, had_sight, had_mode, dead, pawn, weapon) in &pawns {
        let sight = !dead
            && match (loadout, puppet) {
                (Some(l), _) => l.extra_slot() != Some(l.current) && fitted(&l.gun().spec),
                (None, Some(p)) => fitted(p.weapon()),
                _ => false,
            };
        let looking = bot_nv || slot.zip(night.as_ref()).is_some_and(|(s, n)| n.get(s.0).showing(now));
        // A scoped gun keeps its scope: up at the cheek, the scope blocked
        // the view.
        let scoped = weapon.is_some_and(|w| w.def.ads_overlay.is_some());
        let mode = sight && looking && !scoped;
        if sight != had_sight {
            debug!("laser: {} ({e:?}) {}", pawn.name, if sight { "shines" } else { "off" });
            if sight {
                commands.entity(e).insert(LaserSight);
            } else {
                commands.entity(e).remove::<LaserSight>();
            }
        }
        if mode != had_mode {
            if mode {
                commands.entity(e).insert(LaserMode);
            } else {
                commands.entity(e).remove::<LaserMode>();
            }
        }
    }
}

/// The laser's look: CoD4's `gfx_laser` images.
struct Look {
    beam: Handle<StandardMaterial>,
    dot: Handle<StandardMaterial>,
    quad: Handle<Mesh>,
}

/// Each drawn laser (by pawn and viewer): its beam and its dot.
#[derive(Resource, Default)]
struct Beams {
    look: Option<Look>,
    /// Each laser's dot (where its shots go), for the test.
    dots: HashMap<Entity, Vec3>,
    drawn: HashMap<(Entity, usize), (Entity, Entity, Handle<Mesh>)>,
}

fn clear(mut commands: Commands, mut beams: ResMut<Beams>) {
    for (_, (beam, dot, _)) in beams.drawn.drain() {
        commands.entity(beam).try_despawn();
        commands.entity(dot).try_despawn();
    }
    beams.look = None;
}

fn knob(name: &str, default: f32) -> f32 {
    crate::tune::get(&format!("lasergun.{name}"), default)
}

/// Pieces a beam is drawn in (nearer ones shorter), so its width can
/// follow the distance from the eye.
const SEGMENTS: usize = 12;

/// A beam's strip: from `a` to `b`, `ra` and `rb` wide (half widths),
/// facing `eye`, bright `ba` at its start and `bb` at its end. Each point's
/// width is kept between `min` and `max` of its distance from the eye
/// (angles): never thinner than about a pixel far off, nor a fat bar right
/// at the eye.
#[allow(clippy::too_many_arguments)]
fn strip(a: Vec3, b: Vec3, ra: f32, rb: f32, ba: f32, bb: f32, eye: Vec3, min: f32, max: f32) -> (Vec<[f32; 3]>, Vec<[f32; 4]>) {
    let along = (b - a).normalize_or(Vec3::X);
    let mut positions = Vec::with_capacity(2 * (SEGMENTS + 1));
    let mut colors = Vec::with_capacity(2 * (SEGMENTS + 1));
    for i in 0..=SEGMENTS {
        // Squared: finer near the start, where the eye usually is.
        let t = (i as f32 / SEGMENTS as f32).powi(2);
        let p = a.lerp(b, t);
        let dist = eye.distance(p);
        let r = (ra + (rb - ra) * t).clamp(min * dist, (max * dist).max(min * dist));
        let side = along.cross(eye - p).normalize_or(Vec3::Y) * r;
        positions.push((p - side).into());
        positions.push((p + side).into());
        let k = ba + (bb - ba) * t;
        colors.push([k, k, k, k]);
        colors.push([k, k, k, k]);
    }
    (positions, colors)
}

fn beam_mesh() -> Mesh {
    let n = 2 * (SEGMENTS + 1);
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; n]);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 1.0, 0.0]; n]);
    // Across the beam, through the middle of the laser's (round) picture.
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, (0..n).map(|i| [(i % 2) as f32, 0.5]).collect::<Vec<[f32; 2]>>());
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0f32; 4]; n]);
    let indices = (0..SEGMENTS as u32).flat_map(|s| {
        let i = 2 * s;
        [i, i + 1, i + 2, i + 2, i + 1, i + 3]
    });
    m.insert_indices(Indices::U32(indices.collect()));
    m
}

/// The muzzle (`tag_laser`, else `tag_flash`) of a skeleton, as a point
/// and the barrel's way.
fn muzzle(skeleton: &Skeleton, globals: &Query<&GlobalTransform>) -> Option<(Vec3, Vec3)> {
    let tag = skeleton.joint("tag_laser").or_else(|| skeleton.joint("tag_flash"))?;
    let g = globals.get(tag).ok()?;
    Some((g.translation(), (g.rotation() * Vec3::X).normalize_or_zero()))
}

/// A gun's laser: out of its unit's emitter (the muzzle without one) along
/// the barrel (`tag_flash`'s forward).
fn gun_ray(owner: Entity, skeleton: &Skeleton, units: &Query<&Mounted>, globals: &Query<&GlobalTransform>) -> Option<(Vec3, Vec3)> {
    let (muzzle_at, barrel) = muzzle(skeleton, globals)?;
    let barrel = skeleton.joint("tag_flash").and_then(|t| globals.get(t).ok()).map_or(barrel, |g| (g.rotation() * Vec3::X).normalize_or(barrel));
    let from = units.get(owner).ok().and_then(|m| globals.get(m.emitter).ok()).map_or(muzzle_at, |g| g.translation());
    Some((from, barrel))
}

/// On a pawn with a laser: where it shines from and which way (its own
/// gun: the first-person one for a local player in first person). In the
/// laser aim its shots go along it ([`crate::weapons`]).
#[derive(Component, Clone, Copy, Debug)]
pub struct LaserRay {
    pub from: Vec3,
    pub dir: Vec3,
}

/// Each laser's ray, for the shots.
#[allow(clippy::type_complexity)]
fn rays(
    mut commands: Commands,
    third_person: Option<Res<crate::wardrobe::ThirdPerson>>,
    lasers: Query<(Entity, Option<&crate::thirdperson::Body>, Option<&crate::splitscreen::LocalSlot>), (With<LaserSight>, Without<Dead>)>,
    stale: Query<Entity, (With<LaserRay>, Or<(Without<LaserSight>, With<Dead>)>)>,
    viewmodels: Query<(Entity, &Skeleton, &crate::viewmodel::ViewModelSlot), With<crate::viewmodel::ViewModelRoot>>,
    skeletons: Query<&Skeleton>,
    units: Query<&Mounted>,
    globals: Query<&GlobalTransform>,
) {
    for e in &stale {
        commands.entity(e).remove::<LaserRay>();
    }
    for (pawn, body, slot) in &lasers {
        let first_person = slot.filter(|s| !third_person.as_ref().is_some_and(|t| t.on(s.0)));
        let gun = match first_person {
            Some(s) => viewmodels.iter().find(|(_, _, v)| v.0 == s.0).map(|(e, sk, _)| (e, sk)),
            None => body.and_then(|b| skeletons.get(b.0).ok().map(|sk| (b.0, sk))),
        };
        if let Some((from, dir)) = gun.and_then(|(owner, sk)| gun_ray(owner, sk, &units, &globals)) {
            commands.entity(pawn).insert(LaserRay { from, dir });
        }
    }
}

/// Draw every laser for every local player looking through night vision.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn draw(
    mut commands: Commands,
    time: Res<Time>,
    mut beams: ResMut<Beams>,
    content: Option<ResMut<crate::content::Content>>,
    (mut images, mut materials, mut meshes): (ResMut<Assets<Image>>, ResMut<Assets<StandardMaterial>>, ResMut<Assets<Mesh>>),
    night: Option<Res<crate::vision::NightVisions>>,
    third_person: Option<Res<crate::wardrobe::ThirdPerson>>,
    cameras: Query<(&crate::splitscreen::SlotCamera, &GlobalTransform)>,
    lasers: Query<(Entity, Option<&crate::thirdperson::Body>, Option<&crate::splitscreen::LocalSlot>), (With<LaserSight>, Without<Dead>)>,
    viewmodels: Query<(Entity, &Skeleton, &crate::viewmodel::ViewModelSlot), With<crate::viewmodel::ViewModelRoot>>,
    units: Query<&Mounted>,
    skeletons: Query<&Skeleton>,
    (globals, mut transforms, mut visibility): (Query<&GlobalTransform>, Query<&mut Transform>, Query<&mut Visibility>),
    hitboxes: Query<&Hitbox>,
    spatial: SpatialQuery,
) {
    let now = time.elapsed_secs();
    let viewers: Vec<(usize, Vec3)> = cameras
        .iter()
        .filter(|(s, _)| night.as_ref().is_some_and(|n| n.get(s.0).showing(now)))
        .map(|(s, g)| (s.0, g.translation()))
        .collect();
    let mut seen = Vec::new();
    if !viewers.is_empty() && !lasers.is_empty() {
        if beams.look.is_none() {
            let Some(mut content) = content else { return };
            let laser = content.image("laser", true, &mut images);
            let glow = |b: f32| Color::linear_rgb(b, b, b);
            let beam = materials.add(StandardMaterial {
                base_color: glow(knob("beam_bright", 14.0)),
                base_color_texture: laser.clone(),
                alpha_mode: AlphaMode::Add,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                fog_enabled: false,
                ..default()
            });
            let dot = materials.add(StandardMaterial {
                base_color: glow(knob("dot_bright", 8.0)),
                base_color_texture: laser,
                alpha_mode: AlphaMode::Add,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                fog_enabled: false,
                ..default()
            });
            let quad = meshes.add(Rectangle::new(1.0, 1.0));
            beams.look = Some(Look { beam, dot, quad });
        }
        let filter = crate::collision::bullet_filter();
        let split = crate::splitscreen::active();
        let (radius, flare, light) = (knob("radius", RADIUS), knob("flare_pct", FLARE_PCT), knob("dot_radius", LIGHT_RADIUS));
        for (pawn, body, slot) in &lasers {
            let range = u(RANGE);
            let own = |e: Entity| hitboxes.get(e).map_or(true, |h| h.owner != pawn);
            for &(viewer, eye) in &viewers {
                // The viewer's own gun in first person is the viewmodel's.
                let own_view = slot.is_some_and(|s| s.0 == viewer) && !third_person.as_ref().is_some_and(|t| t.on(viewer));
                // From the laser unit's emitter (the muzzle without one).
                let gun = if own_view {
                    viewmodels.iter().find(|(_, _, v)| v.0 == viewer).map(|(e, sk, _)| (e, sk))
                } else {
                    body.and_then(|b| skeletons.get(b.0).ok().map(|sk| (b.0, sk)))
                };
                let Some((owner, skeleton)) = gun else { continue };
                // Out of the unit along the gun's barrel (so it sways and
                // kicks with it), to whatever it meets.
                let Some((from, dir)) = gun_ray(owner, skeleton, &units, &globals) else { continue };
                let Ok(d) = Dir3::new(dir) else { continue };
                let hit = spatial.cast_ray_predicate(from, d, range, true, &filter, &own);
                let length = hit.map_or(range, |h| h.distance);
                let fraction = (length / range).min(1.0);
                if own_view || !beams.dots.contains_key(&pawn) {
                    beams.dots.insert(pawn, from + dir * length);
                }
                let to = from + dir * (length - u(END_OFFSET)).max(0.0);
                let widen = (eye.distance(to) / u(1.0)) * flare * 0.01 + 1.0;
                if std::env::var_os("COD4RW_LASERTEST").is_some() && (now * 2.0).fract() < time.delta_secs() * 2.0 {
                    info!("laser: {pawn:?} for {viewer} from {:?} to {:?} (hit {}), eye {:.1} m from the start", crate::units::to_cod(from).map(f32::round), crate::units::to_cod(to).map(f32::round), hit.is_some(), eye.distance(from));
                }
                let (min, max) = (knob("min_angle", 0.0006), knob("max_angle", 0.0015));
                let (positions, colors) = strip(from, to, u(radius), u(radius) * widen, 1.0, (1.0 - fraction).max(knob("end_floor", 0.5)), eye, min, max);

                let key = (pawn, viewer);
                let Some(look) = beams.look.as_ref() else { continue };
                let (beam_mat, dot_mat, quad) = (look.beam.clone(), look.dot.clone(), look.quad.clone());
                let (beam, dot, mesh) = beams.drawn.entry(key).or_insert_with(|| {
                    let mesh = meshes.add(beam_mesh());
                    let layers = if split { RenderLayers::layer(layer(viewer)) } else { RenderLayers::layer(0) };
                    let beam = commands
                        .spawn((Name::new("laser"), Mesh3d(mesh.clone()), MeshMaterial3d(beam_mat), Transform::default(), NoFrustumCulling, layers.clone(), bevy::light::NotShadowCaster))
                        .id();
                    let dot = commands
                        .spawn((Name::new("laser dot"), Mesh3d(quad), MeshMaterial3d(dot_mat), Transform::default(), NoFrustumCulling, layers, bevy::light::NotShadowCaster))
                        .id();
                    (beam, dot, mesh)
                });
                if let Some(mut m) = meshes.get_mut(&*mesh) {
                    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
                    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
                }
                // The glow where it lands, facing the viewer.
                let size = (u(light) * widen * 2.0).max(knob("dot_min_angle", 0.005) * eye.distance(to));
                let dot_tf = Transform::from_translation(to - dir * u(1.0)).looking_at(eye, Vec3::Y).with_scale(Vec3::splat(size));
                if let Ok(mut tf) = transforms.get_mut(*dot) {
                    *tf = dot_tf;
                }
                commands.entity(*dot).insert(GlobalTransform::from(dot_tf));
                let show = if hit.is_some() { Visibility::Inherited } else { Visibility::Hidden };
                if let Ok(mut v) = visibility.get_mut(*dot) {
                    v.set_if_neq(show);
                }
                if let Ok(mut v) = visibility.get_mut(*beam) {
                    v.set_if_neq(Visibility::Inherited);
                }
                seen.push(key);
            }
        }
    }
    if std::env::var_os("COD4RW_LASERTEST").is_some() && (now * 2.0).fract() < time.delta_secs() * 2.0 {
        info!("laser: {} viewers, {} shining, {} drawn", viewers.len(), lasers.iter().count(), seen.len());
    }
    // Lasers no longer shining (or no longer seen): gone.
    let gone: Vec<(Entity, usize)> = beams.drawn.keys().filter(|k| !seen.contains(k)).copied().collect();
    for k in gone {
        if let Some((beam, dot, mesh)) = beams.drawn.remove(&k) {
            commands.entity(beam).try_despawn();
            commands.entity(dot).try_despawn();
            meshes.remove(&mesh);
        }
    }
}

#[derive(Resource)]
struct TestDir(std::path::PathBuf);

/// Debug aid: with `COD4RW_LASERTEST=<dir>` (and `COD4RW_LASER=1`), night
/// vision goes on at 6 s; screenshots at the hip (8 s), laser-aimed
/// (10 s), of a bot's laser (`COD4RW_LASERTEST_BOT=1`: the nearest bot is
/// turned to face the player at 11 s, shot at 12 s), then without night
/// vision (14 s), and the game exits.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn test(
    mut commands: Commands,
    time: Res<Time>,
    dir: Res<TestDir>,
    mut night: Option<ResMut<crate::vision::NightVisions>>,
    mut player: Query<(&Transform, &mut crate::weapons::WeaponInput, &mut crate::movement::ViewAngles), With<crate::player::LocalPlayer>>,
    mut bots: Query<(&mut Transform, &mut crate::movement::ViewAngles), (With<crate::bots::Bot>, Without<crate::player::LocalPlayer>)>,
    mut step: Local<usize>,
    mut exit: MessageWriter<AppExit>,
    spatial: SpatialQuery,
    beams: Res<Beams>,
    mut shots: MessageReader<crate::weapons::ShotFired>,
    mut third_person: Option<ResMut<crate::wardrobe::ThirdPerson>>,
    mut switch: Query<&mut crate::loadout::SwitchInput, With<crate::player::LocalPlayer>>,
) {
    let t = time.elapsed_secs();
    const STEPS: [(f32, &str); 13] = [
        (6.0, "nv_on"),
        (8.0, "hip"),
        (9.85, "mid_burst"),
        (10.4, "laser_aim"),
        (12.0, "bot"),
        (12.8, "unit_close"),
        (13.0, "pistol"),
        (14.6, "unit_close_pistol"),
        (15.0, "third_person"),
        (15.8, "unit_world"),
        (16.2, "nv_off"),
        (17.5, "no_nv"),
        (18.5, "end"),
    ];
    let Some(&(at, name)) = STEPS.get(*step) else {
        exit.write(AppExit::Success);
        return;
    };
    let Ok((me, mut input, mut my_view)) = player.single_mut() else { return };
    // Shots against the dot: how far each lands from it.
    for s in shots.read() {
        if let Some(dot) = beams.dots.get(&s.shooter) {
            info!("laser test: a shot landed {:.1} units from the dot ({:.0} units away)", s.to.distance(*dot) / u(1.0), s.from.distance(s.to) / u(1.0));
        }
    }
    // The shots go into open ground, a few metres ahead.
    if (6.5..10.6).contains(&t) {
        my_view.pitch = -0.3;
    }
    // Aim from 9 s to the laser-aim shot; a few shots before the hip and
    // the laser-aim pictures (their marks should be on the dot).
    input.ads = (9.0..10.6).contains(&t);
    input.fire = (7.5..7.7).contains(&t) || (9.6..10.0).contains(&t);
    if std::env::var_os("COD4RW_LASERTEST_BOT").is_some() && t > 11.0 {
        let mine = me.translation;
        if let Some((mut tf, mut view)) = bots.iter_mut().min_by(|a, b| a.0.translation.distance(mine).total_cmp(&b.0.translation.distance(mine))) {
            // Brought 400 units in front of the player (where the view
            // looked at 11 s), facing it a little to one side.
            if t < 11.1 {
                let ahead = (my_view.rotation() * Vec3::NEG_Z).with_y(0.0).normalize_or(Vec3::NEG_Z);
                // Short of anything in the way.
                let eye = mine + Vec3::Y * u(50.0);
                let clear = Dir3::new(ahead).ok().and_then(|d| spatial.cast_ray(eye, d, u(400.0), true, &crate::collision::movement_filter())).map_or(u(400.0), |h| h.distance - u(40.0));
                tf.translation = mine + ahead * clear.max(u(120.0));
            }
            let to = mine - tf.translation;
            view.yaw = (-to.x).atan2(-to.z) + 0.15;
            view.pitch = 0.0;
            // And the player looks at it.
            my_view.yaw = (to.x).atan2(to.z);
            my_view.pitch = 0.0;
        }
    }
    if t < at {
        return;
    }
    *step += 1;
    match name {
        "end" => {}
        "pistol" => {
            if let Ok(mut s) = switch.single_mut() {
                s.to = Some(1);
            }
        }
        "third_person" => {
            if let Some(tp) = third_person.as_mut() {
                tp.0[0] = true;
            }
        }
        "nv_on" | "nv_off" => {
            if let Some(n) = night.as_mut() {
                n.0[0].switch(name == "nv_on", t);
            }
        }
        shot => {
            let path = dir.0.join(format!("laser_{shot}.png"));
            info!("laser test: {}", path.display());
            commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(path));
        }
    }
}

/// How far the laser aim's spread is from the hip's towards the sights'
/// (`lasergun.spread`).
pub fn spread_share() -> f32 {
    knob("spread", 0.55).clamp(0.0, 1.0)
}

/// How near the laser aim's walking speed is to the sights' (`lasergun.speed`).
pub fn speed_share() -> f32 {
    knob("speed", 0.85).clamp(0.0, 1.0)
}

/// How much of the sights' zoom the laser aim takes (`lasergun.zoom`).
pub fn zoom_share() -> f32 {
    knob("zoom", 0.15).clamp(0.0, 1.0)
}

/// The laser aim's pose: the gun up to the cheek and canted inward. CoD
/// angles in degrees (pitch down, yaw left, roll right side down) and a
/// shift in inches (forward, left, up), all at full laser aim.
pub fn pose() -> (Vec3, Vec3) {
    (
        Vec3::new(knob("pitch", -2.0), knob("yaw", 2.0), knob("roll", -38.0)),
        Vec3::new(knob("forward", -1.0), knob("left", 1.0), knob("up", 1.5)),
    )
}

/// Where the laser aim's cant turns about, in the view (CoD inches:
/// forward, left, up): about the gun's middle.
pub fn pivot() -> Vec3 {
    Vec3::new(knob("pivot_forward", 14.0), knob("pivot_left", -5.0), knob("pivot_up", -5.0))
}

// --- The laser unit on the gun.

/// CoD4's own IR laser unit: the box on the single-player M4s (Blackout's
/// `viewmodel_m4m203` and `weapon_m4gre_sp`, material
/// `mc/mtl_weapon_laser_point`), first-person and world, read from
/// `blackout.ff` once, in the background, the first time a laser is about.
/// Each: its mesh (the source gun's space), where its emitter is
/// (`tag_laser`) and how that sits from the source gun's `tag_flash`
/// (CoD inches: behind, to the left, below).
struct UnitModel {
    mesh: Mesh,
    emitter: Vec3,
    from_flash: Vec3,
    /// The box's extent about its emitter (CoD inches).
    lo: Vec3,
    hi: Vec3,
}

/// A gun's bind-pose vertex positions (from [`crate::gunmodel`]), on its
/// skeleton's owner: the laser unit is fitted against its side.
#[derive(Component, Clone)]
pub struct GunShape(pub std::sync::Arc<Vec<Vec3>>);

static UNITS: std::sync::OnceLock<Option<[UnitModel; 2]>> = std::sync::OnceLock::new();
static UNITS_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Which of the two: the first-person gun's, or a world gun's.
const VIEW: usize = 0;
const WORLD: usize = 1;

fn load_units() {
    if UNITS_STARTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(|| {
        let units = (|| {
            use iw3::zone::{Asset, ParseOptions, Zone};
            let install = iw3::Install::locate().ok()?;
            let zone = Zone::parse(&iw3::fastfile::load(&install.zone_path("blackout")).ok()?, ParseOptions::default()).ok()?;
            let unit = |model: &str| -> Option<UnitModel> {
                let id = zone.assets.iter().position(|a| matches!(a, Asset::XModel(m) if m.name == model))?;
                let x = zone.xmodel(id)?;
                let bone = |name: &str| x.bone_names.iter().position(|b| b == name).and_then(|i| x.base_mat.get(i)).map(|b| b.trans);
                let (laser, flash) = (bone("tag_laser")?, bone("tag_flash")?);
                // The first (most detailed) surface in the unit's material.
                let lod0 = x.lods.first().map_or(0..x.surfs.len(), |l| l.surf_index as usize..(l.surf_index as usize + l.num_surfs as usize));
                let surf = lod0.into_iter().find(|&i| {
                    x.materials.get(i).copied().flatten().and_then(|m| zone.material(m)).is_some_and(|m| m.name.ends_with("mtl_weapon_laser_point"))
                })?;
                let mesh = crate::content::surface_mesh(x, surf, false)?;
                let (mut lo, mut hi) = (Vec3::MAX, Vec3::MIN);
                if let Some(bevy::mesh::VertexAttributeValues::Float32x3(ps)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
                    for p in ps {
                        let c = Vec3::from_array(crate::units::to_cod(Vec3::from_array(*p))) - Vec3::from_array(laser);
                        lo = lo.min(c);
                        hi = hi.max(c);
                    }
                }
                Some(UnitModel {
                    mesh,
                    emitter: crate::units::pos(laser),
                    from_flash: Vec3::new(flash[0] - laser[0], laser[1] - flash[1], flash[2] - laser[2]),
                    lo,
                    hi,
                })
            };
            Some([unit("viewmodel_m4m203")?, unit("weapon_m4gre_sp")?])
        })();
        match &units {
            Some(u) => info!("laser: the laser unit read from blackout.ff (emitter {:.1} in behind the muzzle)", u[VIEW].from_flash.x),
            None => warn!("laser: no laser unit (blackout.ff's M4 couldn't be read); the beam comes from the muzzle"),
        }
        let _ = UNITS.set(units);
    });
}

/// On a gun's skeleton owner (a viewmodel, a body, a dropped gun): its
/// laser unit, under the gun's root, and the unit's emitter.
#[derive(Component)]
pub struct Mounted {
    flash: Entity,
    unit: Entity,
    pub emitter: Entity,
}

/// The unit's meshes and material, made once from [`UNITS`].
#[derive(Resource, Default)]
struct UnitAssets(Option<([Handle<Mesh>; 2], Handle<StandardMaterial>)>);

/// Put the laser unit on every gun that has the laser (the first-person
/// gun, the world guns in hand and dropped), and take it off guns that no
/// longer do. On the gun's own `tag_laser` where it has one (the M4s),
/// else as far behind its muzzle as on CoD4's M4.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn mount(
    mut commands: Commands,
    mut assets: ResMut<UnitAssets>,
    content: Option<ResMut<crate::content::Content>>,
    (mut images, mut materials, mut meshes): (ResMut<Assets<Image>>, ResMut<Assets<StandardMaterial>>, ResMut<Assets<Mesh>>),
    viewmodels: Query<(Entity, &Skeleton, &crate::viewmodel::ViewModelSlot), With<crate::viewmodel::ViewModelRoot>>,
    pawns: Query<(Option<&crate::thirdperson::Body>, Option<&crate::splitscreen::LocalSlot>), With<LaserSight>>,
    pickups: Query<(Entity, &crate::pickups::Pickup), With<Skeleton>>,
    skeletons: Query<&Skeleton>,
    mounted: Query<(Entity, &Mounted)>,
    (parents, transforms, children, layers): (Query<&ChildOf>, Query<&Transform>, Query<&Children>, Query<&RenderLayers, With<Mesh3d>>),
    (shapes, globals): (Query<&GunShape>, Query<&GlobalTransform>),
) {
    // Who wants one: (skeleton owner, first person?).
    let mut wanted: Vec<(Entity, bool)> = Vec::new();
    for (body, slot) in &pawns {
        if let Some(b) = body {
            wanted.push((b.0, false));
        }
        if let Some(s) = slot {
            if let Some((e, ..)) = viewmodels.iter().find(|(_, _, v)| v.0 == s.0) {
                wanted.push((e, true));
            }
        }
    }
    for (e, p) in &pickups {
        if fitted(&p.gun.spec) {
            wanted.push((e, false));
        }
    }
    // Off guns that no longer have it (or were rebuilt).
    for (owner, m) in &mounted {
        let flash = skeletons.get(owner).ok().and_then(|s| s.joint("tag_flash"));
        if !wanted.iter().any(|w| w.0 == owner) || flash != Some(m.flash) {
            commands.entity(m.unit).try_despawn();
            commands.entity(owner).remove::<Mounted>();
        }
    }
    if wanted.is_empty() {
        return;
    }
    load_units();
    let Some(Some(units)) = UNITS.get() else { return };
    if assets.0.is_none() {
        let Some(mut content) = content else { return };
        let texture = content.image("weapon_laser_point_col", true, &mut images);
        // (Both faces: on a gun's right side it's mirrored.)
        let material = materials.add(StandardMaterial { base_color_texture: texture, perceptual_roughness: 0.6, reflectance: 0.3, double_sided: true, cull_mode: None, ..default() });
        assets.0 = Some(([meshes.add(units[VIEW].mesh.clone()), meshes.add(units[WORLD].mesh.clone())], material));
    }
    let Some((unit_meshes, material)) = assets.0.clone() else { return };
    for (owner, first_person) in wanted {
        let Ok(skeleton) = skeletons.get(owner) else { continue };
        let Some(flash) = skeleton.joint("tag_flash") else { continue };
        if mounted.get(owner).is_ok_and(|(_, m)| m.flash == flash) {
            continue;
        }
        let Ok(root) = parents.get(flash).map(|p| p.parent()) else { continue };
        let Ok(flash_at) = transforms.get(flash).map(|t| t.translation) else { continue };
        let kind = if first_person { VIEW } else { WORLD };
        let unit = &units[kind];
        // Where the emitter goes, in the gun's root space: its own
        // `tag_laser`, else behind the muzzle as on the M4 (but not past
        // the middle of a short gun).
        let cod = |p: Vec3| Vec3::from_array(crate::units::to_cod(p));
        let f = cod(flash_at);
        // (A model whose muzzle isn't out ahead of it: no room for one; the
        // beam comes from the muzzle.)
        if f.x < 2.0 {
            continue;
        }
        // Along the gun: its own `tag_laser` (the M4s), else a hand's
        // width ahead of the magazine (on the handguard, near the body: on
        // a sniper rifle's long barrel it hung far out), else as far behind
        // the muzzle as on the M4; never in front of the muzzle's last 3
        // inches.
        let own_tag = skeleton.joint("tag_laser").and_then(|t| transforms.get(t).ok()).map(|t| cod(t.translation));
        let clip = skeleton.joint("tag_clip").and_then(|t| transforms.get(t).ok()).map(|t| cod(t.translation));
        let (x, z) = match own_tag {
            Some(t) => (t.x, t.z),
            None => {
                let x = clip.map_or(f.x - unit.from_flash.x, |c| c.x + knob("mount_ahead", 10.0));
                (x.min(f.x - 3.0).max(f.x * 0.3), f.z - unit.from_flash.z)
            }
        };
        // A smaller one on a short gun (a pistol): the rifle's box dwarfed it.
        let size = (f.x / knob("full_size_length", 16.0)).clamp(0.5, 1.0);
        let (lo, hi) = (unit.lo * size, unit.hi * size);
        // Across: against the gun's side where the box sits (its widest
        // there), from the gun's own vertices.
        let span = |x: f32| (x + lo.x - 0.5)..(x + hi.x + 0.5);
        let span_z = (z + lo.z)..(z + hi.z);
        let sides = |x: f32| {
            let (mut left, mut right) = (f32::MIN, f32::MAX);
            if let Ok(shape) = shapes.get(owner) {
                for p in shape.0.iter() {
                    let c = cod(*p);
                    if span(x).contains(&c.x) && span_z.contains(&c.z) {
                        left = left.max(c.y);
                        right = right.min(c.y);
                    }
                }
            }
            if left == f32::MIN { (f.y + unit.from_flash.y + lo.y, f.y - unit.from_flash.y - lo.y) } else { (left, right) }
        };
        // The left side, clear of the support hand (first person): moved
        // ahead of it if there's room before the muzzle, else on the right.
        let hand = first_person
            .then(|| skeleton.joint("j_wrist_le").and_then(|w| globals.get(w).ok()).zip(globals.get(root).ok()))
            .flatten()
            .map(|(w, r)| cod(r.affine().inverse().transform_point3(w.translation())));
        let covered = |x: f32| hand.is_some_and(|h| (span(x).start - 1.5..span(x).end + 1.5).contains(&h.x));
        let mut x = x;
        let mut mirrored = false;
        if covered(x) {
            let ahead = hand.map_or(x, |h| h.x + 2.5 - lo.x);
            if ahead <= f.x - 3.0 && !covered(ahead) {
                x = ahead;
            } else {
                mirrored = true;
            }
        }
        let (left, right) = sides(x);
        let gap = 0.03;
        let y = if mirrored { right - gap + lo.y } else { left + gap - lo.y };
        let mount_at = crate::units::pos([x, y, z]);
        let flip = if mirrored { Vec3::new(1.0, 1.0, -1.0) } else { Vec3::ONE } * size;
        // On the same layers as the gun's own surfaces.
        let layer = children
            .iter_descendants(root)
            .find_map(|c| layers.get(c).ok().cloned())
            .or_else(|| children.iter_descendants(owner).find_map(|c| layers.get(c).ok().cloned()));
        let mut e = commands.spawn((
            Name::new("laser unit"),
            Mesh3d(unit_meshes[kind].clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(mount_at - flip * unit.emitter).with_scale(flip),
            ChildOf(root),
        ));
        info!(
            "laser: unit on {owner:?} ({}, {} side{}), emitter at CoD {:?} in the gun (muzzle {:?}), layers {:?}",
            if first_person { "first person" } else { "world" },
            if mirrored { "right" } else { "left" },
            hand.map_or(String::new(), |h| format!(", hand at {:.1}", h.x)),
            crate::units::to_cod(mount_at).map(|v| (v * 10.0).round() / 10.0),
            crate::units::to_cod(flash_at).map(|v| (v * 10.0).round() / 10.0),
            layer
        );
        if let Some(l) = layer {
            e.insert(l);
        }
        let unit_e = e.id();
        let emitter = commands.spawn((Name::new("laser emitter"), Transform::from_translation(unit.emitter), Visibility::default(), ChildOf(unit_e))).id();
        commands.entity(owner).insert(Mounted { flash, unit: unit_e, emitter });
    }
}

/// The test's close-up of the laser unit on the first-person gun (12.4 s
/// to 12.9 s): the gun's camera turned to it and zoomed in.
fn test_close_up(
    time: Res<Time>,
    viewmodels: Query<&Mounted, With<crate::viewmodel::ViewModelRoot>>,
    globals: Query<&GlobalTransform>,
    mut cameras: Query<(&crate::splitscreen::SlotViewModelCamera, &ChildOf, &mut Transform, &mut Projection)>,
) {
    let t = time.elapsed_secs();
    let close = (12.4..12.9).contains(&t) || (14.2..14.7).contains(&t);
    for (slot, parent, mut tf, mut projection) in &mut cameras {
        if slot.0 != 0 {
            continue;
        }
        let target = viewmodels.iter().next().and_then(|m| globals.get(m.emitter).ok()).map(|g| g.translation());
        match (close, target, globals.get(parent.parent())) {
            (true, Some(at), Ok(world)) => {
                let local = world.affine().inverse().transform_point3(at);
                *tf = Transform::default().looking_at(local, Vec3::Y);
                if let Projection::Perspective(p) = projection.as_mut() {
                    p.fov = 22f32.to_radians();
                }
            }
            _ => {
                if tf.rotation != Quat::IDENTITY {
                    *tf = Transform::default();
                }
            }
        }
    }
}
