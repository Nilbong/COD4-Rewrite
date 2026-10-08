//! God rays: light streaming from the sun past whatever stands in front
//! of it (window frames, doorways, foliage, gaps between buildings), the
//! screen-space way: the sky round the sun (masked by depth, so only sky
//! and openings give light) blurred out from the sun's place on screen, at
//! half resolution, and added to the world camera's frame.
//!
//! On at both Volumetric Lighting levels (Low: these alone; High: with the
//! raymarched shafts, `super::volumetric`). They fade out as the sun leaves
//! the screen or goes behind the camera, take the map's sun colour and
//! strength, and level off softly (`GodRays::cap`) so looking into the sun
//! never washes out someone standing there. Not in splitscreen (the passes
//! cover the whole target). Off: no component, no passes.

use super::volumetric::Level;
use crate::splitscreen::SlotCamera;
use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
use bevy::core_pipeline::FullscreenShader;
use bevy::prelude::*;
use bevy::render::extract_component::{ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin, UniformComponentPlugin};
use bevy::render::render_resource::binding_types::{sampler, texture_2d, texture_depth_2d, uniform_buffer};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::render::view::{ExtractedView, ViewDepthTexture, ViewTarget};
use bevy::render::{GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

pub struct GodRaysPlugin;

impl Plugin for GodRaysPlugin {
    fn build(&self, app: &mut App) {
        let registry = app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>();
        registry.insert_asset(
            std::path::PathBuf::from(file!()).with_file_name("godrays.wgsl"),
            std::path::Path::new("cod4rw/godrays.wgsl"),
            include_bytes!("godrays.wgsl").as_slice(),
        );
        app.add_plugins((ExtractComponentPlugin::<GodRays>::default(), UniformComponentPlugin::<GodRays>::default()))
            .add_systems(PostUpdate, aim.after(TransformSystems::Propagate).run_if(crate::state::in_game));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_gpu_resource::<SpecializedRenderPipelines<GodRaysPipeline>>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, (prepare_pipelines.in_set(RenderSystems::Prepare), prepare_textures.in_set(RenderSystems::PrepareResources)))
            .add_systems(Core3d, god_rays.after(Core3dSystems::MainPass).before(Core3dSystems::EarlyPostProcess));
    }
}

/// A world camera's god rays this frame (`godrays.wgsl`).
#[derive(Component, Clone, Copy, Debug, Default, ShaderType, ExtractComponent)]
#[extract_component_filter(With<Camera>)]
pub struct GodRays {
    /// The sun's place on screen (0..1, may lie off it).
    sun_uv: Vec2,
    /// How strong: the map's sun, faded as it leaves the view (0: skip).
    strength: f32,
    /// The most they add (the frame's units), approached softly.
    cap: f32,
    /// The sun's colour, brightest channel 1.
    color: Vec3,
    /// How far each blur sample reaches toward the sun (share of the way).
    density: f32,
}

/// Debug aid: `COD4RW_GODRAYS_GAIN` scales the rays.
fn gain() -> f32 {
    std::env::var("COD4RW_GODRAYS_GAIN").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0)
}

/// Each world camera's rays, aimed at the sun.
#[allow(clippy::type_complexity)]
fn aim(
    mut commands: Commands,
    settings: Res<crate::settings::Settings>,
    fog: Option<Res<crate::fog::MapFog>>,
    suns: Query<(&DirectionalLight, &GlobalTransform), (Without<crate::model_lighting::ViewModelSun>, Without<Camera>)>,
    mut cameras: Query<(Entity, &Camera, &GlobalTransform, &mut Camera3d, Option<&mut GodRays>), With<SlotCamera>>,
    tod: Res<super::climate::TimeOfDay>,
    weather: Res<super::climate::Weather>,
) {
    let level = if crate::splitscreen::active() { Level::Off } else { Level::of(&settings) };
    let sun = suns.iter().find(|(l, _)| l.shadow_maps_enabled).or_else(|| suns.iter().find(|(l, _)| l.illuminance > 0.0));
    for (e, camera, eye, mut cam3d, rays) in &mut cameras {
        let (Some((light, sun_tf)), false) = (sun, level == Level::Off) else {
            if rays.is_some() {
                commands.entity(e).remove::<GodRays>();
            }
            continue;
        };
        // The mask reads the depth buffer.
        let usage = TextureUsages::from(cam3d.depth_texture_usages);
        if !usage.contains(TextureUsages::TEXTURE_BINDING) {
            cam3d.depth_texture_usages = (usage | TextureUsages::TEXTURE_BINDING).into();
        }
        let to_sun = -sun_tf.forward().as_vec3();
        // Fade as the sun leaves the view: gone at 90 degrees off the
        // view's axis, or half a screen past its edge.
        let facing = eye.forward().dot(to_sun);
        let ndc = camera.world_to_ndc(eye, eye.translation() + to_sun * 1000.0).filter(|_| facing > 0.0);
        let fade = ndc.map_or(0.0, |n| {
            let past_edge = (n.x.abs().max(n.y.abs()) - 1.0).max(0.0);
            smooth(facing / 0.35) * (1.0 - smooth(past_edge / 0.5))
        });
        let sun_uv = ndc.map_or(Vec2::splat(0.5), |n| Vec2::new(n.x * 0.5 + 0.5, 0.5 - n.y * 0.5));
        let c = light.color.to_linear();
        let mut color = Vec3::new(c.red, c.green, c.blue);
        // A share of the map's fog colour: Crash's dust, Bog's brown.
        if let Some((f, _)) = super::map_fog(fog.as_deref()) {
            color = color.lerp(color * f / f.max_element().max(1e-3), 0.3);
        }
        let color = color / color.max_element().max(1e-3);
        // The map's sun: stronger where more of it is live.
        // (Not from the moon, nor through storm cloud: the showcase.)
        let veiled = if tod.enabled { (1.0 - tod.night) * (1.0 - (weather.rain / 0.6).clamp(0.0, 1.0)) } else { 1.0 };
        let strength = 5.0 * fade * gain() * veiled * (light.illuminance / 12_000.0).clamp(0.5, 1.3);
        let want = GodRays { sun_uv, strength, cap: 0.25, color, density: 0.6 };
        match rays {
            Some(mut r) => *r = want,
            None => {
                commands.entity(e).insert(want);
            }
        }
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// --- render world

const RAYS_FORMAT: TextureFormat = TextureFormat::Rg11b10Ufloat;

#[derive(Resource)]
struct GodRaysPipeline {
    mask_layout: BindGroupLayoutDescriptor,
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen: FullscreenShader,
    shader: Handle<Shader>,
}

/// Which pass, and the frame's format for the last.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Pass {
    Mask,
    Blur,
    Add(TextureFormat),
}

impl SpecializedRenderPipeline for GodRaysPipeline {
    type Key = Pass;

    fn specialize(&self, pass: Pass) -> RenderPipelineDescriptor {
        let (entry, layout, target) = match pass {
            Pass::Mask => ("mask", &self.mask_layout, ColorTargetState { format: RAYS_FORMAT, blend: None, write_mask: ColorWrites::ALL }),
            Pass::Blur => ("blur", &self.layout, ColorTargetState { format: RAYS_FORMAT, blend: None, write_mask: ColorWrites::ALL }),
            Pass::Add(format) => (
                "add",
                &self.layout,
                ColorTargetState {
                    format,
                    blend: Some(BlendState {
                        color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
                        alpha: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add },
                    }),
                    write_mask: ColorWrites::ALL,
                },
            ),
        };
        RenderPipelineDescriptor {
            label: Some(format!("god rays {entry}").into()),
            layout: vec![layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                entry_point: Some(entry.into()),
                targets: vec![Some(target)],
                ..default()
            }),
            ..default()
        }
    }
}

fn init_pipeline(
    mut commands: Commands,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
    asset_server: Res<AssetServer>,
) {
    let texture = || texture_2d(TextureSampleType::Float { filterable: true });
    let mask_layout = BindGroupLayoutDescriptor::new(
        "god_rays_mask",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (texture(), sampler(SamplerBindingType::Filtering), uniform_buffer::<GodRays>(true), texture_depth_2d()),
        ),
    );
    let layout = BindGroupLayoutDescriptor::new(
        "god_rays",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (texture(), sampler(SamplerBindingType::Filtering), uniform_buffer::<GodRays>(true)),
        ),
    );
    let sampler = device.create_sampler(&SamplerDescriptor {
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        ..default()
    });
    commands.insert_resource(GodRaysPipeline {
        mask_layout,
        layout,
        sampler,
        fullscreen: fullscreen.clone(),
        shader: asset_server.load("embedded://cod4rw/godrays.wgsl"),
    });
}

#[derive(Component)]
struct GodRaysPipelines([CachedRenderPipelineId; 3]);

fn prepare_pipelines(
    mut commands: Commands,
    cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<GodRaysPipeline>>,
    pipeline: Res<GodRaysPipeline>,
    views: Query<(Entity, &ViewTarget), With<GodRays>>,
) {
    for (e, target) in &views {
        let ids = [Pass::Mask, Pass::Blur, Pass::Add(target.main_texture_format())].map(|p| pipelines.specialize(&cache, &pipeline, p));
        commands.entity(e).insert(GodRaysPipelines(ids));
    }
}

/// Half-resolution textures: the masked sky, and the rays.
#[derive(Component)]
struct GodRaysTextures([CachedTexture; 2]);

fn prepare_textures(mut commands: Commands, device: Res<RenderDevice>, mut cache: ResMut<TextureCache>, views: Query<(Entity, &ExtractedView), With<GodRays>>) {
    for (e, view) in &views {
        let size = Extent3d { width: (view.viewport.z / 2).max(1), height: (view.viewport.w / 2).max(1), depth_or_array_layers: 1 };
        let texture = |label: &'static str, cache: &mut TextureCache| {
            cache.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: RAYS_FORMAT,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let textures = [texture("god_rays_mask", &mut cache), texture("god_rays", &mut cache)];
        commands.entity(e).insert(GodRaysTextures(textures));
    }
}

#[allow(clippy::type_complexity)]
fn god_rays(
    view: ViewQuery<(&ViewTarget, &ViewDepthTexture, &GodRays, &GodRaysPipelines, &GodRaysTextures, &DynamicUniformIndex<GodRays>)>,
    pipeline: Res<GodRaysPipeline>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<GodRays>>,
    mut ctx: RenderContext,
) {
    let (target, depth, rays, ids, textures, index) = view.into_inner();
    if rays.strength <= 0.0 {
        return;
    }
    let ([Some(mask), Some(blur), Some(add)], Some(uniforms)) = (ids.0.map(|id| cache.get_render_pipeline(id)), uniforms.binding()) else {
        return;
    };
    let [a, b] = &textures.0;
    let device = ctx.render_device().clone();
    let mask_group = device.create_bind_group(
        "god_rays_mask",
        &cache.get_bind_group_layout(&pipeline.mask_layout),
        &BindGroupEntries::sequential((target.main_texture_view(), &pipeline.sampler, uniforms.clone(), depth.view())),
    );
    let layout = cache.get_bind_group_layout(&pipeline.layout);
    let blur_group = device.create_bind_group("god_rays_blur", &layout, &BindGroupEntries::sequential((&a.default_view, &pipeline.sampler, uniforms.clone())));
    let add_group = device.create_bind_group("god_rays_add", &layout, &BindGroupEntries::sequential((&b.default_view, &pipeline.sampler, uniforms)));
    use bevy::render::diagnostic::RecordDiagnostics;
    let diagnostics = ctx.diagnostic_recorder();
    let diagnostics = diagnostics.as_deref();
    let span = diagnostics.time_span(ctx.command_encoder(), "god_rays");
    pass(&mut ctx, "god rays mask", mask, &mask_group, index.index(), &a.default_view, true);
    pass(&mut ctx, "god rays blur", blur, &blur_group, index.index(), &b.default_view, true);
    pass(&mut ctx, "god rays add", add, &add_group, index.index(), target.main_texture_view(), false);
    span.end(ctx.command_encoder());
}

fn pass(ctx: &mut RenderContext, label: &str, pipeline: &RenderPipeline, group: &BindGroup, index: u32, target: &TextureView, clear: bool) {
    let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: Operations { load: if clear { LoadOp::Clear(Default::default()) } else { LoadOp::Load }, store: StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[index]);
    pass.draw(0..3, 0..1);
}
