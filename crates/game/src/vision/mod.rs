//! A map as CoD4 shows it, plus a Black Ops touch:
//! - its film (`vision/<map>.vision`: contrast, brightness, desaturation,
//!   light and dark tints), graded over the finished frame with the math of
//!   CoD4's film shaders (`film.wgsl`), and its glow (`r_glow*`: what's
//!   brighter than the cutoff, graded, desaturated, blurred and added back;
//!   `glow.wgsl`), which replaces Bevy's bloom: maps with `r_glow 0` have
//!   none, as in CoD4;
//! - night vision (N, CoD4's `+actionslot 1`): the `default_night` vision
//!   set `_load.gsc` gives every map, behind the goggles' overlay, switched
//!   through a fade to black with `item_nightvision_on`/`_off` (CoD4's
//!   multiplayer guns have no goggles animations);
//! - its sun (`sunflare_t`): the lens flare round the sun while it's in
//!   view, and the screen darkening ("blind") and lightening ("glare") of
//!   looking into it, each fading in and out over the map's times;
//! - Black Ops' light grid tweaks for models (see
//!   [`crate::model_lighting`]).
//!
//! `COD4RW_NOVISION` leaves all of it out, for comparing; `COD4RW_SUNTEST`
//! screenshots the sun's effects, `COD4RW_NVTEST` night vision.

use crate::content::Content;
use crate::player::MainCamera;
use crate::state::{GameState, in_game};
use crate::world::MapName;
use avian3d::prelude::SpatialQuery;
use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
use bevy::core_pipeline::tonemapping::tonemapping;
use bevy::core_pipeline::FullscreenShader;
use bevy::prelude::*;
use bevy::render::extract_component::{
    ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin, UniformComponentPlugin,
};
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer};
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderContext, RenderDevice, ViewQuery};
use bevy::render::view::{ExtractedView, ViewTarget};
use bevy::render::{GpuResourceAppExt, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::{Shader, ShaderRef};
use bevy::ui_render::prelude::{MaterialNode, UiMaterial, UiMaterialKey, UiMaterialPlugin};
use bevy::post_process::bloom::Bloom;
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::window::PrimaryWindow;
use iw3::zone::SunFlare;

/// Debug aid: `COD4RW_NOVISION` turns the film, the sun's effects and the
/// light grid tweaks off.
pub fn disabled() -> bool {
    std::env::var_os("COD4RW_NOVISION").is_some()
}

pub struct VisionPlugin;

impl Plugin for VisionPlugin {
    fn build(&self, app: &mut App) {
        let registry = app.world().resource::<bevy::asset::io::embedded::EmbeddedAssetRegistry>();
        for (name, bytes) in [
            ("film.wgsl", include_bytes!("film.wgsl").as_slice()),
            ("glow.wgsl", include_bytes!("glow.wgsl").as_slice()),
            ("sun.wgsl", include_bytes!("sun.wgsl").as_slice()),
        ] {
            registry.insert_asset(
                std::path::PathBuf::from(file!()).with_file_name(name),
                &std::path::Path::new("cod4rw").join(name),
                bytes,
            );
        }
        app.add_plugins((
            ExtractComponentPlugin::<FilmUniform>::default(),
            UniformComponentPlugin::<FilmUniform>::default(),
            UiMaterialPlugin::<SunMaterial>::default(),
        ))
        .init_resource::<MapLook>()
        .init_resource::<NightVisions>()
        .add_systems(OnEnter(GameState::InGame), load_look.in_set(crate::state::Setup::Spawn))
        .add_systems(Update, (night_vision, apply_film, sun_effects).chain().run_if(in_game));
        if let Ok(dir) = std::env::var("COD4RW_NVTEST") {
            app.insert_resource(NvTest(dir.into())).add_systems(Update, nv_test.before(night_vision).run_if(in_game));
        }
        if let Ok(dir) = std::env::var("COD4RW_SUNTEST") {
            app.insert_resource(SunTest(dir.into())).add_systems(Update, sun_test.before(sun_effects).run_if(in_game));
        }
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .init_gpu_resource::<SpecializedRenderPipelines<FilmPipeline>>()
            .add_systems(RenderStartup, init_film_pipeline)
            .add_systems(Render, prepare_film_pipelines.in_set(RenderSystems::Prepare))
            .add_systems(Render, prepare_glow_textures.in_set(RenderSystems::PrepareResources))
            // Each post-process pass flips the frame's two textures, so the
            // film needs a fixed place among them: after tonemapping, before
            // anti-aliasing. Left unordered against SMAA, the two ran either
            // way round, and now and then a frame lost its film or got it
            // twice (a flicker).
            .add_systems(
                Core3d,
                film.after(tonemapping)
                    .before(bevy::anti_alias::smaa::smaa)
                    .before(bevy::anti_alias::fxaa::fxaa)
                    .in_set(Core3dSystems::PostProcess),
            );
    }
}

/// The map's look, read as the match starts: its film and glow (`None`
/// without a vision file), and its sun.
#[derive(Resource, Default)]
struct MapLook {
    film: Option<FilmUniform>,
    /// Through night vision (`VisionSetNight( "default_night" )`).
    night: Option<FilmUniform>,
    sun: Option<Sun>,
}

struct Sun {
    flare: SunFlare,
    /// Towards the sun, Bevy space.
    dir: Vec3,
    image: Option<Handle<Image>>,
}

fn load_look(
    mut look: ResMut<MapLook>,
    mut night_vision: ResMut<NightVisions>,
    mut content: Option<ResMut<Content>>,
    map: Res<MapName>,
    mut images: ResMut<Assets<Image>>,
) {
    *look = MapLook::default();
    *night_vision = NightVisions::default();
    let Some(content) = content.as_deref_mut() else { return };
    look.night = raw_file(content, "vision/default_night.vision").map(|t| FilmUniform::parse(&t));
    if disabled() {
        return;
    }
    look.film = film_of(content, &map.0).map(|(f, contrast)| if cod4_film() { f } else { f.balanced(contrast) });
    look.sun = sun_of(content, &mut images);
    let on = |b: bool| if b { "on" } else { "off" };
    info!(
        "vision: film {}, glow {}, sun flare {}",
        on(look.film.is_some_and(|f| f.film_on())),
        on(look.film.is_some_and(|f| f.glow_on())),
        on(look.sun.is_some())
    );
}

/// A raw file of the match's zones, as text.
fn raw_file(content: &Content, name: &str) -> Option<String> {
    content.zones.iter().flat_map(|z| &z.assets).find_map(|a| match a {
        iw3::zone::Asset::RawFile(r) if r.name.eq_ignore_ascii_case(name) => Some(String::from_utf8_lossy(&r.data).into_owned()),
        _ => None,
    })
}

// --- film

/// The film and glow settings as CoD4's shaders take them: see `film.wgsl`
/// and `glow.wgsl`.
#[derive(Component, Clone, Copy, Debug, PartialEq, ShaderType, ExtractComponent)]
#[extract_component_filter(With<Camera>)]
pub struct FilmUniform {
    tint_base: Vec4,
    tint_delta: Vec4,
    bias: Vec4,
    /// Cutoff, its rescale `1 / (1 - cutoff)`, desaturation, intensity.
    glow: Vec4,
    /// The blur's radius (pixels at 640x480), and 1 when the glow is on.
    glow_blur: Vec4,
}

impl FilmUniform {
    /// No film: the frame as it is.
    const NO_FILM: [Vec4; 3] = [Vec4::new(1.0, 1.0, 1.0, 0.0), Vec4::ZERO, Vec4::ZERO];

    fn film_on(&self) -> bool {
        [self.tint_base, self.tint_delta, self.bias] != Self::NO_FILM
    }

    fn glow_on(&self) -> bool {
        self.glow_blur.y > 0.5
    }

    /// The film without its colour: each tint the grey of its luminance
    /// (`film.wgsl`'s weights), so it still darkens and lightens as much.
    /// The film without its glow (the Bloom setting off).
    fn unglowing(self) -> FilmUniform {
        FilmUniform { glow_blur: Vec4::new(self.glow_blur.x, 0.0, self.glow_blur.z, self.glow_blur.w), ..self }
    }

    fn untinted(self) -> FilmUniform {
        let grey = |v: Vec4| Vec3::splat(v.truncate().dot(Vec3::new(0.299, 0.587, 0.114))).extend(v.w);
        FilmUniform { tint_base: grey(self.tint_base), tint_delta: grey(self.tint_delta), ..self }
    }

    /// One number from a `.vision` file.
    fn value(text: &str, key: &str) -> Option<f32> {
        text.lines().find_map(|l| {
            let mut words = l.split_whitespace();
            (words.next()?.eq_ignore_ascii_case(key)).then(|| words.next()?.trim_matches('"').parse().ok()).flatten()
        })
    }

    /// The film rebalanced for this renderer's exposed, tonemapped frame
    /// (`contrast` is the file's `r_filmContrast`). CoD4's film was tuned
    /// for its own dark raw frame: its contrast and brightness together are
    /// mostly a gain of 1.4-1.7x (Crossfire's leaves only black where it
    /// was), which on a frame auto-exposure already brightened blew the
    /// highlights out and crushed the shadows. Here the tints and
    /// desaturation keep each map's mood, but mid-grey (0.5 on screen) stays
    /// mid-grey (no added contrast: AgX gives the curve), black lifts a
    /// touch (`FILM_LIFT`), highlights stretch (`HIGHLIGHT_GAIN`) so backlit trees and guns keep some shape, and
    /// the desaturation is gentler (AgX already desaturates).
    /// Maps the file lowers contrast on (Bog's 0.82) keep a little of that.
    fn balanced(self, contrast: f32) -> FilmUniform {
        if !self.film_on() {
            return self;
        }
        const MID: f32 = 0.5;
        const FILM_LIFT: f32 = 0.035;
        // Highlights stretched (twice as steep over 0.6 on screen): AgX
        // rolls them off well below white, which with the brighter middle
        // CoD4's look lost to left Crash's ground flat and pale.
        const HIGHLIGHT_GAIN: f32 = 1.0;
        const HIGHLIGHT_FROM: f32 = 0.6;

        let luma = Vec3::new(0.299, 0.587, 0.114);
        let slope = (1.0 + (contrast - 1.0) * 0.5).clamp(0.9, 1.0);
        let at_mid = (self.tint_base.truncate() + self.tint_delta.truncate() * MID).dot(luma).max(0.05);
        let gain = slope * (1.0 - FILM_LIFT) / at_mid;
        FilmUniform {
            tint_base: (self.tint_base.truncate() * gain).extend(self.tint_base.w),
            tint_delta: self.tint_delta * gain,
            bias: Vec3::splat(MID * (1.0 - slope) + FILM_LIFT).extend(self.bias.w * 0.3),
            glow_blur: Vec4::new(self.glow_blur.x, self.glow_blur.y, HIGHLIGHT_GAIN, HIGHLIGHT_FROM),
            ..self
        }
    }

    /// From a `.vision` file: its `r_film*` settings (the frame as it is if
    /// its film is off) and its glow (`r_glow`, `r_glowBloomCutoff`,
    /// `r_glowBloomDesaturation`, and the `0` intensity and radius: the
    /// renderer's only, as its tweak dvars show).
    fn parse(text: &str) -> FilmUniform {
        let value = |key: &str| -> Option<Vec<f32>> {
            text.lines().find_map(|l| {
                let mut words = l.split_whitespace();
                (words.next()? .eq_ignore_ascii_case(key)).then(|| {
                    words.map(|w| w.trim_matches('"')).filter_map(|w| w.parse().ok()).collect()
                })
            })
        };
        let one = |key: &str, default: f32| value(key).and_then(|v| v.first().copied()).unwrap_or(default);
        let three = |key: &str| value(key).filter(|v| v.len() >= 3).map_or(Vec3::ONE, |v| Vec3::new(v[0], v[1], v[2]));
        let cutoff = one("r_glowBloomCutoff", 0.99).clamp(0.0, 0.999);
        let glow = Vec4::new(cutoff, 1.0 / (1.0 - cutoff), one("r_glowBloomDesaturation", 0.0), one("r_glowBloomIntensity0", 0.0));
        let glow_on = one("r_glow", 0.0) != 0.0 && glow.w > 0.0;
        let glow_blur = Vec4::new(one("r_glowRadius0", 0.0), glow_on as u8 as f32, 0.0, 0.0);
        let [tint_base, tint_delta, bias] = if one("r_filmEnable", 0.0) == 0.0 {
            Self::NO_FILM
        } else {
            let contrast = one("r_filmContrast", 1.0);
            let (light, dark) = (three("r_filmLightTint"), three("r_filmDarkTint"));
            let brightness = one("r_filmBrightness", 0.0) + 0.5 * (1.0 - contrast);
            [
                (dark * contrast).extend(one("r_filmInvert", 0.0)),
                ((light - dark) * contrast).extend(0.0),
                Vec3::splat(brightness).extend(one("r_filmDesaturation", 0.0)),
            ]
        };
        FilmUniform { tint_base, tint_delta, bias, glow, glow_blur }
    }
}

/// Debug aid: `COD4RW_FILM=cod4` grades maps exactly as CoD4's film does
/// (see [`FilmUniform::balanced`]), for comparing.
fn cod4_film() -> bool {
    std::env::var("COD4RW_FILM").is_ok_and(|v| v.eq_ignore_ascii_case("cod4"))
}

/// The map's film: the vision set its art script picks
/// (`VisionSetNaked("mp_crash")`), else the one named after the map; and its
/// `r_filmContrast`.
fn film_of(content: &Content, map: &str) -> Option<(FilmUniform, f32)> {
    let art = raw_file(content, &format!("maps/createart/{map}_art.gsc"))
        .or_else(|| raw_file(content, &format!("maps/mp/createart/{map}_art.gsc")));
    let vision = art
        .and_then(|t| {
            // Multiplayer's call, or the campaign's `set_vision_set( "name" )`.
            let at = t.find("VisionSetNaked(").or_else(|| t.find("set_vision_set("))?;
            t[at..].split('"').nth(1).map(str::to_owned)
        })
        .unwrap_or_else(|| map.to_owned());
    let text = raw_file(content, &format!("vision/{vision}.vision"))?;
    let film = FilmUniform::parse(&text);
    let contrast = film.film_on().then(|| FilmUniform::value(&text, "r_filmContrast")).flatten().unwrap_or(1.0);
    Some((film, contrast))
}

/// Grade the frame on the camera that finishes it (the viewmodel camera
/// exposes and tonemaps the whole image): the map's film, or night
/// vision's. With the map's vision file, its glow stands in for Bevy's
/// bloom.
fn apply_film(
    mut commands: Commands,
    time: Res<Time>,
    look: Res<MapLook>,
    night_vision: Res<NightVisions>,
    cameras: Query<
        (Entity, Option<&FilmUniform>, Has<Bloom>, Option<&crate::splitscreen::SlotViewModelCamera>),
        Or<(With<crate::splitscreen::SlotViewModelCamera>, With<crate::ui::ScopeCamera>)>,
    >,
) {
    // The map's film tints grey unless the Film Tint setting is on; night
    // vision's keeps its green. Each player's view by their goggles.
    // The Bloom setting off takes the map's glow off too.
    let base = look.film.map(|f| if crate::ui::film_tint() { f } else { f.untinted() });
    let base = base.map(|f| if crate::settings_apply::bloom() { f } else { f.unglowing() });
    for (e, film, bloom, slot) in &cameras {
        let nv = night_vision.get(slot.map_or(0, |s| s.0));
        let wanted = if nv.showing(time.elapsed_secs()) { look.night.or(base) } else { base };
        match (wanted, film) {
            (Some(f), Some(&now)) if f == now => {}
            (Some(f), _) => {
                commands.entity(e).insert(f);
            }
            (None, Some(_)) => {
                commands.entity(e).remove::<FilmUniform>();
            }
            (None, None) => {}
        }
        if look.film.is_some() && bloom {
            commands.entity(e).remove::<Bloom>();
        }
    }
}

#[derive(Resource)]
struct FilmPipeline {
    layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    fullscreen_shader: FullscreenShader,
    fragment_shader: Handle<Shader>,
    /// The glow's passes: its layout, and the bright pass and the blurs
    /// across and down, all into [`GLOW_FORMAT`] textures.
    glow_layout: BindGroupLayoutDescriptor,
    glow_passes: [CachedRenderPipelineId; 3],
}

/// The glow's textures: a quarter of the frame each way.
const GLOW_FORMAT: TextureFormat = TextureFormat::Rgba16Float;

fn init_film_pipeline(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    fullscreen_shader: Res<FullscreenShader>,
    asset_server: Res<AssetServer>,
    pipeline_cache: Res<PipelineCache>,
) {
    let texture = || texture_2d(TextureSampleType::Float { filterable: true });
    let layout = BindGroupLayoutDescriptor::new(
        "film_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (texture(), sampler(SamplerBindingType::Filtering), uniform_buffer::<FilmUniform>(true), texture()),
        ),
    );
    let glow_layout = BindGroupLayoutDescriptor::new(
        "glow_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (texture(), sampler(SamplerBindingType::Filtering), uniform_buffer::<FilmUniform>(true)),
        ),
    );
    let sampler = render_device.create_sampler(&SamplerDescriptor {
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        ..default()
    });
    let glow_shader: Handle<Shader> = asset_server.load("embedded://cod4rw/glow.wgsl");
    let glow_passes = ["setup", "blur_across", "blur_down"].map(|entry| {
        pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(format!("glow {entry}").into()),
            layout: vec![glow_layout.clone()],
            vertex: fullscreen_shader.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: glow_shader.clone(),
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState { format: GLOW_FORMAT, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        })
    });
    commands.insert_resource(FilmPipeline {
        layout,
        sampler,
        fullscreen_shader: fullscreen_shader.clone(),
        fragment_shader: asset_server.load("embedded://cod4rw/film.wgsl"),
        glow_layout,
        glow_passes,
    });
}

/// A view's glow textures: the bright pass and the blur bounce between
/// them.
#[derive(Component)]
struct ViewGlow([CachedTexture; 2]);

fn prepare_glow_textures(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    mut cache: ResMut<TextureCache>,
    views: Query<(Entity, &ExtractedView), With<FilmUniform>>,
) {
    for (entity, view) in &views {
        let size = Extent3d { width: (view.viewport.z / 4).max(1), height: (view.viewport.w / 4).max(1), depth_or_array_layers: 1 };
        let texture = |label: &'static str, cache: &mut TextureCache| {
            cache.get(
                &render_device,
                TextureDescriptor {
                    label: Some(label),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: GLOW_FORMAT,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let textures = [texture("glow_a", &mut cache), texture("glow_b", &mut cache)];
        commands.entity(entity).insert(ViewGlow(textures));
    }
}

impl SpecializedRenderPipeline for FilmPipeline {
    type Key = TextureFormat;

    fn specialize(&self, format: TextureFormat) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("film".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen_shader.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.fragment_shader.clone(),
                targets: vec![Some(ColorTargetState { format, blend: None, write_mask: ColorWrites::ALL })],
                ..default()
            }),
            ..default()
        }
    }
}

#[derive(Component)]
struct CameraFilmPipeline(CachedRenderPipelineId);

fn prepare_film_pipelines(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    mut pipelines: ResMut<SpecializedRenderPipelines<FilmPipeline>>,
    film_pipeline: Res<FilmPipeline>,
    views: Query<(Entity, &ExtractedView), With<FilmUniform>>,
) {
    for (entity, view) in &views {
        let id = pipelines.specialize(&pipeline_cache, &film_pipeline, view.target_format);
        commands.entity(entity).insert(CameraFilmPipeline(id));
    }
}

/// One full-screen pass of `pipeline` into `target`.
fn fullscreen_pass(ctx: &mut RenderContext, label: &str, pipeline: &RenderPipeline, bind_group: &BindGroup, index: u32, target: &TextureView) {
    let mut pass = ctx.command_encoder().begin_render_pass(&RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(RenderPassColorAttachment {
            view: target,
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
    pass.set_bind_group(0, bind_group, &[index]);
    pass.draw(0..3, 0..1);
}

fn film(
    view: ViewQuery<(&ViewTarget, &CameraFilmPipeline, &DynamicUniformIndex<FilmUniform>, &FilmUniform, &ViewGlow)>,
    film_pipeline: Res<FilmPipeline>,
    pipeline_cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<FilmUniform>>,
    mut ctx: RenderContext,
) {
    let (target, pipeline, index, look, glow) = view.into_inner();
    let (Some(pipeline), Some(uniforms)) = (pipeline_cache.get_render_pipeline(pipeline.0), uniforms.binding()) else { return };
    let post_process = target.post_process_write();
    let [a, b] = &glow.0;
    // The glow: the bright pass into one texture, blurred across into the
    // other and down back into the first.
    let passes = film_pipeline.glow_passes.map(|id| pipeline_cache.get_render_pipeline(id));
    if let (true, [Some(setup), Some(across), Some(down)]) = (look.glow_on(), passes) {
        let layout = pipeline_cache.get_bind_group_layout(&film_pipeline.glow_layout);
        let bind = |ctx: &RenderContext, source: &TextureView| {
            ctx.render_device()
                .create_bind_group("glow_bind_group", &layout, &BindGroupEntries::sequential((source, &film_pipeline.sampler, uniforms.clone())))
        };
        let groups = [bind(&ctx, post_process.source), bind(&ctx, &a.default_view), bind(&ctx, &b.default_view)];
        fullscreen_pass(&mut ctx, "glow setup", setup, &groups[0], index.index(), &a.default_view);
        fullscreen_pass(&mut ctx, "glow blur across", across, &groups[1], index.index(), &b.default_view);
        fullscreen_pass(&mut ctx, "glow blur down", down, &groups[2], index.index(), &a.default_view);
    }
    let bind_group = ctx.render_device().create_bind_group(
        "film_bind_group",
        &pipeline_cache.get_bind_group_layout(&film_pipeline.layout),
        &BindGroupEntries::sequential((post_process.source, &film_pipeline.sampler, uniforms, &a.default_view)),
    );
    fullscreen_pass(&mut ctx, "film", pipeline, &bind_group, index.index(), post_process.destination);
}

// --- night vision

/// `nightVisionFadeInOutTime` (to black and back, switching) and
/// `nightVisionPowerOnTime` (black to night vision); CoD4's defaults live
/// in its executable, these are their feel.
const NV_FADE: f32 = 0.1;
const NV_POWER_ON: f32 = 0.3;

/// Each local player's night vision (by their place).
#[derive(Resource, Debug, Default)]
pub struct NightVisions(pub [NightVision; crate::splitscreen::MAX_PLAYERS]);

impl NightVisions {
    pub fn get(&self, slot: usize) -> &NightVision {
        &self.0[slot.min(crate::splitscreen::MAX_PLAYERS - 1)]
    }
}

/// A local player's night vision: on or off, and when it was switched.
#[derive(Debug, Clone, Copy)]
pub struct NightVision {
    pub on: bool,
    since: f32,
}

impl Default for NightVision {
    fn default() -> NightVision {
        NightVision { on: false, since: -1000.0 }
    }
}

impl NightVision {
    /// The night vision view is up (past the switch's fade to black).
    pub fn showing(&self, now: f32) -> bool {
        (now - self.since >= NV_FADE) == self.on
    }

    /// How black the screen is with a switch: fading to black, then from
    /// it (more slowly as the goggles power up).
    pub fn black(&self, now: f32) -> f32 {
        let t = now - self.since;
        if t < NV_FADE {
            return t / NV_FADE;
        }
        let back = if self.on { NV_POWER_ON } else { NV_FADE };
        (1.0 - (t - NV_FADE) / back).max(0.0)
    }

    fn switch(&mut self, on: bool, now: f32) {
        self.on = on;
        self.since = now;
    }
}

/// N switches night vision while playing; dying takes it off.
fn night_vision(
    time: Res<Time>,
    players: Query<(&crate::splitscreen::LocalSlot, &crate::splitscreen::PlayerInput, Has<crate::combat::Dead>)>,
    mut night_vision: ResMut<NightVisions>,
    mut sfx: ResMut<crate::audio::Sfx>,
) {
    let now = time.elapsed_secs();
    for (slot, input, dead) in &players {
        let Some(nv) = night_vision.0.get_mut(slot.0) else { continue };
        if dead {
            if nv.on {
                *nv = NightVision::default();
            }
            continue;
        }
        let settled = now - nv.since >= NV_FADE + NV_POWER_ON;
        if input.live && settled && input.keys.just_pressed(KeyCode::KeyN) {
            let on = !nv.on;
            nv.switch(on, now);
            sfx.play(if on { "item_nightvision_on" } else { "item_nightvision_off" }, None);
        }
    }
}

// --- the sun

/// The map's `sunflare_t`, if it has one, and its flare's image.
fn sun_of(content: &mut Content, images: &mut Assets<Image>) -> Option<Sun> {
    let zi = crate::content::MAP_ZONE;
    let flare = content.zones[zi].gfx_world()?.sun_flare.clone();
    if !flare.valid {
        return None;
    }
    let image = flare.flare.and_then(|id| {
        let semantic = content.zones[zi].material(id)?.textures.first()?.semantic;
        content.material_texture(zi, id, semantic, true, images)
    });
    Some(Sun { dir: crate::units::dir(flare.fx_position).normalize_or_zero(), flare, image })
}

/// IW3's `2d` materials blended ONE/ONE: the flare and the glare.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
struct SunMaterial {
    #[uniform(0)]
    color: Vec4,
    #[texture(1)]
    #[sampler(2)]
    texture: Handle<Image>,
}

impl UiMaterial for SunMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://cod4rw/sun.wgsl".into()
    }

    fn specialize(descriptor: &mut RenderPipelineDescriptor, _key: UiMaterialKey<Self>) {
        let target = descriptor.fragment.as_mut().and_then(|f| f.targets.first_mut()).and_then(|t| t.as_mut());
        if let Some(target) = target {
            let add = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
            target.blend = Some(BlendState { color: add, alpha: BlendComponent::OVER });
        }
    }
}

#[derive(Component)]
enum SunPart {
    Flare,
    Glare,
    Blind,
}

/// Under the HUD's layers.
const SUN_Z: i32 = 900;

/// How much of each effect is showing: the flare's visibility, the blind's
/// darkening and the glare's lightening, each easing towards its target over
/// the map's fade times.
#[derive(Default)]
struct SunState {
    visible: f32,
    darken: f32,
    lighten: f32,
}

/// Move `x` towards `target`, taking `up` seconds to rise by 1 and `down`
/// to fall by 1.
fn approach(x: f32, target: f32, dt: f32, up: f32, down: f32) -> f32 {
    if target > x { (x + dt / up.max(1e-3)).min(target) } else { (x - dt / down.max(1e-3)).max(target) }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn sun_effects(
    mut commands: Commands,
    time: Res<Time>,
    look: Res<MapLook>,
    spatial: SpatialQuery,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut parts: Query<(&SunPart, &mut Node, &mut Visibility, Option<&MaterialNode<SunMaterial>>, Option<&mut BackgroundColor>)>,
    mut materials: ResMut<Assets<SunMaterial>>,
    mut state: Local<SunState>,
) {
    // Over the whole window: not in splitscreen.
    let Some(sun) = look.sun.as_ref().filter(|_| !crate::splitscreen::active()) else { return };
    if parts.is_empty() {
        let full = Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() };
        let flare = MaterialNode(materials.add(SunMaterial { color: Vec4::ZERO, texture: sun.image.clone().unwrap_or_default() }));
        let glare = MaterialNode(materials.add(SunMaterial { color: Vec4::ZERO, texture: Handle::default() }));
        commands.spawn((Name::new("sun flare"), SunPart::Flare, Node { position_type: PositionType::Absolute, ..default() }, flare, GlobalZIndex(SUN_Z)));
        commands.spawn((Name::new("sun glare"), SunPart::Glare, full.clone(), glare, GlobalZIndex(SUN_Z)));
        commands.spawn((Name::new("sun blind"), SunPart::Blind, full, BackgroundColor(Color::NONE), GlobalZIndex(SUN_Z)));
        return;
    }
    let Ok((cam, cam_tf)) = camera.single() else { return };
    let f = &sun.flare;
    let dt = time.delta_secs();
    let ms = |t: i32| t as f32 / 1000.0;
    let dot = cam_tf.forward().dot(sun.dir);
    // Nothing solid between the eye and the sky towards the sun.
    let clear = dot > 0.0
        && Dir3::new(sun.dir).is_ok_and(|d| {
            spatial.cast_ray(cam_tf.translation(), d, crate::units::u(32768.0), true, &crate::collision::sight_filter()).is_none()
        });
    let ramp = |min: f32, max: f32| if max > min { ((dot - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    state.visible = approach(state.visible, clear as u8 as f32, dt, ms(f.flare_fade_in), ms(f.flare_fade_out));
    let blind = if clear { f.blind_max_darken * ramp(f.blind_min_dot, f.blind_max_dot) } else { 0.0 };
    state.darken = approach(state.darken, blind, dt, ms(f.blind_fade_in) / f.blind_max_darken.max(1e-3), ms(f.blind_fade_out) / f.blind_max_darken.max(1e-3));
    let glare = if clear { f.glare_max_lighten * ramp(f.glare_min_dot, f.glare_max_dot) } else { 0.0 };
    state.lighten = approach(state.lighten, glare, dt, ms(f.glare_fade_in) / f.glare_max_lighten.max(1e-3), ms(f.glare_fade_out) / f.glare_max_lighten.max(1e-3));

    // The flare: centred on the sun, growing and brightening as it nears the
    // middle of the view (sizes in the 480-line virtual screen).
    let frac = ramp(f.flare_min_dot, f.flare_max_dot);
    let alpha = f.flare_max_alpha * frac * state.visible;
    let at = (dot > 0.0).then(|| cam.world_to_viewport(cam_tf, cam_tf.translation() + sun.dir * 1000.0).ok()).flatten();
    let size = (f.flare_min_size + (f.flare_max_size - f.flare_min_size) * frac) * window.height() / 480.0;
    for (part, mut node, mut vis, material, background) in &mut parts {
        let (shown, color) = match part {
            SunPart::Flare => {
                if let Some(at) = at {
                    node.left = px(at.x - size * 0.5);
                    node.top = px(at.y - size * 0.5);
                    node.width = px(size);
                    node.height = px(size);
                }
                (at.is_some() && alpha > 0.001, Vec4::new(1.0, 1.0, 1.0, alpha))
            }
            SunPart::Glare => (state.lighten > 0.001, Vec4::new(1.0, 1.0, 1.0, state.lighten)),
            SunPart::Blind => {
                if let Some(mut bg) = background {
                    bg.0 = Color::srgba(0.0, 0.0, 0.0, state.darken);
                }
                (state.darken > 0.001, Vec4::ZERO)
            }
        };
        vis.set_if_neq(if shown { Visibility::Inherited } else { Visibility::Hidden });
        if let Some(m) = material.filter(|_| shown).and_then(|m| materials.get_mut(&m.0)) {
            let mut m = m;
            m.color = color;
        }
    }
}

/// Debug aid: with `COD4RW_SUNTEST=<dir>`, look at the sun from 5 s in,
/// screenshot it (`sun.png`), then 15 degrees beside it (`beside.png`), and
/// exit.
#[derive(Resource)]
struct SunTest(std::path::PathBuf);

fn sun_test(
    mut commands: Commands,
    time: Res<Time>,
    test: Res<SunTest>,
    content: Option<Res<Content>>,
    mut player: Query<&mut crate::movement::ViewAngles, With<crate::player::LocalPlayer>>,
    mut step: Local<u8>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let t = time.elapsed_secs();
    let Ok(mut view) = player.single_mut() else { return };
    // The sun's direction even when its effects are off, to compare.
    let dir = content
        .and_then(|c| c.zones[crate::content::MAP_ZONE].gfx_world().map(|w| crate::units::dir(w.sun_flare.fx_position)))
        .and_then(|d| d.try_normalize())
        .unwrap_or(Vec3::new(0.0, 0.5, -1.0).normalize());
    let (yaw, pitch) = ((-dir.x).atan2(-dir.z), dir.y.asin());
    let beside = (*step >= 1) as u8 as f32 * 15f32.to_radians();
    if t > 5.0 {
        view.yaw = yaw + beside;
        view.pitch = pitch;
    }
    let shot = |commands: &mut Commands, name: &str| {
        std::fs::create_dir_all(&test.0).ok();
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.0.join(name)));
    };
    match *step {
        0 if t > 8.0 => {
            shot(&mut commands, "sun.png");
            *step = 1;
        }
        1 if t > 9.0 => {
            shot(&mut commands, "beside.png");
            *step = 2;
        }
        2 if t > 10.0 => {
            exit.write(AppExit::Success);
        }
        _ => {}
    }
}

/// Debug aid: with `COD4RW_NVTEST=<dir>`, screenshot the view (`off.png`),
/// switch night vision on (`to_black.png`, `powering_up.png`, `on.png`), off again
/// (`after.png`), and exit.
#[derive(Resource)]
struct NvTest(std::path::PathBuf);

fn nv_test(
    mut commands: Commands,
    time: Res<Time>,
    test: Res<NvTest>,
    mut night_visions: ResMut<NightVisions>,
    mut step: Local<usize>,
    mut exit: MessageWriter<AppExit>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    let t = time.elapsed_secs();
    let night_vision = &mut night_visions.0[0];
    let steps: [(f32, &str); 7] =
        [(5.0, "off.png"), (6.0, "on"), (6.06, "to_black.png"), (6.25, "powering_up.png"), (7.0, "on.png"), (8.0, "off"), (9.0, "after.png")];
    let Some(&(at, what)) = steps.get(*step).filter(|s| t >= s.0) else {
        if *step == steps.len() && t > 10.0 {
            exit.write(AppExit::Success);
        }
        return;
    };
    let _ = at;
    *step += 1;
    match what {
        "on" => night_vision.switch(true, t),
        "off" => night_vision.switch(false, t),
        name => {
            std::fs::create_dir_all(&test.0).ok();
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(test.0.join(name)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_killhouse_film() {
        let text = "r_filmEnable \"1\"\nr_filmContrast \"0.985\"\nr_filmBrightness \"0.0346\"\nr_filmDesaturation \"0.2\"\n\
                    r_filmInvert \"0\"\nr_filmLightTint \"1.1 1.05 0.85\"\nr_filmDarkTint \"0.956127 0.854841 0.777573\"\n";
        let f = FilmUniform::parse(text);
        assert!((f.bias.w - 0.2).abs() < 1e-6);
        assert!((f.tint_base.x - 0.956127 * 0.985).abs() < 1e-5);
        assert!((f.tint_delta.z - (0.85 - 0.777573) * 0.985).abs() < 1e-5);
        assert!((f.bias.x - (0.0346 + 0.5 * 0.015)).abs() < 1e-5);
        assert!(!FilmUniform::parse("r_filmEnable \"0\"").film_on());
        assert!(!f.glow_on());
    }

    #[test]
    fn reads_vacant_glow() {
        let text = "r_glow \"1\"\nr_glowRadius0 \"2.1\"\nr_glowBloomCutoff \"0.45\"\nr_glowBloomDesaturation \"0\"\n\
                    r_glowBloomIntensity0 \"1.00002\"\nr_glowBloomIntensity1 \"5\"\n";
        let f = FilmUniform::parse(text);
        assert!(f.glow_on() && !f.film_on());
        assert!((f.glow.y - 1.0 / 0.55).abs() < 1e-5);
        assert!((f.glow.w - 1.00002).abs() < 1e-6);
        assert_eq!(f.glow_blur.x, 2.1);
        assert!(!FilmUniform::parse("r_glow \"0\"\nr_glowBloomIntensity0 \"1\"").glow_on());
    }

    #[test]
    fn night_vision_switches_through_black() {
        let mut nv = NightVision::default();
        assert!(!nv.showing(5.0) && nv.black(5.0) == 0.0);
        nv.switch(true, 10.0);
        assert!(!nv.showing(10.05) && (nv.black(10.05) - 0.5).abs() < 1e-4);
        assert!(nv.showing(10.1) && (nv.black(10.25) - 0.5).abs() < 1e-4);
        assert!(nv.black(10.5) == 0.0);
        nv.switch(false, 20.0);
        assert!(nv.showing(20.05) && !nv.showing(20.1));
        assert!((nv.black(20.15) - 0.5).abs() < 1e-4 && nv.black(20.3) == 0.0);
    }

    #[test]
    fn balanced_film_keeps_mid_grey() {
        // Crossfire's: a 1.56x stretch that clipped everything over 0.59.
        let text = "r_filmEnable 1
r_filmContrast \"1.55798\"
r_filmBrightness \"0.265218\"
r_filmDesaturation \"0.45\"
                    r_filmLightTint \"1.0209 1.05 1.11\"
r_filmDarkTint \"1.08003 1.08 1.13691\"
";
        let f = FilmUniform::parse(text).balanced(1.55798);
        let grade = |c: f32| {
            let (b, d) = (f.tint_base.truncate(), f.tint_delta.truncate());
            ((b + d * c) * c + f.bias.truncate()).dot(Vec3::new(0.299, 0.587, 0.114))
        };
        // Mid-grey stays, give or take half the black lift.
        assert!((grade(0.5) - 0.5).abs() < 0.025, "{}", grade(0.5));
        assert!(grade(0.9) < 1.0 && grade(0.1) > 0.08, "{} {}", grade(0.9), grade(0.1));
    }

    #[test]
    fn untinted_film_is_grey() {
        let f = FilmUniform::parse("r_filmEnable 1
r_filmLightTint \"1.1 1.05 0.85\"
r_filmDarkTint \"0.956 0.855 0.778\"
");
        let g = f.untinted();
        for v in [g.tint_base, g.tint_delta] {
            assert!((v.x - v.y).abs() < 1e-6 && (v.y - v.z).abs() < 1e-6, "{v}");
        }
        assert_eq!(g.bias, f.bias);
    }

    #[test]
    fn approaches_at_fade_rates() {
        assert!((approach(0.0, 1.0, 0.25, 0.5, 1.0) - 0.5).abs() < 1e-6);
        assert!((approach(1.0, 0.0, 0.25, 0.5, 1.0) - 0.75).abs() < 1e-6);
        assert_eq!(approach(0.9, 1.0, 1.0, 0.5, 1.0), 1.0);
    }
}
