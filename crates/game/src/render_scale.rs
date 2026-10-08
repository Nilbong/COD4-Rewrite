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
        app.add_plugins(ExtractComponentPlugin::<RenderScale>::default())
            .add_systems(PostUpdate, mark_cameras.run_if(crate::state::in_game));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_gpu_resource::<SpecializedRenderPipelines<UpscalePipeline>>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare.in_set(RenderSystems::Prepare))
            .add_systems(Core3d, upscale.after(Core3dSystems::MainPass).before(Core3dSystems::EarlyPostProcess));
    }
}

/// On the world camera while it's drawn smaller: the share of the window.
#[derive(Component, Clone, Copy, ExtractComponent)]
pub struct RenderScale(pub f32);

/// The settings' scale on the world camera (none at 100%, or in
/// splitscreen).
fn mark_cameras(
    mut commands: Commands,
    settings: Res<crate::settings::Settings>,
    cameras: Query<(Entity, Option<&RenderScale>), With<crate::player::MainCamera>>,
) {
    let scale = (settings.num("r_renderscale") / 100.0).clamp(0.5, 1.0);
    let wanted = (scale < 0.999 && !crate::splitscreen::active()).then_some(scale);
    for (e, now) in &cameras {
        match (wanted, now) {
            (Some(s), Some(n)) if (n.0 - s).abs() < 1e-4 => {}
            (Some(s), _) => {
                commands.entity(e).insert(RenderScale(s));
            }
            (None, Some(_)) => {
                commands.entity(e).remove::<RenderScale>();
            }
            (None, None) => {}
        }
    }
}

#[derive(Resource)]
struct UpscalePipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen_shader: FullscreenShader,
    shader: Handle<Shader>,
}

impl SpecializedRenderPipeline for UpscalePipeline {
    type Key = TextureFormat;

    fn specialize(&self, format: TextureFormat) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("render scale upscale".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen_shader.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
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
    });
}

/// The view's upscale: its pipeline, and the share of the frame the world
/// was drawn into.
#[derive(Component)]
struct Upscale {
    pipeline: CachedRenderPipelineId,
    share: Vec2,
}

/// Shrink the world camera's main pass, and get its upscale ready.
fn prepare(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<UpscalePipeline>>,
    upscale_pipeline: Res<UpscalePipeline>,
    views: Query<(Entity, &ExtractedCamera, &ViewTarget, Option<&RenderScale>)>,
) {
    for (entity, camera, target, scale) in &views {
        let (Some(scale), Some(size)) = (scale, camera.physical_viewport_size) else {
            commands.entity(entity).remove::<(MainPassResolutionOverride, Upscale)>();
            continue;
        };
        let small = (size.as_vec2() * scale.0).round().as_uvec2().max(UVec2::ONE);
        let full = camera.physical_target_size.unwrap_or(size).as_vec2().max(Vec2::ONE);
        let pipeline = pipelines.specialize(&pipeline_cache, &upscale_pipeline, target.main_texture_format());
        commands.entity(entity).insert((MainPassResolutionOverride(small), Upscale { pipeline, share: small.as_vec2() / full }));
    }
}

/// Stretch the world, drawn into the frame's top left, over all of it.
fn upscale(
    view: ViewQuery<(&ViewTarget, &Upscale, Option<&RenderScale>)>,
    upscale_pipeline: Res<UpscalePipeline>,
    pipeline_cache: Res<PipelineCache>,
    mut ctx: RenderContext,
) {
    let (target, upscale, scale) = view.into_inner();
    let Some(pipeline) = pipeline_cache.get_render_pipeline(upscale.pipeline) else { return };
    // Sharpening against the stretch's blur: none at full size, 0.25 at half.
    let sharpen = (1.0 - scale.map_or(1.0, |s| s.0)) * 0.5;
    let params: [f32; 4] = [upscale.share.x, upscale.share.y, sharpen, 0.0];
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
