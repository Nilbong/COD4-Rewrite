//! The dynamic sky (`r_sky`): a cube map we draw ourselves (`sky.wgsl`) in
//! place of the map's skybox image, so every camera's skybox (splitscreen,
//! the scope, the killcam) shows it with no pass of its own. A quarter of
//! one face is redrawn a frame (all six at the start), so drifting clouds
//! cost a 24th of a full redraw.
//!
//! What it takes from the map: its skybox's colours (blurred), its fog
//! colour at the horizon, the sun's direction and colour, and how cloudy
//! it is ([`cloud_cover`]: overcast Crash and Overgrown, clear Backlot;
//! other maps guess from how grey their skybox is). The distance fog
//! glows toward the sun as the sky does.

use super::setting;
use crate::player::MainCamera;
use bevy::asset::RenderAssetUsages;
use bevy::core_pipeline::schedule::camera_driver;
use bevy::light::Skybox;
use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::binding_types::{sampler, texture_3d, texture_cube, texture_storage_2d_array, uniform_buffer};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, RenderGraph, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

pub struct SkyPlugin;

impl Plugin for SkyPlugin {
    fn build(&self, app: &mut App) {
        let registry = app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>();
        registry.insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("sky.wgsl"),
            std::path::Path::new("cod4rw/sky.wgsl"),
            include_bytes!("sky.wgsl").as_slice(),
        );
        app.add_plugins(ExtractResourcePlugin::<DynamicSky>::default())
            .add_systems(Update, (create, update, swap, sun_fog).chain().run_if(crate::state::in_game));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare.in_set(RenderSystems::PrepareBindGroups))
            .add_systems(RenderGraph, draw.in_set(bevy::render::renderer::RenderGraphSystems::Render).before(camera_driver));
    }
}

/// Cube map face size: the sun's disc is a few texels across.
const SIZE: u32 = 1024;
/// A face is redrawn in this many strips, one a frame (96 frames for the
/// whole sky: clouds drift a few metres in that time).
const STRIPS: u32 = 16;

/// The dynamic sky's cube map, the classic one it's drawn from, and what
/// to draw this frame.
#[derive(Resource, Clone, ExtractResource)]
pub struct DynamicSky {
    image: Handle<Image>,
    noise: Handle<Image>,
    classic: Handle<Image>,
    params: SkyParams,
    /// Draw this frame (the setting is on).
    on: bool,
    /// Faces to draw (from `params.face_base`) and rows of each (from
    /// `params.row_base`).
    faces: u32,
    rows: u32,
    frames: u32,
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
struct SkyParams {
    sun_dir: Vec3,
    sun_size: f32,
    sun_color: Vec3,
    coverage: f32,
    horizon: Vec3,
    fog: f32,
    wind: Vec2,
    darkness: f32,
    face_base: u32,
    row_base: u32,
    moon_dir: Vec3,
    night: f32,
    bolt_dir: Vec3,
    flash: f32,
    key_dir: Vec3,
    physical: f32,
    key_color: Vec3,
    cloud_extra: f32,
    time: f32,
    bolt_seed: f32,
}

/// How cloudy each map is (cover 0..1, how dark the undersides): CoD4's
/// skies for them, overcast or clear. `None`: guessed from the skybox.
fn cloud_cover(map: &str) -> Option<(f32, f32)> {
    Some(match map {
        "mp_crash" | "mp_crash_snow" => (0.72, 0.85),
        "mp_overgrown" => (0.72, 0.7),
        "mp_bloc" | "mp_farm" | "mp_cargoship" => (0.85, 0.6),
        "mp_backlot" | "mp_strike" | "mp_citystreets" | "mp_convoy" | "mp_showdown" => (0.3, 0.0),
        "mp_bog" => (0.55, 0.4),
        // Night maps: broken cloud, so their night sky shows between.
        "mp_carentan" | "mp_vacant" => (0.45, 0.5),
        _ => return None,
    })
}

/// Whether the dynamic sky is chosen.
fn dynamic(settings: &crate::settings::Settings) -> bool {
    // (The showcase's day and night need ours.)
    super::climate::showcase() || setting(settings, "r_sky", "COD4RW_SKY") == "dynamic"
}

fn create(mut commands: Commands, classic: Option<Res<crate::world::MapSky>>, sky: Option<Res<DynamicSky>>, mut images: ResMut<Assets<Image>>) {
    let Some(classic) = classic else { return };
    if sky.is_some_and(|s| s.classic == classic.0) {
        return;
    }
    let mut image = Image::new_fill(
        Extent3d { width: SIZE, height: SIZE, depth_or_array_layers: 6 },
        TextureDimension::D2,
        &[0; 8],
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..default() });
    image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING | TextureUsages::COPY_DST;
    commands.insert_resource(DynamicSky {
        image: images.add(image),
        noise: images.add(noise_image()),
        classic: classic.0.clone(),
        params: SkyParams::default(),
        on: false,
        faces: 6,
        rows: SIZE,
        frames: 0,
    });
}

/// Texels across the clouds' noise texture, and lattice cells across it.
const NOISE_SIZE: u32 = 128;
const NOISE_CELLS: u32 = 32;

/// Smooth, tiling 3D value noise for the clouds (sampled with wrapping).
fn noise_image() -> Image {
    let hash = |x: u32, y: u32, z: u32| {
        let (x, y, z) = (x % NOISE_CELLS, y % NOISE_CELLS, z % NOISE_CELLS);
        let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ z.wrapping_mul(0xcb1a_b31f);
        h ^= h >> 13;
        h = h.wrapping_mul(0x5bd1_e995);
        h ^= h >> 15;
        (h & 0xffff) as f32 / 65535.0
    };
    let per = (NOISE_SIZE / NOISE_CELLS) as f32;
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let mut data = Vec::with_capacity((NOISE_SIZE * NOISE_SIZE * NOISE_SIZE) as usize);
    for z in 0..NOISE_SIZE {
        for y in 0..NOISE_SIZE {
            for x in 0..NOISE_SIZE {
                let p = Vec3::new(x as f32, y as f32, z as f32) / per;
                let (i, f) = (p.floor(), p - p.floor());
                let (i, u) = ((i.x as u32, i.y as u32, i.z as u32), Vec3::new(smooth(f.x), smooth(f.y), smooth(f.z)));
                let corner = |dx, dy, dz| hash(i.0 + dx, i.1 + dy, i.2 + dz);
                let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
                let a = lerp(corner(0, 0, 0), corner(1, 0, 0), u.x);
                let b = lerp(corner(0, 1, 0), corner(1, 1, 0), u.x);
                let c = lerp(corner(0, 0, 1), corner(1, 0, 1), u.x);
                let d = lerp(corner(0, 1, 1), corner(1, 1, 1), u.x);
                let v = lerp(lerp(a, b, u.y), lerp(c, d, u.y), u.z);
                data.push((v * 255.0).round() as u8);
            }
        }
    }
    let mut image = Image::new(
        Extent3d { width: NOISE_SIZE, height: NOISE_SIZE, depth_or_array_layers: NOISE_SIZE },
        TextureDimension::D3,
        data,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::Repeat,
        address_mode_v: bevy::image::ImageAddressMode::Repeat,
        address_mode_w: bevy::image::ImageAddressMode::Repeat,
        mag_filter: bevy::image::ImageFilterMode::Linear,
        min_filter: bevy::image::ImageFilterMode::Linear,
        ..default()
    });
    image
}

fn update(
    mut sky: Option<ResMut<DynamicSky>>,
    settings: Res<crate::settings::Settings>,
    time: Res<Time>,
    map: Option<Res<crate::world::MapName>>,
    fog: Option<Res<crate::fog::MapFog>>,
    suns: Query<(&DirectionalLight, &GlobalTransform), (Without<crate::model_lighting::ViewModelSun>, Without<Camera>)>,
    camera: Query<(&Skybox, Option<&bevy::camera::Exposure>), With<MainCamera>>,
    tod: Res<super::climate::TimeOfDay>,
    weather: Res<super::climate::Weather>,
    mut bolts: Local<u32>,
) {
    let Some(sky) = sky.as_mut() else { return };
    let on = dynamic(&settings);
    if on != sky.on {
        info!("sky: {}", if on { "dynamic" } else { "classic" });
        sky.frames = 0;
    }
    sky.on = on;
    if !on {
        return;
    }
    let Some((light, sun_tf)) = suns.iter().find(|(l, _)| l.shadow_maps_enabled).or_else(|| suns.iter().find(|(l, _)| l.illuminance > 0.0)) else { return };
    let Ok((skybox, exposure)) = camera.single() else { return };
    let c = light.color.to_linear();
    let sun_color = Vec3::new(c.red, c.green, c.blue);
    // The skybox draws a texel of 1 at `brightness * exposure`; the fog's
    // colour is drawn as it is.
    let scale = skybox.brightness * exposure.copied().unwrap_or_default().exposure();
    let (horizon, has_fog) = super::map_fog(fog.as_deref()).map_or((Vec3::ZERO, 0.0), |(c, _)| (c / scale.max(1e-6), 1.0));
    // The showcase's horizon is the clock's (the fog's, the ocean's).
    let (horizon, has_fog) = if tod.enabled { (tod.horizon / scale.max(1e-6), 1.0) } else { (horizon, has_fog) };
    let (mut coverage, mut darkness) = map.as_ref().and_then(|m| cloud_cover(&m.0)).unwrap_or((-1.0, 0.3));
    // The showcase: the clock's sun and moon light the sky and clouds, the
    // weather sets the cover; storms tower and darken; lightning flashes.
    let showcase = tod.enabled;
    let storm = if weather.enabled { (weather.rain / 0.8).clamp(0.0, 1.0) } else { 0.0 };
    if weather.enabled {
        coverage = weather.cloud_cover;
        darkness = darkness.max(0.3 + 0.6 * storm);
    }
    let (key_dir, key_color) = if !showcase {
        (-sun_tf.forward().as_vec3(), sun_color / sun_color.max_element().max(1e-3))
    } else if tod.sun_illuminance >= tod.moon_illuminance {
        let full = tod.map.map_or(1.0, |m| m.illuminance.max(1.0));
        (tod.sun_dir, tod.sun_color * (tod.sun_illuminance / full).min(1.3))
    } else {
        let full = tod.map.map_or(1.0, |m| m.illuminance.max(1.0));
        (tod.moon_dir, tod.moon_color * (tod.moon_illuminance / full) * 2.0)
    };
    let flash = weather.lightning.map_or(0.0, |l| l.flash);
    if flash > 0.0 && sky.params.flash == 0.0 {
        *bolts += 1;
    }
    // Clouds drift at ~5 m/s.
    // (Faster in the showcase's storm wind.)
    let speed = if weather.enabled { 5.0 + weather.wind.length() } else { 5.0 };
    let drift = time.elapsed_secs() * speed;
    let slice = sky.frames % (6 * STRIPS);
    // A lightning flash is quick: the whole sky every frame while it lasts
    // (and the frame after, to clear it).
    let flashing = flash > 0.0 || sky.params.flash > 0.0;
    // (A flash redraws the face the bolt is on, whole, each frame.)
    let bolt = weather.lightning.map_or(Vec3::Y, |l| l.dir);
    let (faces, rows, face_base, row_base) = if sky.frames == 0 {
        (6, SIZE, 0, 0)
    } else if flashing {
        // (All of it, at half the steps: a flash lights the clouds every
        // way, and one face alone left a hard-edged square in the sky.)
        // Half the faces a frame (a flash is ~0.6 s: 2 frames behind is unseen).
        let _ = bolt;
        (3, SIZE, (sky.frames % 2) * 3, 0)
    } else {
        (1, SIZE / STRIPS, slice / STRIPS, slice % STRIPS * SIZE / STRIPS)
    };
    sky.faces = faces;
    sky.rows = rows;
    sky.params = SkyParams {
        sun_dir: if showcase { tod.sun_dir } else { -sun_tf.forward().as_vec3() },
        // Twice the real sun's, as games draw it.
        sun_size: 0.0093,
        sun_color: if showcase { tod.sun_color } else { sun_color / sun_color.max_element().max(1e-3) },
        coverage,
        horizon,
        fog: has_fog,
        wind: Vec2::new(drift, drift * 0.35),
        darkness,
        face_base,
        row_base,
        moon_dir: tod.moon_dir,
        night: if showcase { tod.night } else { 0.0 },
        bolt_dir: weather.lightning.map_or(Vec3::Y, |l| l.dir),
        flash,
        key_dir,
        physical: if showcase { 1.0 } else { 0.0 },
        key_color,
        cloud_extra: 3400.0 * storm,
        time: time.elapsed_secs(),
        bolt_seed: *bolts as f32,
    };
    sky.frames += 1;
}

/// The cameras' skybox: ours or the map's (cameras showing either).
fn swap(sky: Option<Res<DynamicSky>>, mut skyboxes: Query<&mut Skybox, With<Camera>>) {
    let Some(sky) = sky else { return };
    let want = if sky.on { &sky.image } else { &sky.classic };
    for mut s in &mut skyboxes {
        let ours = s.image.as_ref().is_some_and(|i| *i == sky.image || *i == sky.classic);
        if ours && s.image.as_ref() != Some(want) {
            s.image = Some(want.clone());
        }
    }
}

/// With the dynamic sky, the distance fog glows toward the sun as the sky
/// does round it.
fn sun_fog(sky: Option<Res<DynamicSky>>, suns: Query<&DirectionalLight, Without<crate::model_lighting::ViewModelSun>>, mut fogs: Query<&mut DistanceFog>) {
    let Some(sky) = sky else { return };
    let tint = suns.iter().find(|l| l.shadow_maps_enabled).map_or(Vec3::ZERO, |l| {
        let c = l.color.to_linear();
        Vec3::new(c.red, c.green, c.blue)
    });
    // A share of the sun's light (`directional_light_color` scales it).
    let want = if sky.on { Color::linear_rgba(tint.x, tint.y, tint.z, 0.012) } else { Color::NONE };
    for mut f in &mut fogs {
        if f.directional_light_color != want {
            f.directional_light_color = want;
            f.directional_light_exponent = 12.0;
        }
    }
}

// --- render world

#[derive(Resource)]
struct SkyPipeline {
    layout: BindGroupLayoutDescriptor,
    pipeline: CachedComputePipelineId,
    sampler: Sampler,
}

#[derive(Resource)]
struct SkyBindGroup {
    group: BindGroup,
    faces: u32,
    rows: u32,
}

fn init_pipeline(mut commands: Commands, asset_server: Res<AssetServer>, pipeline_cache: Res<PipelineCache>, device: Res<RenderDevice>) {
    let layout = BindGroupLayoutDescriptor::new(
        "dynamic_sky",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::COMPUTE,
            (
                uniform_buffer::<SkyParams>(false),
                texture_cube(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                texture_storage_2d_array(TextureFormat::Rgba16Float, StorageTextureAccess::WriteOnly),
                texture_3d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some("dynamic_sky".into()),
        layout: vec![layout.clone()],
        shader: asset_server.load("embedded://cod4rw/sky.wgsl"),
        ..default()
    });
    let sampler = device.create_sampler(&SamplerDescriptor {
        label: Some("dynamic_sky_classic"),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        mipmap_filter: MipmapFilterMode::Linear,
        ..default()
    });
    commands.insert_resource(SkyPipeline { layout, pipeline, sampler });
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    mut commands: Commands,
    sky: Option<Res<DynamicSky>>,
    pipeline: Res<SkyPipeline>,
    images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    pipeline_cache: Res<PipelineCache>,
    mut views: Local<Option<(AssetId<Image>, TextureView)>>,
) {
    commands.remove_resource::<SkyBindGroup>();
    let Some(sky) = sky.filter(|s| s.on) else { return };
    let (Some(out), Some(classic), Some(noise)) = (images.get(&sky.image), images.get(&sky.classic), images.get(&sky.noise)) else { return };
    if views.as_ref().is_none_or(|(id, _)| *id != sky.image.id()) {
        let view = out.texture.create_view(&TextureViewDescriptor { dimension: Some(TextureViewDimension::D2Array), ..default() });
        *views = Some((sky.image.id(), view));
    }
    let Some((_, out_view)) = views.as_ref() else { return };
    let mut uniform = UniformBuffer::from(sky.params);
    uniform.write_buffer(&device, &queue);
    let group = device.create_bind_group(
        "dynamic_sky",
        &pipeline_cache.get_bind_group_layout(&pipeline.layout),
        &BindGroupEntries::sequential((&uniform, &classic.texture_view, &pipeline.sampler, out_view, &noise.texture_view, &noise.sampler)),
    );
    commands.insert_resource(SkyBindGroup { group, faces: sky.faces, rows: sky.rows });
}

fn draw(mut ctx: RenderContext, group: Option<Res<SkyBindGroup>>, pipeline: Res<SkyPipeline>, pipeline_cache: Res<PipelineCache>) {
    use bevy::render::diagnostic::RecordDiagnostics;
    let Some(group) = group else { return };
    let Some(compute) = pipeline_cache.get_compute_pipeline(pipeline.pipeline) else { return };
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let span = diagnostics.time_span(ctx.command_encoder(), "dynamic_sky");
    {
        let mut pass = ctx.command_encoder().begin_compute_pass(&ComputePassDescriptor { label: Some("dynamic_sky"), timestamp_writes: None });
        pass.set_pipeline(compute);
        pass.set_bind_group(0, &group.group, &[]);
        pass.dispatch_workgroups(SIZE.div_ceil(8), group.rows.div_ceil(8), group.faces);
    }
    span.end(ctx.command_encoder());
}
