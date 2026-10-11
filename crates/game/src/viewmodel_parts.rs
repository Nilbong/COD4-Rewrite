//! Viewmodel parts that only belong on screen while reloading: World at
//! War's .357's speedloader, its rounds and the spent cases, which its
//! model parks beside the gun (6-13 units off) and only its reload brings
//! in. Unhidden, drawing the gun swept them up across the view.

use crate::models::Skeleton;
use crate::weapons::WeaponState;
use bevy::prelude::*;

pub struct ViewModelPartsPlugin;

impl Plugin for ViewModelPartsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, reload_parts.after(crate::weapons::WeaponSet).run_if(crate::state::in_game));
    }
}

/// (Weapon name prefix, joints shown only while reloading.)
const RELOAD_ONLY: [(&str, &[&str]); 1] = [(
    "357magnum",
    &["tag_speedloader", "tag_bullets", "tag_spent0", "tag_spent1", "tag_spent2", "tag_spent3", "tag_spent4", "tag_spent5"],
)];

fn reload_parts(
    players: Query<(&crate::splitscreen::LocalSlot, &WeaponState)>,
    mut viewmodels: Query<(&crate::viewmodel::ViewModelSlot, &mut Skeleton), With<crate::viewmodel::ViewModelRoot>>,
) {
    for (slot, mut skeleton) in &mut viewmodels {
        let Some((_, w)) = players.iter().find(|p| p.0.0 == slot.0) else { continue };
        let name = w.def.name.trim_start_matches("t4_");
        let Some((_, joints)) = RELOAD_ONLY.iter().find(|(p, _)| name.starts_with(p)) else { continue };
        let reloading = w.reloading();
        for j in joints.iter() {
            if reloading { skeleton.show(j) } else { skeleton.hide(j) }
        }
    }
}
