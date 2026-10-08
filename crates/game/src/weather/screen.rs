//! Rain on the lens: out in the rain and looking up, drops land on the
//! screen, hang, run down in fits and starts and dry away. Each is a small
//! quad just in front of the viewmodel camera (drawn last, over
//! everything): a drop picture, darker round its rim and catching a
//! highlight, as water on glass does.

use super::Storm;
use super::occlusion::RainMap;
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

pub(super) fn build(app: &mut App) {
    app.add_systems(Update, drops.run_if(crate::state::in_game).run_if(crate::atmos::climate::on));
}

/// Drops on the lens at most.
const MAX_DROPS: usize = 48;
/// New drops a second at full rain, looking straight up.
const RATE: f32 = 9.0;
/// How far in front of the camera they're drawn (metres), within its near
/// plane's reach.
const DEPTH: f32 = 0.05;

#[derive(Component)]
struct Drop {
    /// Where on the screen (-1..1, y up), how big (share of the height).
    at: Vec2,
    size: f32,
    age: f32,
    life: f32,
    /// Running down: speed (screen heights a second), until the next stop.
    run: f32,
    until: f32,
}

#[derive(Default)]
struct Pictures {
    mesh: Option<Handle<Mesh>>,
    /// One per shape in the picture.
    materials: Vec<Handle<StandardMaterial>>,
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The drops' pictures: water on glass, mostly clear. No outline: a soft
/// faint darkening toward one side (where it bends the light away), a
/// faint glint on the other, an irregular edge; a short wet trail above
/// for the running ones. Four shapes side by side (`SHAPES`).
const SHAPES: u32 = 4;
fn picture() -> Image {
    const N: u32 = 64;
    let w = N * SHAPES;
    let mut px = Vec::with_capacity((w * N * 4) as usize);
    let noise = |p: Vec2, k: u32| -> f32 {
        let h = |q: Vec2| {
            let v = (q.x * 127.1 + q.y * 311.7 + k as f32 * 74.7).sin() * 43_758.547;
            v - v.floor()
        };
        let c = p.floor();
        let f = p - c;
        let u = f * f * (Vec2::splat(3.0) - 2.0 * f);
        let (a, b, d, e) = (h(c), h(c + Vec2::X), h(c + Vec2::Y), h(c + Vec2::ONE));
        a + (b - a) * u.x + (d - a) * u.y + (a - b - d + e) * u.x * u.y
    };
    for y in 0..N {
        for x in 0..w {
            let k = x / N;
            let p = Vec2::new((x % N) as f32 + 0.5, y as f32 + 0.5) / N as f32 * 2.0 - 1.0;
            // An irregular blob in the lower half; a trail above it.
            let q = Vec2::new(p.x, (p.y - 0.35) * 1.4);
            let wobble = (noise(p * 3.0, k) - 0.5) * 0.35;
            let r = q.length() * 1.6 + wobble;
            let body = 1.0 - smoothstep(0.75, 1.0, r);
            let trail = (1.0 - smoothstep(0.05, 0.18 + 0.1 * noise(Vec2::new(0.0, p.y * 4.0), k + 7), p.x.abs())) * smoothstep(0.3, -0.9, p.y) * 0.35;
            let a = body.max(trail);
            // Darker toward the lower right, a glint up left (refraction).
            let lean = (q.x * 0.6 + q.y * 0.4).clamp(-1.0, 1.0);
            let shade = 0.35 + 0.25 * -lean + 0.6 * (1.0 - smoothstep(0.0, 0.25, (q - Vec2::new(-0.2, -0.25)).length())) * body;
            let alpha = a * (0.12 + 0.18 * lean.abs());
            let v = (shade.clamp(0.0, 1.0) * 255.0) as u8;
            px.extend([v, v, v, (alpha.clamp(0.0, 1.0) * 255.0) as u8]);
        }
    }
    Image::new(
        Extent3d { width: N * SHAPES, height: N, depth_or_array_layers: 1 },
        TextureDimension::D2,
        px,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

#[allow(clippy::too_many_arguments)]
fn drops(
    mut commands: Commands,
    time: Res<Time>,
    storm: Res<Storm>,
    map: Option<Res<RainMap>>,
    cameras: Query<(Entity, &GlobalTransform, &Projection), With<crate::player::ViewModelCamera>>,
    mut drops: Query<(Entity, &mut Drop, &mut Transform)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut pictures: Local<Pictures>,
    mut due: Local<f32>,
    mut seed: Local<u32>,
) {
    let Some((camera, tf, projection)) = cameras.iter().next() else { return };
    let dt = time.delta_secs();
    let eye = tf.translation();
    // Out in the rain, looking up: drops land, more the higher the look.
    let up = tf.forward().y;
    let exposed = map.as_ref().and_then(|m| m.exposed(eye)).unwrap_or(false);
    let rate = if exposed { RATE * storm.rain * ((up - 0.1) / 0.9).clamp(0.0, 1.0) } else { 0.0 };
    *due += rate * dt;
    let mesh = pictures.mesh.get_or_insert_with(|| meshes.add(Rectangle::new(1.0, 1.0))).clone();
    if pictures.materials.is_empty() {
        let image = images.add(picture());
        pictures.materials = (0..SHAPES)
            .map(|k| {
                materials.add(StandardMaterial {
                    base_color_texture: Some(image.clone()),
                    uv_transform: bevy::math::Affine2::from_scale_angle_translation(Vec2::new(1.0 / SHAPES as f32, 1.0), 0.0, Vec2::new(k as f32 / SHAPES as f32, 0.0)),
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                })
            })
            .collect();
    }
    let mut rand = || {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (*seed >> 8) as f32 / (1u32 << 24) as f32
    };
    let mut count = drops.iter().len();
    while *due >= 1.0 {
        *due -= 1.0;
        if count >= MAX_DROPS {
            continue;
        }
        count += 1;
        let d = Drop {
            at: Vec2::new(rand() * 2.0 - 1.0, rand() * 2.0 - 1.0),
            // Mostly small; a big one now and then.
            size: 0.008 + 0.03 * rand() * rand() * rand(),
            age: 0.0,
            life: 0.6 + 1.4 * rand(),
            run: 0.0,
            until: 0.4 + rand(),
        };
        commands.spawn((
            Name::new("lens drop"),
            d,
            Mesh3d(mesh.clone()),
            MeshMaterial3d(pictures.materials[(rand() * SHAPES as f32) as usize % SHAPES as usize].clone()),
            Transform::default(),
            RenderLayers::layer(crate::player::VIEWMODEL_LAYER),
            bevy::light::NotShadowCaster,
            ChildOf(camera),
        ));
    }
    // The screen's half size at the drops' depth.
    let fov = match projection {
        Projection::Perspective(p) => p.fov,
        _ => 1.0,
    };
    let half_h = DEPTH * (fov * 0.5).tan();
    let aspect = match projection {
        Projection::Perspective(p) => p.aspect_ratio,
        _ => 16.0 / 9.0,
    };
    for (e, mut d, mut t) in &mut drops {
        d.age += dt;
        if d.age >= d.life {
            commands.entity(e).despawn();
            continue;
        }
        // Bigger drops run; they stop and go.
        if d.size > 0.02 {
            d.until -= dt;
            if d.until <= 0.0 {
                d.run = if d.run > 0.0 { 0.0 } else { 0.08 + 0.25 * rand() };
                d.until = 0.2 + 0.8 * rand();
            }
            d.at.y -= d.run * dt * 2.0;
        }
        // Drying: shrinking at the end.
        let dry = 1.0 - ((d.age - d.life * 0.7) / (d.life * 0.3)).clamp(0.0, 1.0);
        let s = d.size * 2.0 * half_h * dry.max(0.05);
        t.translation = Vec3::new(d.at.x * half_h * aspect, d.at.y * half_h, -DEPTH);
        t.scale = Vec3::new(s, s * 1.15, 1.0);
    }
}
