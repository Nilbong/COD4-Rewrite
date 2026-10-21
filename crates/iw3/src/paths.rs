//! A single-player level's path nodes as the map compiler left them in its
//! GameWorldSp asset (`PathData`): where each stands, its type, its angle
//! and its links to others.

use crate::zone::{Asset, AssetType, Zone};

#[derive(Clone, Debug)]
pub struct Node {
    pub origin: [f32; 3],
    /// CoD4's node type (1 plain path node, 2.. cover and the rest).
    pub kind: i32,
    pub angle: f32,
    /// Linked nodes and how far.
    pub links: Vec<(usize, f32)>,
}

/// The zone's path nodes (none if it has no GameWorldSp).
pub fn from_zone(zone: &Zone) -> Vec<Node> {
    let Some(world) = zone.assets.iter().find_map(|a| match a {
        Asset::Generic(g) if g.ty == AssetType::GameWorldSp => Some(&g.root),
        _ => None,
    }) else {
        return Vec::new();
    };
    let Some(path) = world.node("path") else { return Vec::new() };
    path.nodes("nodes")
        .iter()
        .filter_map(|n| {
            let c = n.node("constant")?;
            Some(Node {
                origin: [c.float("vOrigin[0]"), c.float("vOrigin[1]"), c.float("vOrigin[2]")],
                kind: c.int("type") as i32,
                angle: c.float("fAngle"),
                links: c.nodes("Links").iter().map(|l| (l.int("nodeNum") as usize, l.float("fDist"))).collect(),
            })
        })
        .collect()
}
