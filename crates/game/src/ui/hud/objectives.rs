//! Objectives on the HUD (`_gameobjects.gsc`'s 2D and 3D icons and use
//! bars): Domination's flags over themselves and on the compass, as one to
//! defend (yours), capture (theirs) or capture (nobody's), and the capture
//! bar while taking one; Search and Destroy's bomb sites (to destroy, defend
//! or defuse), the dropped bomb for the attackers, the plant and defuse bars
//! and hints, the bomb you carry, and who won the round; Headquarters' HQ
//! likewise, with the time it has left once held.

use super::{Painter, WHITE, loc, vr};
use crate::combat::Team;
use crate::modes::{BombSite, Flag, Hq, Objectives};
use crate::units::u;
use bevy::prelude::*;

/// A flag's icon state for the player's team (`waypoint_<state>_a`).
fn flag_state(flag: &Flag, team: Team) -> &'static str {
    match flag.owner {
        Some(o) if o == team => "defend",
        Some(_) => "capture",
        None => "captureneutral",
    }
}

/// The HQ's icon state for the player's team (`waypoint_<state>`).
fn hq_state(hq: &Hq, team: Team) -> &'static str {
    match hq.owner {
        Some(o) if o == team => "defend",
        Some(_) => "capture",
        None => "captureneutral",
    }
}

/// A bomb site's icon state for the player's team: attackers destroy it
/// (then guard the bomb on it), defenders defend it (then defuse it). None
/// once destroyed, or while the bomb is planted at the other one.
fn site_state(site: &BombSite, o: &Objectives, team: Team) -> Option<&'static str> {
    let planted = o.bomb.and_then(|b| b.planted);
    // Sabotage's targets are their team's; Search and Destroy's the
    // defenders'.
    let attacking = site.team.map_or(o.attackers == Some(team), |t| t != team);
    match planted {
        _ if site.destroyed => None,
        Some(l) if l != site.label => None,
        Some(_) if attacking => Some("defend"),
        Some(_) => Some("defuse"),
        None if attacking => Some("target"),
        None => Some("defend"),
    }
}

/// The compass images of the states (their materials are in a zone
/// matches don't load, so they're drawn by image).
fn compass_color(state: &str) -> &'static str {
    match state {
        "defend" => "green",
        "captureneutral" => "white",
        _ => "red",
    }
}

/// The attackers see the bomb lying about.
fn loose_bomb(o: &Objectives, team: Team) -> Option<Vec3> {
    // Sabotage's is anyone's.
    o.bomb.filter(|b| b.carrier.is_none() && b.planted.is_none() && o.attackers.is_none_or(|a| a == team)).map(|b| b.pos)
}

/// A site's icon suffix: Search and Destroy's are lettered, Sabotage's
/// targets aren't (`waypoint_target`).
fn site_suffix(site: &BombSite) -> String {
    if site.team.is_some() { String::new() } else { format!("_{}", site.letter()) }
}

/// On the compass (`compass_waypoint_<state>_a`), at its edge when beyond
/// it. `place` turns a world point into compass space (-1..1), `icon`
/// draws there.
pub(super) fn compass_icons(
    p: &mut Painter,
    o: &Objectives,
    team: Team,
    place: &dyn Fn(Vec3) -> Vec2,
    icon: &dyn Fn(&mut Painter, &str, Vec2, f32, f32, f32),
) {
    let draw = |p: &mut Painter, material: String, at: Vec3| {
        let s = place(at);
        let edge = s.abs().max_element();
        icon(p, &material, if edge > 1.0 { s / edge } else { s }, 15.0, 0.0, 1.0);
    };
    for flag in &o.flags {
        draw(p, format!("compass_waypoint_{}_{}", compass_color(flag_state(flag, team)), flag.letter()), flag.pos);
    }
    for site in &o.sites {
        if let Some(state) = site_state(site, o, team) {
            draw(p, format!("compass_waypoint_{}{}", compass_color(state), site_suffix(site)), site.pos);
        }
    }
    if let Some(at) = loose_bomb(o, team) {
        draw(p, "compass_waypoint_yellow".into(), at);
    }
    if let Some(hq) = &o.hq {
        draw(p, format!("compass_waypoint_{}", compass_color(hq_state(hq, team))), hq.pos);
    }
}

/// Over each objective in the world (`waypoint_<state>_a`), on screen only.
pub(super) fn waypoints(p: &mut Painter, o: &Objectives, team: Team, (w, h): (f32, f32), camera: (&Camera, &GlobalTransform)) {
    let mut icons: Vec<(String, Vec3)> = Vec::new();
    for flag in &o.flags {
        // `(0,0,100)`: the icon's offset over the flag.
        icons.push((format!("waypoint_{}_{}", flag_state(flag, team), flag.letter()), flag.pos + Vec3::Y * u(100.0)));
    }
    for site in &o.sites {
        if let Some(state) = site_state(site, o, team) {
            icons.push((format!("waypoint_{state}{}", site_suffix(site)), site.pos + Vec3::Y * u(72.0)));
        }
    }
    if let Some(at) = loose_bomb(o, team) {
        icons.push(("waypoint_bomb".into(), at + Vec3::Y * u(32.0)));
    }
    if let Some(hq) = &o.hq {
        icons.push((format!("waypoint_{}", hq_state(hq, team)), hq.pos + Vec3::Y * u(48.0)));
    }
    let (w, h) = (w.max(1.0), h.max(1.0));
    let (cam, cam_tf) = camera;
    for (material, at) in icons {
        if cam_tf.forward().dot(at - cam_tf.translation()) <= 0.0 {
            continue;
        }
        let Ok(px) = cam.world_to_viewport(cam_tf, at) else { continue };
        // Stretched virtual coordinates, square on screen.
        let (sw, sh) = (16.0 / w * 640.0 * (h / 480.0), 16.0);
        let (vx, vy) = (px.x / w * 640.0, px.y / h * 480.0);
        p.image(&material, vr(vx - sw * 0.5, vy - sh * 0.5, sw, sh, 4, 4), [1.0, 1.0, 1.0, 0.85], 0);
    }
}

/// `createPrimaryProgressBar`: a label and how far along.
fn bar(p: &mut Painter, text: &str, progress: f32) {
    let (width, height, y) = (120.0, 8.0, 64.0);
    p.text(text, 0.0, y - 4.0, 2, 2, 0.4 * 48.0, 0, WHITE, 0.5, true);
    p.image("white", vr(-width * 0.5 - 1.0, y - 1.0, width + 2.0, height + 2.0, 2, 2), [0.0, 0.0, 0.0, 0.6], 1);
    p.image("white", vr(-width * 0.5, y, width * progress.clamp(0.0, 1.0), height, 2, 2), WHITE, 1);
}

/// The player's part in the objectives: capturing, planting or defusing
/// (with how far), what holding use would do, the bomb they carry, and the
/// round's result. `use_key` names the use button.
pub(super) fn status(p: &mut Painter, o: &Objectives, me: Entity, team: Team, at: Vec3, alive: bool, use_key: &str, now: f32) {
    if let Some((winner, key, text)) = o.round_over {
        let result = if winner == team { loc(p.fe, "MP_ROUND_WIN", "Round Win!") } else { loc(p.fe, "MP_ROUND_LOSS", "Round Loss") };
        p.text(&result, 0.0, -60.0, 2, 2, 0.75 * 48.0, 0, WHITE, 0.5, true);
        p.text(&loc(p.fe, key, text), 0.0, -30.0, 2, 2, 0.45 * 48.0, 0, WHITE, 0.5, true);
        return;
    }
    if o.sudden_death {
        p.text(&loc(p.fe, "MP_SUDDEN_DEATH", "Sudden Death"), 0.0, 46.0, 2, 1, 0.45 * 48.0, 0, WHITE, 0.5, true);
    }
    // Headquarters: how long the HQ has left once held.
    if let Some(left) = o.hq.as_ref().and_then(|h| h.expires_at).map(|at| (at - now).max(0.0)) {
        let text = format!("{} {}:{:02}", loc(p.fe, "MP_HQ_EXPIRES_IN", "HQ offline in"), (left / 60.0) as u32, left.ceil() as u32 % 60);
        p.text(&text, 0.0, 46.0, 2, 1, 0.35 * 48.0, 0, WHITE, 0.5, true);
    }
    if !alive {
        return;
    }
    // Headquarters: taking or destroying the HQ the player stands in.
    if let Some(hq) = o.hq.as_ref().filter(|h| h.contains(at)) {
        if let Some((_, progress)) = hq.capture.filter(|c| c.0 == team) {
            let text = if hq.owner.is_some() {
                loc(p.fe, "MP_DESTROYING_HQ", "Destroying HQ...")
            } else {
                loc(p.fe, "MP_CAPTURING_HQ", "Capturing HQ...")
            };
            bar(p, &text, progress);
            return;
        }
    }
    // Domination: taking the flag the player stands in.
    if let Some(progress) = o.flags.iter().filter(|f| f.contains(at)).find_map(|f| f.capture.filter(|c| c.0 == team).map(|c| c.1)) {
        bar(p, &loc(p.fe, "MP_CAPTURING_FLAG", "Capturing..."), progress);
        return;
    }
    let Some(bomb) = o.bomb else { return };
    if let Some(&(_, progress, defusing)) = o.using.iter().find(|u| u.0 == me) {
        let text = if defusing {
            loc(p.fe, "MP_DEFUSING_EXPLOSIVE", "Defusing Explosive...")
        } else {
            loc(p.fe, "MP_PLANTING_EXPLOSIVE", "Planting Explosive...")
        };
        bar(p, &text, progress);
        return;
    }
    let carrying = bomb.carrier == Some(me);
    if carrying {
        p.image("hud_suitcase_bomb", vr(-58.0, -100.0, 40.0, 40.0, 3, 3), WHITE, 1);
    }
    let planted_here = o.sites.iter().find(|s| Some(s.label) == bomb.planted);
    let defending = planted_here.is_some_and(|s| s.team.map_or(o.attackers != Some(team), |t| t == team));
    let hint = if carrying && o.sites.iter().any(|s| !s.destroyed && s.team != Some(team) && s.contains(at)) {
        Some(("PLATFORM_HOLD_TO_PLANT_EXPLOSIVES", "Hold [{+activate}] to plant the explosives"))
    } else if defending && at.distance(bomb.pos) < u(64.0) {
        Some(("PLATFORM_HOLD_TO_DEFUSE_EXPLOSIVES", "Hold [{+activate}] to defuse the explosives"))
    } else {
        None
    };
    // Hardcore hides the hints (the `cursorhints` menu).
    if let Some((key, fallback)) = hint.filter(|_| !crate::tdm::hardcore()) {
        // CoD4's string names the key as `&&1`.
        let text = loc(p.fe, key, fallback).replace("[{+activate}]", use_key).replace("&&1", use_key);
        p.text(&text, 0.0, 70.0, 2, 2, 0.4 * 48.0, 0, WHITE, 0.5, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::Bomb;

    #[test]
    fn flag_icons_follow_the_holder() {
        let mut f = Flag { label: 'B', pos: Vec3::ZERO, radius: 1.0, height: 1.0, owner: None, capture: None, contested: false };
        assert_eq!(flag_state(&f, Team::Allies), "captureneutral");
        f.owner = Some(Team::Allies);
        assert_eq!((flag_state(&f, Team::Allies), flag_state(&f, Team::Axis)), ("defend", "capture"));
    }

    #[test]
    fn site_icons_follow_the_bomb() {
        let site = |label| BombSite { label, min: Vec3::ZERO, max: Vec3::ONE, pos: Vec3::ZERO, destroyed: false, team: None };
        let mut o = Objectives { sites: vec![site('A'), site('B')], attackers: Some(Team::Allies), ..default() };
        o.bomb = Some(Bomb { pos: Vec3::ZERO, carrier: None, planted: None, explodes_at: None });
        assert_eq!(site_state(&o.sites[0], &o, Team::Allies), Some("target"));
        assert_eq!(site_state(&o.sites[0], &o, Team::Axis), Some("defend"));
        o.bomb = Some(Bomb { pos: Vec3::ZERO, carrier: None, planted: Some('A'), explodes_at: Some(1.0) });
        assert_eq!(site_state(&o.sites[0], &o, Team::Axis), Some("defuse"));
        assert_eq!(site_state(&o.sites[1], &o, Team::Axis), None);
        assert_eq!(compass_color("defuse"), "red");
    }
}
