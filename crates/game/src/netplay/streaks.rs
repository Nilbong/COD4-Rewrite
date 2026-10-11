//! Killstreaks a guest sees but doesn't run: the attack helicopter, the
//! care package's helicopter and crate, and sentry guns. The host's are
//! the real ones (their flight, targets, damage, capture); each snapshot
//! carries where each is ([`Mirrors::encode`]) and a guest draws a stand-in
//! there ([`mirror`]): the same model, eased between snapshots, its gun
//! heard and seen while it fires. (Airstrikes a guest flies itself from the
//! host's call, [`super::world`]; UAVs are in the snapshot's state.)
//!
//! Also here: C4 and claymores going off on the host set off the guest's
//! copies ([`host_blasts`]).

use super::wire::{GameMsg, R, W};
use super::*;
use crate::killstreaks::carepackage::{CarePackage, Courier};
use crate::killstreaks::helicopter::Helicopter;
use crate::killstreaks::models::StreakModels;
use crate::killstreaks::sentry::Sentry;

/// What a mirror is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Cobra = 1,
    Hind = 2,
    Courier = 3,
    CourierAxis = 7,
    CrateAllies = 4,
    CrateAxis = 5,
    Sentry = 6,
}

impl Kind {
    fn from(b: u8) -> Option<Kind> {
        Some(match b {
            1 => Kind::Cobra,
            2 => Kind::Hind,
            3 => Kind::Courier,
            7 => Kind::CourierAxis,
            4 => Kind::CrateAllies,
            5 => Kind::CrateAxis,
            6 => Kind::Sentry,
            _ => return None,
        })
    }

    /// The models to draw it with, first found.
    fn models(self) -> &'static [&'static str] {
        match self {
            Kind::Cobra => &["vehicle_cobra_helicopter_fly"],
            Kind::Hind => &["vehicle_mi24p_hind_desert"],
            // (CoD4's: the allies' courier is a Cobra, the axis' a Hind.)
            Kind::Courier => &["vehicle_cobra_helicopter_fly", "vehicle_mi24p_hind_desert"],
            Kind::CourierAxis => &["vehicle_mi24p_hind_desert", "vehicle_cobra_helicopter_fly"],
            Kind::CrateAllies | Kind::CrateAxis => &[crate::killstreaks::carepackage::CRATE_MODEL],
            Kind::Sentry => &crate::killstreaks::sentry::MODEL,
        }
    }

    /// Its engine's sound, played on it.
    fn sound(self) -> Option<&'static str> {
        match self {
            Kind::Cobra => Some("mp_cobra_helicopter"),
            Kind::Hind => Some("mp_hind_helicopter"),
            _ => None,
        }
    }

    /// The weapon its gun sounds and looks like.
    fn gun(self) -> Option<&'static str> {
        match self {
            Kind::Cobra | Kind::Hind => Some("cobra_20mm_mp"),
            Kind::Sentry => Some("saw_mp"),
            _ => None,
        }
    }
}

/// One mirrored thing in a snapshot.
struct Object {
    id: u16,
    kind: Kind,
    at: Transform,
    firing: bool,
}

/// Bytes per object.
const OBJECT_SIZE: usize = 2 + 1 + 12 + 6 + 1;

/// Host: what to mirror.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct Mirrors<'w, 's> {
    helis: Query<'w, 's, (Entity, &'static GlobalTransform, &'static Helicopter)>,
    couriers: Query<'w, 's, (Entity, &'static GlobalTransform, Option<&'static Children>), With<Courier>>,
    crates: Query<'w, 's, (Entity, &'static GlobalTransform, &'static CarePackage)>,
    sentries: Query<'w, 's, (Entity, &'static GlobalTransform, &'static Sentry)>,
}

impl Mirrors<'_, '_> {
    /// As many as fit in `room` bytes.
    pub fn encode(&self, room: usize, _id: &dyn Fn(Entity) -> u16) -> Vec<u8> {
        let mut list: Vec<Object> = Vec::new();
        let net_id = |e: Entity| (e.index_u32() & 0xFFFF) as u16;
        for (e, g, h) in &self.helis {
            let kind = if h.team == Team::Allies { Kind::Cobra } else { Kind::Hind };
            list.push(Object { id: net_id(e), kind, at: g.compute_transform(), firing: h.firing() });
        }
        for (e, g, children) in &self.couriers {
            // Its side: the crate it carries (Cobra until known).
            let axis = children.is_some_and(|c| c.iter().any(|c| self.crates.get(c).is_ok_and(|(_, _, p)| p.team == Team::Axis)));
            let kind = if axis { Kind::CourierAxis } else { Kind::Courier };
            list.push(Object { id: net_id(e), kind, at: g.compute_transform(), firing: false });
        }
        for (e, g, c) in &self.crates {
            let kind = if c.team == Team::Allies { Kind::CrateAllies } else { Kind::CrateAxis };
            list.push(Object { id: net_id(e), kind, at: g.compute_transform(), firing: false });
        }
        for (e, g, s) in &self.sentries {
            let mut at = g.compute_transform();
            // The gun's way, the whole sentry turned to it (the stand-in has
            // no turret to turn by itself).
            let (yaw, _) = s.aim();
            at.rotation *= Quat::from_rotation_y(yaw);
            list.push(Object { id: net_id(e), kind: Kind::Sentry, at, firing: s.firing() });
        }
        let fit = (room.saturating_sub(1) / OBJECT_SIZE).min(list.len()).min(32);
        let mut w = W::new();
        w.u8(fit as u8);
        for o in list.iter().take(fit) {
            let (y, x, z) = o.at.rotation.to_euler(EulerRot::YXZ);
            w.u16(o.id).u8(o.kind as u8).vec3(o.at.translation).angle(y).angle(x).angle(z).u8(o.firing as u8);
        }
        w.0
    }
}

fn decode(data: &[u8]) -> anyhow::Result<Vec<Object>> {
    let mut r = R(data);
    if r.is_empty() {
        return Ok(Vec::new());
    }
    let n = r.u8()? as usize;
    anyhow::ensure!(n <= 32, "too many mirrors");
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let id = r.u16()?;
        let kind = r.u8()?;
        let pos = r.vec3()?;
        let (y, x, z) = (r.angle()?, r.angle()?, r.angle()?);
        let firing = r.u8()? != 0;
        let Some(kind) = Kind::from(kind) else { anyhow::bail!("unknown mirror") };
        out.push(Object { id, kind, at: Transform { translation: pos, rotation: Quat::from_euler(EulerRot::YXZ, y, x, z), ..default() }, firing });
    }
    r.done()?;
    Ok(out)
}

/// A guest's stand-in for one of the host's.
#[derive(Component)]
pub(super) struct Mirrored {
    id: u16,
    kind: Kind,
    target: Transform,
    firing: bool,
    next_shot: f32,
}

/// Guest: the host's helicopters, crates and sentries, where the newest
/// snapshot has them.
#[allow(clippy::too_many_arguments)]
pub(super) fn mirror(
    mut commands: Commands,
    time: Res<Time>,
    guest: Res<GuestState>,
    mut applied: Local<Option<u32>>,
    mut mirrors: Query<(Entity, &mut Mirrored, &mut Transform)>,
    mut assets: ParamSet<(StreakModels, Res<crate::content::Content>)>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut shots: MessageWriter<ShotFired>,
) {
    let now = time.elapsed_secs();
    let dt = time.delta_secs();
    if let Some(snap) = guest.latest.as_ref()
        && *applied != Some(snap.tick)
    {
        *applied = Some(snap.tick);
        let objects = match super::world::split(&snap.extra).map(|(_, rest)| decode(rest)) {
            Some(Ok(o)) => o,
            Some(Err(e)) => {
                warn!("netplay: bad mirrors: {e}");
                Vec::new()
            }
            None => Vec::new(),
        };
        // Gone on the host: gone here.
        for (e, m, _) in &mirrors {
            if !objects.iter().any(|o| o.id == m.id && o.kind == m.kind) {
                commands.entity(e).despawn();
            }
        }
        for o in objects {
            if let Some((_, mut m, _)) = mirrors.iter_mut().find(|(_, m, _)| m.id == o.id && m.kind == o.kind) {
                m.target = o.at;
                m.firing = o.firing;
                continue;
            }
            let e = commands.spawn((Name::new(format!("mirrored {:?}", o.kind)), o.at, Visibility::default(), Mirrored { id: o.id, kind: o.kind, target: o.at, firing: o.firing, next_shot: 0.0 })).id();
            match o.kind {
                Kind::CrateAllies | Kind::CrateAxis => {
                    let mut models = assets.p0();
                    // (The crate hangs below the chopper's slung point on
                    // the host; here it's drawn where its entity is.)
                    if models.spawn(&mut commands, o.kind.models(), e, true).is_none() {
                        let half = Vec3::new(0.76, 0.41, 0.38);
                        models.spawn_box(&mut commands, e, Vec3::Y * 0.38, half, Color::srgb(0.33, 0.36, 0.24));
                    }
                }
                _ => {
                    assets.p0().spawn(&mut commands, o.kind.models(), e, true);
                }
            }
            // Solid as the host's are (or our soldier walks through what
            // the host's is stopped by, and is put back).
            use crate::collision::Layer;
            use avian3d::prelude::{Collider, CollisionLayers, LayerMask, RigidBody};
            let solid = match o.kind {
                Kind::Sentry => Some((Collider::cylinder(0.36, 1.17), Layer::PlayerClip, Vec3::Y * 0.58)),
                Kind::CrateAllies | Kind::CrateAxis => Some((Collider::cuboid(1.52, 0.76, 0.82), Layer::World, Vec3::Y * 0.38)),
                _ => None,
            };
            if let Some((collider, layer, at)) = solid {
                commands.spawn((RigidBody::Static, collider, CollisionLayers::new(layer, LayerMask::NONE), Transform::from_translation(at), ChildOf(e)));
            }
            if let Some(sound) = o.kind.sound() {
                sfx.play_on(sound, e);
            }
            info!("netplay: the host's {:?} ({})", o.kind, o.id);
        }
    }
    let content = assets.p1();
    // Eased towards where the host has them (a snapshot is 50 ms).
    let k = (dt * 15.0).min(1.0);
    for (e, mut m, mut tf) in &mut mirrors {
        if tf.translation.distance(m.target.translation) > 20.0 {
            *tf = m.target;
        } else {
            tf.translation = tf.translation.lerp(m.target.translation, k);
            tf.rotation = tf.rotation.slerp(m.target.rotation, k);
        }
        // Its gun, while it fires: heard and seen.
        if m.firing && now >= m.next_shot {
            m.next_shot = now + 0.1;
            let weapon = m.kind.gun().and_then(|g| crate::loadout::bot_weapon(&content, g));
            let forward = tf.rotation * Vec3::X;
            let from = tf.translation + Vec3::Y * if m.kind == Kind::Sentry { 0.9 } else { -1.0 } + forward * 1.0;
            shots.write(ShotFired { shooter: e, weapon, from, to: from + forward * 40.0, hit_pawn: false, normal: Vec3::ZERO, hit_world: false });
        }
    }
}

/// Host: C4 and claymores going off, for guests' copies to go off too.
pub(super) fn host_blasts(online: Res<Online>, mut blasts: MessageReader<crate::explosives::Exploded>) {
    for b in blasts.read() {
        if b.weapon == "c4_mp" || b.weapon == "claymore_mp" {
            online.send(EVERYONE, &LobbyMsg::Game(GameMsg::Blast { weapon: b.weapon.clone(), at: b.at }.encode()));
        }
    }
}
