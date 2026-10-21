//! The settings' Render Scale: the world drawn at a fraction of the window's
//! size (50 to 100%) and stretched back up before anything else touches the
//! frame, so the gun in hand (its own camera), the post-processing and the
//! HUD stay at the window's own resolution.
//!
//! The world camera's main pass is shrunk with Bevy's
//! `MainPassResolutionOverride` (it draws into the frame's top left), then
//! [`upscale`] stretches that part over the whole frame, filtered and lightly
//! sharpened (more the lower the scale), between the main pass and the
//! post-processing. Splitscreen keeps full size: its players share one frame,
//! which the stretch would scramble.
//!
//! The settings' Upscaler (Quality, Balanced, Performance) sets the scale
//! itself (67, 59, 50%) and brings the world up with an edge-adaptive
//! upscale and sharpening after the method of AMD's FSR 1
//! (`upscale_fsr.wgsl`) instead of the plain stretch. It draws the whole
//! world camera into an image of that size (so its depth prepass, ambient
//! occlusion, volumetric fog and transparents all run at the lower
//! resolution, not just its main pass: shrinking the main pass alone saved
//! nothing, those grew), and [`upscale_world`] brings that image up into the
//! frame as the viewmodel camera starts, before the gun, post-processing and
//! HUD are drawn at full size. The image's scale factor keeps its logical
//! size the window's, so screen positions (HUD markers) don't change.

use bevy::camera::MainPassResolutionOverride;
use bevy::core_pipeline::FullscreenShader;
use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer_sized};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::view::ViewTarget;
use bevy::render::{GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;
use std::num::NonZeroU64;

pub struct RenderScalePlugin;

impl Plugin for RenderScalePlugin {
    fn build(&self, app: &mut App) {
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("render_scale.wgsl"),
            std::path::Path::new("cod4rw/render_scale.wgsl"),
            include_bytes!("render_scale.wgsl").as_slice(),
        );
        app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>().insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("upscale_fsr.wgsl"),
            std::path::Path::new("cod4rw/upscale_fsr.wgsl"),
            include_bytes!("upscale_fsr.wgsl").as_slice(),
        );
        app.add_plugins((ExtractComponentPlugin::<RenderScale>::default(), ExtractComponentPlugin::<UpscaleFrom>::default(), crate::taa::TaaPlugin))
            // Before Bevy updates the cameras: a camera moved between the
            // window and the upscaler's image is sized for its new target
            // the same frame. After, for a frame it could keep the old size: its
            // colour and depth textures (shared per target) came out of
            // step, a render error that closed the game (the player
            // switching the Upscaler, 2026-10-14).
            .add_systems(PostUpdate, (mark_cameras, low_res_world).before(bevy::camera::CameraUpdateSystems).run_if(crate::state::in_game));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_gpu_resource::<SpecializedRenderPipelines<UpscalePipeline>>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare.in_set(RenderSystems::Prepare))
            .add_systems(Core3d, upscale.after(Core3dSystems::MainPass).before(Core3dSystems::EarlyPostProcess))
            .add_systems(Core3d, upscale_world.after(Core3dSystems::Prepass).before(Core3dSystems::MainPass));
    }
}

/// On the world camera while it's drawn smaller: the share of the window,
/// and the FSR-style upscaler's sharpening (none: the plain stretch).
#[derive(Component, Clone, Copy, PartialEq, ExtractComponent)]
pub struct RenderScale(pub f32, pub Option<f32>);

/// The Upscaler setting's scale and sharpening (0..1), if on.
/// `COD4RW_UPSCALER=quality|balanced|performance|off` overrides it.
pub fn upscaler(settings: &crate::settings::Settings) -> Option<(f32, f32)> {
    let env = std::env::var("COD4RW_UPSCALER").ok();
    match env.as_deref().unwrap_or(settings.text("r_upscaler")) {
        "quality" => Some((1.0 / 1.5, 0.8)),
        "balanced" => Some((1.0 / 1.7, 0.85)),
        "performance" => Some((0.5, 0.9)),
        // With TAA, a Render Scale under 100% goes the upscaler's way too:
        // the plain stretch shrinks only the main pass, so the TAA would
        // read depth and motion for just a corner of its view.
        "off" | "" if crate::taa::wanted(settings) && env.as_deref() != Some("stretch") => {
            let scale = (settings.num("r_renderscale") / 100.0).clamp(0.5, 1.0);
            (scale < 0.999).then_some((scale, 0.8))
        }
        _ => None,
    }
}

/// The settings' scale on the world camera (none at 100%, or in
/// splitscreen).
fn mark_cameras(
    mut commands: Commands,
    settings: Res<crate::settings::Settings>,
    cameras: Query<(Entity, Option<&RenderScale>), With<crate::player::MainCamera>>,
) {
    let wanted = match upscaler(&settings) {
        // The upscaler draws the world into its own image (`low_res_world`).
        Some(_) => None,
        // (`COD4RW_UPSCALER=stretch`: the plain stretch at Quality's scale, to compare.)
        None if std::env::var("COD4RW_UPSCALER").is_ok_and(|v| v == "stretch") => Some(RenderScale(1.0 / 1.5, None)),
        None => {
            let scale = (settings.num("r_renderscale") / 100.0).clamp(0.5, 1.0);
            (scale < 0.999).then_some(RenderScale(scale, None))
        }
    }
    .filter(|_| !crate::splitscreen::active());
    for (e, now) in &cameras {
        match (wanted, now) {
            (Some(s), Some(n)) if *n == s => {}
            (Some(s), _) => {
                commands.entity(e).insert(s);
            }
            (None, Some(_)) => {
                commands.entity(e).remove::<RenderScale>();
            }
            (None, None) => {}
        }
    }
}

/// On the viewmodel camera while the upscaler is on: the world's image to
/// bring up into the frame first, and the sharpening.
#[derive(Component, Clone, PartialEq, ExtractComponent)]
pub struct UpscaleFrom(pub Handle<Image>, pub f32);

/// The world camera's own small image while the upscaler is on.
#[derive(Resource)]
struct LowRes(Handle<Image>, UVec2);

/// The world camera's image size while the upscaler draws it smaller.
static WORLD_SIZE: std::sync::Mutex<Option<UVec2>> = std::sync::Mutex::new(None);

/// The size the world camera draws at, if the upscaler has it smaller than
/// the window (for screen-space effects that copy its image: SSR).
pub fn world_size() -> Option<UVec2> {
    *WORLD_SIZE.lock().ok()?
}

/// With the upscaler on, the world camera draws into an image at its scale
/// (made anew when the window's size changes) and the viewmodel camera
/// starts from it; off, both are put back.
#[allow(clippy::type_complexity)]
fn low_res_world(
    mut commands: Commands,
    settings: Res<crate::settings::Settings>,
    window: Single<&Window, With<bevy::window::PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    low: Option<Res<LowRes>>,
    mut world: Query<(Entity, &mut Camera, Option<&bevy::camera::RenderTarget>), (With<crate::player::MainCamera>, Without<crate::player::ViewModelCamera>)>,
    vm: Query<(Entity, Option<&UpscaleFrom>, Option<&bevy::anti_alias::smaa::Smaa>), With<crate::player::ViewModelCamera>>,
    world_aa: Query<Option<&bevy::anti_alias::smaa::Smaa>, With<crate::player::MainCamera>>,
) {
    let wanted = upscaler(&settings).filter(|_| !crate::splitscreen::active());
    let Some((scale, sharpen)) = wanted else {
        if low.is_some() {
            *WORLD_SIZE.lock().unwrap_or_else(|e| e.into_inner()) = None;
            commands.remove_resource::<LowRes>();
            for (e, mut camera, _) in &mut world {
                camera.output_mode = bevy::camera::CameraOutputMode::Skip;
                commands.entity(e).insert(bevy::camera::RenderTarget::Window(bevy::window::WindowRef::Primary));
                commands.entity(e).remove::<bevy::anti_alias::smaa::Smaa>();
            }
            for (e, _, _) in &vm {
                commands.entity(e).remove::<UpscaleFrom>();
            }
        }
        return;
    };
    let full = UVec2::new(window.physical_width(), window.physical_height());
    // A minimized window reports (next to) nothing: the image is left as it
    // was (a 1 px image broke ambient occlusion's mip chain, a validation
    // error every frame).
    if full.min_element() < 16 {
        return;
    }
    let size = (full.as_vec2() * scale).round().as_uvec2().max(UVec2::ONE);
    *WORLD_SIZE.lock().unwrap_or_else(|e| e.into_inner()) = Some(size);
    let image = match low.as_deref() {
        Some(LowRes(h, s)) if *s == size => h.clone(),
        _ => {
            let h = images.add(Image::new_target_texture(size.x, size.y, TextureFormat::Rgba16Float, None));
            commands.insert_resource(LowRes(h.clone(), size));
            h
        }
    };
    let target = bevy::camera::RenderTarget::Image(bevy::camera::ImageRenderTarget {
        handle: image.clone(),
        // Logical size stays the window's (screen positions unchanged).
        scale_factor: window.scale_factor() * scale,
    });
    // The settings' SMAA on the small image too, before it's brought up
    // (thin wires and cables broke into dashes otherwise).
    // `COD4RW_UPSCALE_AA=0` leaves it to the full-size frame alone.
    let aa = vm.iter().find_map(|(_, _, smaa)| smaa.copied()).filter(|_| !std::env::var("COD4RW_UPSCALE_AA").is_ok_and(|v| v == "0"))
        // (TAA takes SMAA's place there: `crate::taa`.)
        .filter(|_| !crate::taa::wanted(&settings));
    for (e, mut camera, now) in &mut world {
        let same = matches!(now, Some(bevy::camera::RenderTarget::Image(t)) if t.handle == image);
        if !same {
            commands.entity(e).insert(target.clone());
        }
        let has = world_aa.get(e).ok().flatten().map(|s| s.preset);
        match aa {
            Some(smaa) if has != Some(smaa.preset) => {
                commands.entity(e).insert(smaa);
            }
            None if has.is_some() => {
                commands.entity(e).remove::<bevy::anti_alias::smaa::Smaa>();
            }
            _ => {}
        }
        if matches!(camera.output_mode, bevy::camera::CameraOutputMode::Skip) {
            camera.output_mode = bevy::camera::CameraOutputMode::default();
        }
    }
    let from = UpscaleFrom(image, sharpen);
    for (e, now, _) in &vm {
        if now != Some(&from) {
            commands.entity(e).insert(from.clone());
        }
    }
}

#[derive(Resource)]
struct UpscalePipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen_shader: FullscreenShader,
    shader: Handle<Shader>,
    fsr: Handle<Shader>,
}

/// Which pass: the plain stretch, or the upscaler's two.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Pass {
    Stretch,
    Easu,
    Rcas,
}

impl SpecializedRenderPipeline for UpscalePipeline {
    type Key = (TextureFormat, Pass);

    fn specialize(&self, (format, pass): Self::Key) -> RenderPipelineDescriptor {
        let (shader, entry) = match pass {
            Pass::Stretch => (self.shader.clone(), None),
            Pass::Easu => (self.fsr.clone(), Some("easu".into())),
            Pass::Rcas => (self.fsr.clone(), Some("rcas".into())),
        };
        RenderPipelineDescriptor {
            label: Some("render scale upscale".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen_shader.to_vertex_state(),
            fragment: Some(FragmentState {
                shader,
                entry_point: entry,
                targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        }
    }
}

fn init_pipeline(mut commands: Commands, render_device: Res<RenderDevice>, fullscreen_shader: Res<FullscreenShader>, asset_server: Res<AssetServer>) {
    let layout = BindGroupLayoutDescriptor::new(
        "render_scale_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                uniform_buffer_sized(false, NonZeroU64::new(16)),
            ),
        ),
    );
    let sampler = render_device.create_sampler(&SamplerDescriptor { mag_filter: FilterMode::Linear, min_filter: FilterMode::Linear, ..default() });
    commands.insert_resource(UpscalePipeline {
        layout,
        sampler,
        fullscreen_shader: fullscreen_shader.clone(),
        shader: asset_server.load("embedded://cod4rw/render_scale.wgsl"),
        fsr: asset_server.load("embedded://cod4rw/upscale_fsr.wgsl"),
    });
}

/// The view's upscale: its pipeline, and the share of the frame the world
/// was drawn into.
#[derive(Component)]
struct Upscale {
    pipeline: CachedRenderPipelineId,
    /// The upscaler's sharpening pass, when on.
    rcas: Option<CachedRenderPipelineId>,
    share: Vec2,
}

/// The viewmodel camera's passes bringing the world's image up.
#[derive(Component)]
struct UpscaleFromPipelines {
    easu: CachedRenderPipelineId,
    rcas: CachedRenderPipelineId,
}

/// Shrink the world camera's main pass, and get its upscale ready.
fn prepare(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<UpscalePipeline>>,
    upscale_pipeline: Res<UpscalePipeline>,
    views: Query<(Entity, &ExtractedCamera, &ViewTarget, Option<&RenderScale>)>,
    from: Query<(Entity, &ViewTarget), With<UpscaleFrom>>,
) {
    for (entity, target) in &from {
        let format = target.main_texture_format();
        let easu = pipelines.specialize(&pipeline_cache, &upscale_pipeline, (format, Pass::Easu));
        let rcas = pipelines.specialize(&pipeline_cache, &upscale_pipeline, (format, Pass::Rcas));
        commands.entity(entity).insert(UpscaleFromPipelines { easu, rcas });
    }
    for (entity, camera, target, scale) in &views {
        let (Some(scale), Some(size)) = (scale, camera.physical_viewport_size) else {
            commands.entity(entity).remove::<(MainPassResolutionOverride, Upscale)>();
            continue;
        };
        let small = (size.as_vec2() * scale.0).round().as_uvec2().max(UVec2::ONE);
        let full = camera.physical_target_size.unwrap_or(size).as_vec2().max(Vec2::ONE);
        let format = target.main_texture_format();
        let fsr = scale.1.is_some();
        let pipeline = pipelines.specialize(&pipeline_cache, &upscale_pipeline, (format, if fsr { Pass::Easu } else { Pass::Stretch }));
        let rcas = fsr.then(|| pipelines.specialize(&pipeline_cache, &upscale_pipeline, (format, Pass::Rcas)));
        commands.entity(entity).insert((MainPassResolutionOverride(small), Upscale { pipeline, rcas, share: small.as_vec2() / full }));
    }
}

/// Stretch the world, drawn into the frame's top left, over all of it
/// (the upscaler: then sharpen it).
fn upscale(
    view: ViewQuery<(&ViewTarget, &Upscale, Option<&RenderScale>)>,
    upscale_pipeline: Res<UpscalePipeline>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (target, upscale, scale) = view.into_inner();
    let Some(pipeline) = pipeline_cache.get_render_pipeline(upscale.pipeline) else { return };
    let rcas = upscale.rcas.and_then(|id| pipeline_cache.get_render_pipeline(id));
    if upscale.rcas.is_some() && rcas.is_none() {
        return;
    }
    let sharpen = match scale {
        Some(RenderScale(_, Some(s))) => *s,
        // The stretch's sharpening against its blur: none at full size,
        // 0.25 at half.
        s => (1.0 - s.map_or(1.0, |s| s.0)) * 0.5,
    };
    draw(&mut ctx, target, &upscale_pipeline, &pipeline_cache, pipeline, [upscale.share.x, upscale.share.y, sharpen, 0.0]);
    if let Some(rcas) = rcas {
        draw(&mut ctx, target, &upscale_pipeline, &pipeline_cache, rcas, [1.0, 1.0, sharpen, 0.0]);
    }
}

/// The viewmodel camera, as it starts: the world's small image brought up
/// into the frame (edge-adaptive upscale, then sharpening), for the gun,
/// post-processing and HUD to draw over.
fn upscale_world(
    view: ViewQuery<(&ViewTarget, &UpscaleFrom, &UpscaleFromPipelines)>,
    upscale_pipeline: Res<UpscalePipeline>,
    pipeline_cache: Res<PipelineCache>,
    images: Res<bevy::render::render_asset::RenderAssets<bevy::render::texture::GpuImage>>,
    mut ctx: RenderContext,
) {
    let (target, from, pipelines) = view.into_inner();
    let (Some(easu), Some(rcas), Some(world)) = (
        pipeline_cache.get_render_pipeline(pipelines.easu),
        pipeline_cache.get_render_pipeline(pipelines.rcas),
        images.get(&from.0),
    ) else {
        return;
    };
    // Into the frame's other image (its first is overwritten anyway).
    let post_process = target.post_process_write();
    pass(&mut ctx, &upscale_pipeline, &pipeline_cache, easu, &world.texture_view, post_process.destination, [1.0, 1.0, from.1, 0.0]);
    draw(&mut ctx, target, &upscale_pipeline, &pipeline_cache, rcas, [1.0, 1.0, from.1, 0.0]);
}

/// One full-frame pass from `source` into `destination`.
fn pass(
    ctx: &mut RenderContext,
    upscale_pipeline: &UpscalePipeline,
    pipeline_cache: &PipelineCache,
    pipeline: &RenderPipeline,
    source: &TextureView,
    destination: &TextureView,
    params: [f32; 4],
) {
    let bytes: Vec<u8> = params.iter().flat_map(|v| v.to_le_bytes()).collect();
    let buffer = ctx.render_device().create_buffer_with_data(&BufferInitDescriptor {
        label: Some("render_scale_params"),
        contents: &bytes,
        usage: BufferUsages::UNIFORM,
    });
    let bind_group = ctx.render_device().create_bind_group(
        "render_scale_bind_group",
        &pipeline_cache.get_bind_group_layout(&upscale_pipeline.layout),
        &BindGroupEntries::sequential((source, &upscale_pipeline.sampler, buffer.as_entire_binding())),
    );
    let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some("render scale upscale"),
        color_attachments: &[Some(RenderPassColorAttachment { view: destination, depth_slice: None, resolve_target: None, ops: Operations::default() })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.draw(0..3, 0..1);
}

/// One full-frame pass from the frame's image into its other one.
fn draw(
    ctx: &mut RenderContext,
    target: &ViewTarget,
    upscale_pipeline: &UpscalePipeline,
    pipeline_cache: &PipelineCache,
    pipeline: &RenderPipeline,
    params: [f32; 4],
) {
    let bytes: Vec<u8> = params.iter().flat_map(|v| v.to_le_bytes()).collect();
    let buffer = ctx.render_device().create_buffer_with_data(&BufferInitDescriptor {
        label: Some("render_scale_params"),
        contents: &bytes,
        usage: BufferUsages::UNIFORM,
    });
    let post_process = target.post_process_write();
    let bind_group = ctx.render_device().create_bind_group(
        "render_scale_bind_group",
        &pipeline_cache.get_bind_group_layout(&upscale_pipeline.layout),
        &BindGroupEntries::sequential((post_process.source, &upscale_pipeline.sampler, buffer.as_entire_binding())),
    );
    let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some("render scale upscale"),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: post_process.destination,
            depth_slice: None,
            resolve_target: None,
            ops: Operations::default(),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, &bind_group, &[]);
    pass.draw(0..3, 0..1);
}
