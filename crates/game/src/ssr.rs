//! Screen-space reflections for the showcase (`COD4RW_SHOWCASE=1`): wet
//! ground, puddles and glossy surfaces reflect what's on screen.
//!
//! The world is drawn forward, so Bevy's own SSR (deferred only) can't be
//! used: `shaders/world.wgsl` marches the reflected ray through the depth
//! prepass itself and, where it hits, takes the colour from
//! [`history`]: the world camera's lit image (before the gun and the
//! post-processing) copied here after its transparent pass each frame, so
//! a reflection is a frame old. Where the ray leaves the screen or finds
//! nothing, the reflection probe shows instead.

use crate::player::MainCamera;
use bevy::prelude::*;
use bevy::render::extract_component::{ExtractComponent, ExtractComponentPlugin};
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{Extent3d, TextureFormat};
use bevy::render::renderer::{RenderContext, ViewQuery};
use bevy::render::texture::GpuImage;
use bevy::render::view::ViewTarget;
use std::sync::OnceLock;

pub struct SsrPlugin;

impl Plugin for SsrPlugin {
    fn build(&self, app: &mut App) {
        let image = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            images.add(Image::new_target_texture(1920, 1080, TextureFormat::Rgba16Float, None))
        };
        HISTORY.set(image).ok();
        if !crate::atmos::climate::enabled() {
            return;
        }
        app.add_plugins(ExtractComponentPlugin::<SsrSource>::default())
            .add_systems(Update, (mark_camera, size_history).run_if(crate::atmos::climate::on));
        if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
            use bevy::core_pipeline::core_3d::main_transparent_pass_3d;
            use bevy::core_pipeline::schedule::{Core3d, Core3dSystems};
            render_app.add_systems(Core3d, copy_history.in_set(Core3dSystems::MainPass).after(main_transparent_pass_3d));
        }
    }
}

static HISTORY: OnceLock<Handle<Image>> = OnceLock::new();

/// The world camera's last image, every world material binds
/// (`WorldLighting::ssr_history`).
pub fn history() -> Option<Handle<Image>> {
    HISTORY.get().cloned()
}

/// The camera whose image is kept (player 1's world camera).
#[derive(Component, Clone, ExtractComponent)]
pub struct SsrSource;

fn mark_camera(mut commands: Commands, cameras: Query<Entity, (With<MainCamera>, Without<SsrSource>)>) {
    if crate::splitscreen::active() {
        return;
    }
    for e in &cameras {
        commands.entity(e).insert(SsrSource);
    }
}

/// The history the size of the window (the copy needs them equal).
fn size_history(window: Option<Single<&Window, With<bevy::window::PrimaryWindow>>>, mut images: ResMut<Assets<Image>>) {
    let (Some(window), Some(handle)) = (window, HISTORY.get()) else { return };
    let size = Extent3d { width: window.physical_width().max(1), height: window.physical_height().max(1), depth_or_array_layers: 1 };
    if images.get(handle).is_some_and(|i| i.texture_descriptor.size != size) {
        if let Some(mut image) = images.get_mut(handle) {
            image.resize(size);
        }
    }
}

/// After the world camera's transparent pass: its image into the history.
fn copy_history(view: ViewQuery<&ViewTarget, With<SsrSource>>, images: Res<RenderAssets<GpuImage>>, mut ctx: RenderContext) {
    let target = view.into_inner();
    let Some(history) = HISTORY.get().and_then(|h| images.get(h)) else { return };
    let source = target.main_texture();
    if source.size() != history.texture.size() || source.format() != history.texture.format() {
        return;
    }
    ctx.command_encoder().copy_texture_to_texture(source.as_image_copy(), history.texture.as_image_copy(), source.size());
}
