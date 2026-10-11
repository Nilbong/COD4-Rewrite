//! The scope cameras' own sun shadows. Bevy gives every active camera its
//! own cascades of the sun's shadow map, so a scoped lens drew the sun's
//! three cascades a second time (1.5-3 ms of GPU while aimed).
//! `COD4RW_LENSCASCADES=n` keeps only the scope view's nearest n
//! (beyond the last one kept, the lens view is unshadowed).

use bevy::prelude::*;

pub struct ScopeShadowsPlugin;

impl Plugin for ScopeShadowsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            trim.after(bevy::light::SimulationLightSystems::UpdateDirectionalLightCascades)
                .before(bevy::light::SimulationLightSystems::UpdateLightFrusta),
        );
    }
}

fn keep() -> Option<usize> {
    static KEEP: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *KEEP.get_or_init(|| std::env::var("COD4RW_LENSCASCADES").ok().and_then(|v| v.parse().ok()))
}

#[allow(clippy::type_complexity)]
fn trim(
    scopes: Query<Entity, Or<(With<crate::ui::scope::ScopeCamera>, With<crate::ui::scope3d::ScopeCamera3d>)>>,
    mut suns: Query<&mut bevy::light::Cascades>,
) {
    let Some(keep) = keep() else { return };
    for scope in &scopes {
        for mut cascades in &mut suns {
            if let Some(list) = cascades.bypass_change_detection().cascades.get_mut(&scope) {
                list.truncate(keep);
            }
        }
    }
}
