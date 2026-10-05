//! Bots' classes, set up the way CoD4 players set theirs: the gun they like
//! with the attachment most players put on that kind, a pistol, three perks
//! (Stopping Power for most; Martyrdom and Last Stand mostly for weaker
//! players, as in public matches), a special grenade and, from perk 1, a
//! claymore, C4 or an RPG; camouflage more often the higher their rank.
//! The bot's pawn gets it as a [`PawnClass`], so it spawns with it as the
//! player does with theirs.

use super::{Bot, GunKind, Personality};
use crate::combat::Dead;
use crate::loadout::{ClassLoadout, Gun, PawnClass};
use bevy::prelude::*;
use rand::Rng;

/// One of `options` by weight.
fn pick<T: Copy>(rng: &mut impl Rng, options: &[(T, f32)]) -> T {
    let total: f32 = options.iter().map(|o| o.1.max(0.0)).sum();
    let mut r = rng.random::<f32>() * total;
    for &(v, w) in options {
        r -= w.max(0.0);
        if r <= 0.0 {
            return v;
        }
    }
    options[options.len() - 1].0
}

/// CoD4's guns with a gold camouflage.
const GOLD: [&str; 5] = ["ak47", "uzi", "m60e4", "dragunov", "m1014"];
/// CoD4's camouflage index for gold (1-5 are the others).
const GOLD_CAMO: usize = 6;

/// A class for a bot with this gun and taste.
pub(super) fn class_for(gun: &str, kind: GunKind, p: &Personality, skill: f32, rank: (i32, i32), rng: &mut impl Rng) -> ClassLoadout {
    let attachment = match kind {
        GunKind::Rifle => pick(rng, &[("reflex", 0.4), ("", 0.2), ("silencer", 0.15), ("acog", 0.15), ("gl", 0.1)]),
        GunKind::Smg => pick(rng, &[("reflex", 0.35), ("silencer", 0.35), ("", 0.2), ("acog", 0.1)]),
        GunKind::Lmg => pick(rng, &[("grip", 0.45), ("reflex", 0.3), ("", 0.15), ("acog", 0.1)]),
        GunKind::Shotgun => pick(rng, &[("", 0.5), ("grip", 0.3), ("reflex", 0.2)]),
        GunKind::Sniper => pick(rng, &[("", 0.7), ("acog", 0.3)]),
    };
    // Perk 1: equipment, or more grenades or ammo (the grenade launcher
    // takes its place).
    let perk1 = (attachment != "gl").then(|| {
        pick(
            rng,
            &[
                ("specialty_weapon_claymore", 0.08 + 0.2 * p.patience),
                ("specialty_weapon_c4", 0.06 + 0.12 * p.aggression),
                ("specialty_weapon_rpg", 0.12),
                ("specialty_fraggrenade", 0.2),
                ("specialty_extraammo", 0.18),
                ("specialty_specialgrenade", 0.08),
                ("specialty_detectexplosive", 0.04),
            ],
        )
    });
    let perk2 = pick(
        rng,
        &[
            ("specialty_bulletdamage", 0.6),
            ("specialty_armorvest", 0.14),
            ("specialty_gpsjammer", 0.12),
            ("specialty_rof", 0.07),
            ("specialty_fastreload", 0.07),
        ],
    );
    let perk3 = match kind {
        GunKind::Sniper => pick(rng, &[("specialty_holdbreath", 0.6), ("specialty_bulletpenetration", 0.3), ("specialty_quieter", 0.1)]),
        _ => pick(
            rng,
            &[
                ("specialty_bulletpenetration", 0.25),
                ("specialty_longersprint", 0.12 + 0.2 * p.aggression),
                ("specialty_bulletaccuracy", 0.12),
                ("specialty_quieter", 0.1),
                ("specialty_grenadepulldeath", 0.14 * (1.2 - skill)),
                ("specialty_pistoldeath", 0.12 * (1.2 - skill)),
                ("specialty_parabolic", 0.05),
            ],
        ),
    };
    let inventory = match perk1 {
        Some("specialty_weapon_claymore") => Some("claymore_mp"),
        Some("specialty_weapon_c4") => Some("c4_mp"),
        Some("specialty_weapon_rpg") => Some("rpg_mp"),
        _ => None,
    };
    let perks = perk1.filter(|_| inventory.is_none()).into_iter().chain([perk2, perk3]).map(str::to_owned).collect();
    let pistol = pick(rng, &[("beretta", 0.35), ("usp", 0.25), ("colt45", 0.2), ("deserteagle", 0.2)]);
    let special = pick(rng, &[("flash_grenade", 0.5), ("concussion_grenade", 0.35), ("smoke_grenade", 0.15)]);
    // Camouflage: more of it up the ranks, gold for the odd top one.
    let (level, prestige) = rank;
    let camo = if (prestige > 0 || level >= 50) && GOLD.contains(&gun) && rng.random::<f32>() < 0.15 {
        GOLD_CAMO
    } else if rng.random::<f32>() < 0.15 + 0.6 * (level as f32 / 54.0).max(if prestige > 0 { 1.0 } else { 0.0 }) {
        rng.random_range(1..=5)
    } else {
        0
    };
    let spec = if attachment.is_empty() { gun.to_owned() } else { format!("{gun}:{attachment}") };
    let gun = |spec: String, camo| Gun { name: spec.split(':').next().unwrap_or_default().to_ascii_uppercase(), spec, camo, variant: None };
    ClassLoadout {
        name: "Bot".into(),
        guns: vec![gun(spec, camo), gun(pistol.to_owned(), 0)],
        perks,
        special: Some(special.into()),
        inventory: inventory.map(str::to_owned),
    }
}

/// Give each bot's pawn its class, once (spawning then arms it). A gun
/// whose attachment the game doesn't have goes without it.
pub(super) fn class_bots(
    mut commands: Commands,
    content: Option<Res<crate::content::Content>>,
    bots: Query<(Entity, &Bot), (Without<PawnClass>, Without<Dead>)>,
) {
    let Some(content) = content else { return };
    for (e, bot) in &bots {
        let mut class = bot.class.clone();
        for g in &mut class.guns {
            if crate::loadout::bot_weapon(&content, &g.spec).is_none() {
                g.spec = g.spec.split(':').next().unwrap_or_default().to_owned();
            }
        }
        class.guns.retain(|g| crate::loadout::bot_weapon(&content, &g.spec).is_some());
        if class.guns.is_empty() {
            continue;
        }
        commands.entity(e).insert(PawnClass(class));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_are_complete() {
        let mut rng = rand::rng();
        let p = Personality { aggression: 0.5, patience: 0.5, preferred_range: 1.0, head_bias: 0.3, sprinter: 0.7 };
        for kind in [GunKind::Rifle, GunKind::Smg, GunKind::Lmg, GunKind::Shotgun, GunKind::Sniper] {
            for _ in 0..50 {
                let c = class_for("m4", kind, &p, 0.5, (30, 0), &mut rng);
                assert_eq!(c.guns.len(), 2);
                assert!(c.special.is_some());
                // Two or three perks: perk 1 is equipment or the launcher.
                let gl = c.guns[0].spec.ends_with(":gl");
                assert_eq!(c.perks.len(), if gl || c.inventory.is_some() { 2 } else { 3 }, "{c:?}");
                assert!(!(gl && c.inventory.is_some()));
            }
        }
    }
}
