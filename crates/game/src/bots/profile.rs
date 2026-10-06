//! Player profiles: a real player's habits measured from gameplay
//! recordings (`--record`, then `tools/fit_player.py`), so bots can play
//! like them. `--bot-profile <file>` loads one; `--skill 0.5` then means
//! "as good as the recorded player", higher or lower scales from there.
//!
//! The file is plain `key = value` lines (`#` starts a comment); missing
//! keys keep the bots' defaults. See [`PlayerProfile`] for the keys.

use super::Personality;
use crate::units::u;
use bevy::prelude::*;
use rand::Rng;

/// `--bot-profile <file>`, set before the plugins build.
#[derive(Resource, Default)]
pub struct ProfileArg(pub Option<String>);

// Measurement offsets: what `tools/fit_player.py` reads from bots with known
// settings, so a profile measured from a player converts to bot settings
// that measure the same. Found by recording every bot of 4-minute matches
// (22 bots, ~700 fights each) with default and with profile settings.
/// Measured reaction includes the time to notice the enemy (bots' takes
/// about this long; their reaction setting is what comes after).
pub const REACTION_OFFSET: f32 = 0.1;
/// The most a bot ever sprints, as a share of moving time (CoD4's sprint
/// runs out after 4 s; bots stop for corners and contacts).
pub const SPRINT_MAX: f32 = 0.35;
/// Measured first flicks run long by about `a - b * ID` (segmentation).
pub const FITTS_OFFSET: (f32, f32) = (0.081, 0.013);
/// Measured landing spread is wider than the hand model's own.
pub const GAIN_SD_SCALE: f32 = 0.4;
/// Pauses measure long (the fitter folds the shortest into key switches),
/// flips a little rare.
pub const PAUSE_SCALE: f32 = 0.85;
pub const FLIP_SCALE: f32 = 1.2;
/// Crouching and jumping only happen in straight fights, not on the way to
/// cover, so they measure lower than set.
pub const CROUCH_SCALE: f32 = 1.25;
pub const JUMP_SCALE: f32 = 1.5;
/// Stance changes per second in close fights that aren't crouch spam
/// (crouched engagements starting and ending, going prone).
pub const CROUCH_TAP_BASELINE: f32 = 0.6;
/// Headshot share of hits is about `0.05 + 0.36 * head_bias`.
pub const HEAD_RATE: (f32, f32) = (0.05, 0.36);

/// Distance bands for fight movement: close, mid, far (CoD units).
pub const BANDS: [f32; 2] = [600.0, 1600.0];

pub fn band(dist_units: f32) -> usize {
    BANDS.iter().filter(|&&b| dist_units >= b).count()
}

/// A recorded player's measured habits. Times in seconds, distances in CoD
/// units.
#[derive(Resource, Clone, Debug, Default)]
pub struct PlayerProfile {
    // Aim.
    /// Seconds from an enemy appearing to the first aiming move.
    pub reaction: Option<f32>,
    /// Fitts' law for the first flick: `a + b * log2(A/W + 1)` seconds.
    pub fitts_a: Option<f32>,
    pub fitts_b: Option<f32>,
    /// Where the first flick lands, as a fraction of the way to the target.
    pub gain_bias: Option<f32>,
    pub gain_sd: Option<f32>,
    // Movement while fighting, per distance band (`close_`, `mid_`, `far_`).
    /// Average length of one strafe key press.
    pub strafe_press: [Option<f32>; 3],
    /// Average pause between strafes.
    pub strafe_pause: [Option<f32>; 3],
    /// How often a strafe is followed straight by one the other way rather
    /// than a pause.
    pub strafe_flip: [Option<f32>; 3],
    /// Hip-fire inside this distance, aim down sights beyond it.
    pub ads_range: Option<f32>,
    /// Fraction of fights fought crouched.
    pub fight_crouch: Option<f32>,
    /// Fraction of close fights opened by dropping prone.
    pub dropshot: Option<f32>,
    /// Stance changes per second in close fights (crouch spamming).
    pub crouch_taps: Option<f32>,
    /// Jumps per minute of fighting.
    pub fight_jumps: Option<f32>,
    /// Average forward input while fighting, -1 (backing off) to 1 (pushing).
    pub push: Option<f32>,
    // Playstyle.
    /// Typical distance of the first shot at an enemy.
    pub preferred_range: Option<f32>,
    /// Fraction of time out of fights spent standing still.
    pub stillness: Option<f32>,
    /// Fraction of moving time out of fights spent sprinting.
    pub sprint: Option<f32>,
    /// Fraction of hits that are headshots.
    pub head_rate: Option<f32>,
}

impl PlayerProfile {
    pub fn parse(text: &str) -> PlayerProfile {
        let mut p = PlayerProfile::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once('=') else { continue };
            let Ok(v) = value.trim().parse::<f32>() else { continue };
            let v = Some(v).filter(|v| v.is_finite());
            let key = key.trim();
            let banded = |prefix: &str| -> Option<usize> {
                ["close_", "mid_", "far_"].iter().position(|b| key.strip_prefix(b) == Some(prefix))
            };
            match key {
                "reaction" => p.reaction = v,
                "fitts_a" => p.fitts_a = v,
                "fitts_b" => p.fitts_b = v,
                "gain_bias" => p.gain_bias = v,
                "gain_sd" => p.gain_sd = v,
                "ads_range" => p.ads_range = v,
                "fight_crouch" => p.fight_crouch = v,
                "dropshot" => p.dropshot = v,
                "crouch_taps" => p.crouch_taps = v,
                "fight_jumps" => p.fight_jumps = v,
                "push" => p.push = v,
                "preferred_range" => p.preferred_range = v,
                "stillness" => p.stillness = v,
                "sprint" => p.sprint = v,
                "head_rate" => p.head_rate = v,
                _ => {
                    if let Some(b) = banded("strafe_press") {
                        p.strafe_press[b] = v;
                    } else if let Some(b) = banded("strafe_pause") {
                        p.strafe_pause[b] = v;
                    } else if let Some(b) = banded("strafe_flip") {
                        p.strafe_flip[b] = v;
                    } else {
                        warn!("bot profile: unknown key {key}");
                    }
                }
            }
        }
        p
    }

    /// Personality for a bot playing like this player, varied a little.
    pub(super) fn personality(&self, base: Personality, skill: f32, rng: &mut impl Rng) -> Personality {
        let range_scale = rng.random_range(0.8..1.25);
        let mut vary = |v: f32, spread: f32| (v + rng.random_range(-spread..spread)).clamp(0.05, 0.95);
        Personality {
            aggression: self.push.map_or(base.aggression, |push| vary(0.55 + push * 0.9, 0.12)),
            patience: self.stillness.map_or(base.patience, |s| vary((s - 0.1) / 0.5, 0.12)),
            preferred_range: self
                .preferred_range
                .map_or(base.preferred_range, |r| u(r.clamp(150.0, 3000.0)) * range_scale),
            head_bias: self
                .head_rate
                .map_or(base.head_bias, |h| ((h - HEAD_RATE.0) / HEAD_RATE.1 * (0.75 + 0.5 * skill)).clamp(0.0, 0.9)),
            sprinter: self.sprint.map_or(base.sprinter, |s| vary(s / SPRINT_MAX, 0.08)),
        }
    }
}

/// How a bot moves and stands in a fight.
#[derive(Clone, Debug)]
pub struct MoveStyle {
    /// Per distance band: mean strafe press, mean pause, chance a strafe
    /// is followed straight by one the other way.
    pub press: [f32; 3],
    pub pause: [f32; 3],
    pub flip: [f32; 3],
    /// The distance (metres) at which a standing bot aims down sights half
    /// the time in a fight: less closer, more further (see
    /// `bots::ads_chance`).
    pub ads_range: f32,
    /// Chance of fighting crouched, beyond `CROUCH_FROM` only by default.
    pub crouch: f32,
    pub crouch_any_range: bool,
    /// Chance of opening a close fight by going prone; `None` uses the
    /// default formula (aggression and skill).
    pub dropshot: Option<f32>,
    /// Stance changes per second in close fights; `None`: the default
    /// (aggressive bots only).
    pub crouch_taps: Option<f32>,
    /// Jumps per minute of fighting.
    pub jumps: f32,
    /// Average forward input while fighting; `None`: from aggression and
    /// preferred range.
    pub push: Option<f32>,
}

impl Default for MoveStyle {
    fn default() -> Self {
        MoveStyle {
            press: [0.22, 0.26, 0.33],
            // Real players in the demos move on 54% of their shots: shorter
            // stops at mid and long range than first fitted.
            pause: [0.3, 0.35, 1.2],
            // Straight back the other way: half as often as first fitted
            // (bot lab, 2026-10-06: real players in the demos reverse a
            // strafe about half as often as bots did, the "bot" A-D-A-D
            // look; flips fell on all six suite maps).
            flip: [0.45, 0.36, 0.18],
            ads_range: u(170.0),
            crouch: 0.35,
            crouch_any_range: false,
            dropshot: None,
            crouch_taps: None,
            jumps: 0.0,
            push: None,
        }
    }
}

impl MoveStyle {
    pub fn from_profile(p: &PlayerProfile) -> MoveStyle {
        let mut s = MoveStyle::default();
        for b in 0..3 {
            s.press[b] = p.strafe_press[b].map_or(s.press[b], |v| v.clamp(0.06, 2.0));
            s.pause[b] = p.strafe_pause[b].map_or(s.pause[b], |v| (v * PAUSE_SCALE).clamp(0.06, 6.0));
            s.flip[b] = p.strafe_flip[b].map_or(s.flip[b], |v| (v * FLIP_SCALE).clamp(0.0, 1.0));
        }
        s.ads_range = p.ads_range.map_or(s.ads_range, |r| u(r.clamp(0.0, 3000.0)));
        if let Some(c) = p.fight_crouch {
            s.crouch = (c * CROUCH_SCALE).clamp(0.0, 1.0);
            s.crouch_any_range = true;
        }
        s.dropshot = p.dropshot.map(|d| d.clamp(0.0, 1.0));
        s.crouch_taps = p.crouch_taps.map(|r| (r - CROUCH_TAP_BASELINE).clamp(0.0, 6.0));
        s.jumps = p.fight_jumps.map_or(0.0, |j| (j * JUMP_SCALE).clamp(0.0, 60.0));
        s.push = p.push.map(|v| v.clamp(-1.0, 1.0));
        s
    }
}

/// Load `--bot-profile`, if given.
pub fn setup(app: &mut App) {
    let Some(path) = app.world().get_resource::<ProfileArg>().and_then(|a| a.0.clone()) else { return };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let profile = PlayerProfile::parse(&text);
            info!("bots play like the profile in {path}: {profile:?}");
            app.insert_resource(profile);
        }
        Err(e) => warn!("can't read bot profile {path}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys_bands_and_comments() {
        let p = PlayerProfile::parse(
            "# fitted from 2 recordings\nreaction = 0.31\nmid_strafe_press = 0.4 # seconds\nfar_strafe_flip=0.2\nbogus\n",
        );
        assert_eq!(p.reaction, Some(0.31));
        assert_eq!(p.strafe_press, [None, Some(0.4), None]);
        assert_eq!(p.strafe_flip[2], Some(0.2));
        assert_eq!(p.fitts_a, None);
    }

    #[test]
    fn bands_split_at_600_and_1600() {
        assert_eq!(band(100.0), 0);
        assert_eq!(band(600.0), 1);
        assert_eq!(band(2000.0), 2);
    }

    #[test]
    fn sights_up_more_at_range_and_less_on_the_move() {
        let ads = super::super::ads_chance;
        let half = MoveStyle::default().ads_range;
        assert!((ads(half, false, half) - 0.5).abs() < 1e-4);
        assert!(ads(half * 4.0, false, half) > 0.85 && ads(half / 4.0, false, half) < 0.15);
        assert!(ads(half * 2.0, true, half) < ads(half * 2.0, false, half) * 0.6);
        // A profile that always aims down sights.
        assert_eq!(ads(u(50.0), true, 0.0), 1.0);
    }
}
