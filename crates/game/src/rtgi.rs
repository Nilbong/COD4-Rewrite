//! Ray-traced lighting (Options > Graphics > Lighting: Ray Traced Low/High), with
//! Bevy's Solari: the sun and the sky traced through the map, with bounce
//! light (ReSTIR direct and indirect light, a world-space radiance cache),
//! in place of the baked lightmaps, light grid and shadow maps.
//!
//! The map is lit as usual (Baked) unless the setting asks for ray tracing,
//! the GPU can trace rays ([`supported`]) and one player is playing (each
//! splitscreen view would trace the whole frame again). Then, as a match
//! loads:
//! - opaque and alpha-tested world surfaces draw into Solari's G-buffer
//!   with their own material, deferred (`shaders/world_deferred.wgsl`:
//!   CoD4's normal maps, and the baked lightmap as a floor Solari adds its
//!   light to, so rooms are never darker than Baked); props draw as plain
//!   deferred standard materials;
//! - each also gets a copy of its mesh in the form Solari traces (position,
//!   normal, UV, tangent, 32-bit indices). Alpha-tested ones (foliage,
//!   fences) aren't traced, as Solari traces them solid: they're lit but
//!   cast no shadow. Skinned characters keep their usual lighting;
//! - the sky lights the map through an emissive dome only the rays see
//!   (Solari has no sky light of its own), tinted like the sky;
//! - opaque unlit surfaces (lamps' bulbs, light boxes, screens) glow and
//!   light what's around them: CoD4's interiors were lit by lights only its
//!   lightmaps kept, so rooms without such fixtures come out dark;
//! - the sun's shadow maps go (Solari traces its shadows) and screen-space
//!   AO with them; the viewmodel camera shares the traced image.
//!
//! Low traces the world's brushes and props over 2 m; High every prop, and
//! a finer sky dome. Solari's shaders take some seconds to compile the
//! first time (the map shows its clear colour until they have), and without
//! DLSS (NVIDIA only) its own denoising leaves some grain in shade.

//!
//! A test feature: built only with `--features raytracing` (the Solari code
//! is in [`traced`]); otherwise lighting is always Baked and the option hidden.

use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// The lighting the setting asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lighting {
    Baked,
    RayTracedLow,
    RayTracedHigh,
}

static SUPPORTED: AtomicBool = AtomicBool::new(false);

/// The GPU can trace rays (wgpu's ray query and binding arrays).
pub fn supported() -> bool {
    SUPPORTED.load(Ordering::Relaxed)
}

/// The lighting this match uses: the setting's, if it can be traced.
pub fn lighting() -> Lighting {
    let wanted = crate::ui::lighting();
    if wanted == Lighting::Baked || !supported() || crate::splitscreen::count() > 1 {
        Lighting::Baked
    } else {
        wanted
    }
}

pub struct RtgiPlugin;

impl Plugin for RtgiPlugin {
    fn finish(&self, _app: &mut App) {
        #[cfg(feature = "raytracing")]
        traced::check_support(_app);
    }

    fn build(&self, _app: &mut App) {
        #[cfg(feature = "raytracing")]
        traced::build(_app);
    }
}

#[cfg(feature = "raytracing")]
mod traced;
