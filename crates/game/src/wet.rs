//! Wet world surfaces for the showcase (`COD4RW_SHOWCASE=1`): what
//! `shaders/world.wgsl`'s wet shading reads.
//!
//! - Each world mesh's `MeshTag` carries the weather: bits 0-7 how wet
//!   (`Weather::wetness`), 8-15 how hard it rains, bit 16 the showcase is
//!   on, 24-31 the sky's light against CoD4's hour
//!   (`climate::light_share`, for the reflection probes). Only the tags change as the weather does, never the materials.
//! - [`WetMap`]: where rain reaches, from above, around the camera, for
//!   the shader (dry under roofs and cover): `crate::weather::occlusion`'s
//!   heights copied into a filterable half-float image every world
//!   material binds. The image keeps its GPU texture as it updates, so the
//!   materials' bind groups stay as they are.
//!
//! `COD4RW_WETNESS=<0..1>` sets how wet (for test shots).

use crate::atmos::climate::Weather;
use crate::weather::occlusion::{CELLS, RainMap, UNKNOWN};
use crate::world::WorldMaterial;
use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::sync::OnceLock;

pub struct WetPlugin;

impl Plugin for WetPlugin {
    fn build(&self, app: &mut App) {
        let image = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            images.add(blank_map())
        };
        WET_MAP.set(image).ok();
        if crate::atmos::climate::enabled() {
            app.add_systems(Update, (copy_rain_map, tag_world).run_if(crate::state::in_game).run_if(crate::atmos::climate::on));
        }
    }
}

static WET_MAP: OnceLock<Handle<Image>> = OnceLock::new();

/// A static model's material as a world one that can be wet: its own
/// standard material under the world shader with nothing else of the
/// world's (no lightmap, maps or probe), so it's lit as before; wet, it
/// reflects the sky (`sky`, flagged in `probe.z`).
pub fn wet_material(base: Option<&StandardMaterial>, sky: Option<Handle<Image>>, world_materials: &mut Assets<WorldMaterial>) -> Handle<WorldMaterial> {
    let params = crate::world::WorldParams { probe: Vec4::new(0.0, 0.0, sky.is_some() as u32 as f32, 0.0), ..default() };
    world_materials.add(WorldMaterial {
        base: base.cloned().unwrap_or_default(),
        extension: crate::world::WorldLighting { params, reflection_probe: sky, wet_map: wet_map(), ssr_history: crate::ssr::history(), ..default() },
    })
}

/// The wet map every world material binds (`WorldLighting::wet_map`).
pub fn wet_map() -> Option<Handle<Image>> {
    WET_MAP.get().cloned()
}

/// A height that means open sky (as `occlusion::UNKNOWN`, within half
/// floats' range).
const OPEN: f32 = -60000.0;

fn blank_map() -> Image {
    let open = half::f16::from_f32(OPEN).to_le_bytes();
    let mut image = Image::new(
        Extent3d { width: CELLS as u32, height: CELLS as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        open.repeat(CELLS * CELLS),
        TextureFormat::R16Float,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    image.sampler = ImageSampler::nearest();
    image
}

/// The rain map's heights into ours, as they change.
fn copy_rain_map(rain: Option<Res<RainMap>>, mut images: ResMut<Assets<Image>>) {
    let Some(rain) = rain else { return };
    let Some(target) = WET_MAP.get() else { return };
    if !rain.is_changed() {
        return;
    }
    let Some(src) = images.get(&rain.image).and_then(|i| i.data.as_ref()) else { return };
    let out: Vec<u8> = src
        .chunks_exact(4)
        .flat_map(|px| {
            let h = f32::from_le_bytes([px[0], px[1], px[2], px[3]]);
            half::f16::from_f32(if h <= UNKNOWN { OPEN } else { h }).to_le_bytes()
        })
        .collect();
    if let Some(mut image) = images.get_mut(target) {
        image.data = Some(out);
    }
}

/// How wet and how hard it rains, packed for the world meshes' tags.
fn tag_world(
    mut commands: Commands,
    weather: Option<Res<Weather>>,
    meshes: Query<(Entity, Option<&MeshTag>), With<MeshMaterial3d<WorldMaterial>>>,
) {
    let (mut wet, mut rain) = weather.as_deref().filter(|w| w.enabled).map_or((0.0, 0.0), |w| (w.wetness, w.rain));
    if let Some(v) = std::env::var("COD4RW_WETNESS").ok().and_then(|v| v.parse::<f32>().ok()) {
        wet = v;
        rain = rain.max(v);
    }
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    // Bits 24-31: the sky's light now against CoD4's own hour, which the
    // reflection probes were baked at (dim at night).
    let tag = byte(wet) | byte(rain) << 8 | 1 << 16 | byte(crate::atmos::climate::light_share()) << 24;
    for (e, current) in &meshes {
        if current.map(|t| t.0) != Some(tag) {
            commands.entity(e).insert(MeshTag(tag));
        }
    }
}
