//! Names over heads, as CoD4 draws them (`CG_DrawFriendlyNames`,
//! `CG_DrawCrosshairNames`, `CG_DrawOverheadNames`).
//!
//! A teammate's name shows over their head, in the team's colour, while
//! their head is in view and nothing solid is between it and the camera
//! (within `cg_overheadNamesMaxDist`); it stays 1.5 s after they're lost
//! from view (`cg_friendlyNameFadeOut`). An enemy's name shows, in red, only
//! while the crosshair is on them, after a quarter second and for a quarter
//! second after (`cg_enemyNameFadeIn`/`Out`; not in Hardcore, which has no
//! crosshair). Names shrink with distance, from full size at 256 units to
//! 0.6 at 1024 (`cg_overheadNamesNear`/`FarDist`, `FarScale`), with the
//! rank's icon and level before them.
//!
//! What's seen is worked out by [`update`] (it needs rays through the world),
//! what's drawn by [`paint`], per local player.

use super::{Painter, rank_icon};
use crate::bots::Bot;
use crate::combat::{Dead, Hitbox, Pawn, hostile};
use crate::movement::Mover;

use crate::splitscreen::{LocalSlot, SlotCamera};
use crate::units::u;
use avian3d::prelude::*;
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Mutex;

/// `g_TeamColor_MyTeam`, `g_TeamColor_EnemyTeam`.
const FRIENDLY: [f32; 3] = [0.4, 0.6, 0.85];
const ENEMY: [f32; 3] = [0.75, 0.25, 0.25];
/// CoD units.
const MAX_DIST: f32 = 10000.0;
const NEAR_DIST: f32 = 256.0;
const FAR_DIST: f32 = 1024.0;
const FAR_SCALE: f32 = 0.6;
/// Seconds.
const FRIENDLY_FADE_OUT: f32 = 1.5;
const ENEMY_FADE_IN: f32 = 0.25;
const ENEMY_FADE_OUT: f32 = 0.25;
/// Name height at full size, in 480-line units (`cg_overheadNamesSize`).
const NAME_HEIGHT: f32 = 13.0;
/// Over the eyes (CoD4 puts names 10 units over `j_head`, a little below
/// which the eyes are).
const OVER_HEAD: f32 = 7.0;

/// A name to draw for a local player.
#[derive(Clone)]
struct Tag {
    slot: usize,
    head: Vec3,
    name: String,
    color: [f32; 3],
    /// Rank (0-based) and prestige, if known.
    rank: Option<(i32, i32)>,
    alpha: f32,
}

/// When each (local player, pawn) name was first and last seen.
#[derive(Default)]
struct Seen {
    since: HashMap<(usize, Entity), (f32, f32, bool)>,
    tags: Vec<Tag>,
}

static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

/// Fade like `CG_FadeCrosshairNameAlpha`: nothing until seen for `fade_in`,
/// then full, fading out over `fade_out` after it was last seen.
fn alpha(now: f32, start: f32, last: f32, fade_in: f32, fade_out: f32) -> f32 {
    let since = now - last;
    if since >= fade_out || last - start < fade_in {
        return 0.0;
    }
    if fade_out <= 0.0 { 1.0 } else { ((fade_out - since) / fade_out).clamp(0.0, 1.0) }
}

/// Who each local player can see (teammates' heads, the enemy under the
/// crosshair), and how faded their names are.
#[allow(clippy::type_complexity)]
pub(super) fn update(
    time: Res<Time>,
    spatial: SpatialQuery,
    cameras: Query<(&GlobalTransform, &SlotCamera)>,
    players: Query<(Entity, &LocalSlot)>,
    pawns: Query<(Entity, &Pawn, &Transform, &Mover, Has<Dead>, Option<&Bot>, &InheritedVisibility)>,
    hitboxes: Query<&Hitbox>,
) {
    let now = time.elapsed_secs();
    let mut guard = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = guard.get_or_insert_default();
    let filter = crate::collision::bullet_filter();
    let mut tags = Vec::new();
    for (cam, slot) in &cameras {
        let Some((me, _)) = players.iter().find(|p| p.1.0 == slot.0) else { continue };
        let Ok((_, mine, ..)) = pawns.get(me) else { continue };
        let eye = cam.translation();
        let forward = cam.forward();
        // The enemy under the crosshair (as the HUD's own aim check).
        let not_me = |e: Entity| hitboxes.get(e).map_or(true, |h| h.owner != me);
        let aimed = spatial
            .cast_ray_predicate(eye, forward, u(8192.0), true, &filter, &not_me)
            .and_then(|h| hitboxes.get(h.entity).ok())
            .map(|h| h.owner);
        for (e, pawn, tf, mover, dead, bot, shown) in &pawns {
            if e == me || dead || !shown.get() {
                continue;
            }
            let enemy = hostile(pawn, mine);
            let head = tf.translation + Vec3::Y * (mover.eye_height + u(OVER_HEAD));
            let visible = if enemy {
                !crate::tdm::hardcore() && aimed == Some(e)
            } else {
                let to = head - eye;
                let dist = to.length();
                forward.dot(to) > 0.0
                    && dist <= u(MAX_DIST)
                    && {
                        // Nothing in the way but the two of us.
                        let skip = |h: Entity| hitboxes.get(h).map_or(true, |b| b.owner != me && b.owner != e);
                        let dir = Dir3::new(to).unwrap_or(Dir3::NEG_Z);
                        spatial.cast_ray_predicate(eye, dir, dist, true, &filter, &skip).is_none_or(|h| h.distance >= dist - 0.05)
                    }
            };
            let key = (slot.0, e);
            let entry = seen.since.entry(key).or_insert((now, f32::NEG_INFINITY, false));
            if visible {
                if !entry.2 {
                    entry.0 = now;
                }
                entry.1 = now;
            }
            entry.2 = visible;
            let a = if enemy {
                alpha(now, entry.0, entry.1, ENEMY_FADE_IN, ENEMY_FADE_OUT)
            } else {
                alpha(now, entry.0, entry.1, 0.0, FRIENDLY_FADE_OUT)
            };
            if a <= 0.0 {
                continue;
            }
            tags.push(Tag {
                slot: slot.0,
                head,
                name: pawn.name.clone(),
                color: if enemy { ENEMY } else { FRIENDLY },
                rank: bot.map(|b| b.rank),
                alpha: a,
            });
        }
    }
    // Forget pawns long gone.
    seen.since.retain(|_, (_, last, _)| now - *last < 10.0 || last.is_infinite());
    seen.tags = tags;
}

/// One local player's names, over the heads where they are on screen.
pub(super) fn paint(p: &mut Painter, slot: usize, (cam, cam_tf): (&Camera, &GlobalTransform)) {
    let guard = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let Some(seen) = guard.as_ref() else { return };
    // Window pixels per 480-line unit.
    let k = p.pl.h / 480.0;
    for tag in seen.tags.iter().filter(|t| t.slot == slot) {
        let Ok(px) = cam.world_to_viewport(cam_tf, tag.head) else { continue };
        let dist = (tag.head - cam_tf.translation()).length() / crate::units::INCH;
        let scale = if dist <= NEAR_DIST {
            1.0
        } else if dist <= FAR_DIST {
            let f = (dist - NEAR_DIST) / (FAR_DIST - NEAR_DIST);
            f * FAR_SCALE + 1.0 - f
        } else {
            FAR_SCALE
        };
        let height = NAME_HEIGHT * scale * k;
        let [r, g, b] = tag.color;
        let name_w = p.text(&tag.name, px.x, px.y, 5, 5, height, 0, [r, g, b, tag.alpha], 0.5, true);
        // The rank's icon and level, before the name.
        if let Some((rank, prestige)) = tag.rank {
            let icon = rank_icon(p.fe, rank, prestige);
            let size = height * 0.9;
            let level = (rank + 1).to_string();
            let left = px.x - name_w * 0.5 - 2.0 * scale * k;
            let level_w = p.text(&level, left, px.y, 5, 5, height * 0.8, 0, [1.0, 1.0, 1.0, tag.alpha], 1.0, true);
            let x = left - level_w - size - 1.0 * k;
            p.image(&icon, super::vr(x, px.y - size * 0.85, size, size, 5, 5), [1.0, 1.0, 1.0, tag.alpha], 1);
        }
    }
}
