//! Loading characters' zones, from CoD4 or Black Ops, on loader threads,
//! and Black Ops' own player animations.

use crate::characters::{Character, Game};
use iw3::iwd::Vfs;
use iw3::xanim::XAnim;
use iw3::zone::{Asset, AssetType, ParseOptions, Zone};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

/// Where models load from: a zone of one of the games, with any zones its
/// materials come from.
#[derive(Clone, PartialEq, Debug)]
pub struct Source {
    pub game: Game,
    pub zone: &'static str,
    pub also: &'static [&'static str],
}

impl Source {
    pub fn of(c: &Character) -> Source {
        Source { game: c.game, zone: c.zone, also: c.also }
    }

    /// One zone on its own.
    pub fn zone(game: Game, zone: &'static str) -> Source {
        Source { game, zone, also: &[] }
    }
}

/// Zones, and Black Ops' archives for their textures (CoD4's are the map's).
pub type Loaded = (Vec<Zone>, Option<Arc<Vfs>>);

/// Load `src`'s zones on a thread of their own.
pub fn load(src: Source) -> JoinHandle<anyhow::Result<Loaded>> {
    std::thread::spawn(move || match src.game {
        Game::Cod4 => {
            let install = iw3::Install::locate()?;
            Ok((vec![Zone::parse(&iw3::fastfile::load(&install.zone_path(src.zone))?, ParseOptions::default())?], None))
        }
        Game::BlackOps => {
            load_black_ops_anims();
            let install = t5::Install::locate()?;
            let zones = std::iter::once(src.zone)
                .chain(src.also.iter().copied())
                .map(|z| {
                    let parsed = t5::zone::Zone::parse(&t5::fastfile::load(&install.zone_path(z))?, t5::zone::ParseOptions::default())?;
                    Ok(t5::convert::to_iw3(&parsed))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok((zones, Some(black_ops_vfs()?)))
        }
    })
}

/// Black Ops' `.iwd` archives, mounted once.
pub fn black_ops_vfs() -> anyhow::Result<Arc<Vfs>> {
    static VFS: OnceLock<Option<Arc<Vfs>>> = OnceLock::new();
    VFS.get_or_init(|| t5::Install::locate().and_then(|i| i.vfs()).map(Arc::new).ok())
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no Black Ops install"))
}

/// Black Ops' player animations (`pb_*`, from its common_mp), decoded as
/// asked for. Its rigs sit differently from CoD4's around the neck and face,
/// so CoD4's animations tip their heads down and drop their jaws.
struct AnimLibrary {
    zone: Zone,
    decoded: Mutex<HashMap<String, Option<Arc<XAnim>>>>,
}

static ANIMS: OnceLock<Option<AnimLibrary>> = OnceLock::new();

/// Load Black Ops' player animations, once; blocks for a few seconds the
/// first time, so call it from a loader thread. False without Black Ops.
pub fn load_black_ops_anims() -> bool {
    ANIMS
        .get_or_init(|| {
            let install = t5::Install::locate().ok()?;
            let data = t5::fastfile::load(&install.zone_path("common_mp")).ok()?;
            let parsed = t5::zone::Zone::parse(&data, t5::zone::ParseOptions::default()).ok()?;
            let mut zone = t5::convert::to_iw3(&parsed);
            zone.assets.retain(|a| matches!(a, Asset::Generic(g) if g.ty == AssetType::XAnimParts && g.name.starts_with("pb_")));
            Some(AnimLibrary { zone, decoded: Mutex::default() })
        })
        .is_some()
}

/// One of Black Ops' player animations, once they're loaded.
pub fn black_ops_anim(name: &str) -> Option<Arc<XAnim>> {
    let lib = ANIMS.get()?.as_ref()?;
    let mut decoded = lib.decoded.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(a) = decoded.get(name) {
        return a.clone();
    }
    let anim = lib.zone.assets.iter().find_map(|a| match a {
        Asset::Generic(g) if g.name.eq_ignore_ascii_case(name) => {
            XAnim::from_node(&g.name, &g.root, &lib.zone.script_strings).ok().map(Arc::new)
        }
        _ => None,
    });
    decoded.insert(name.to_owned(), anim.clone());
    anim
}
