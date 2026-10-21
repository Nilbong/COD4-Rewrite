//! A brush model's (`*n`) convex brushes as planes, about the model's own
//! origin: what triggers test points against (their true shape, not just
//! their box).

use crate::zone::types::ClipMap;

/// Each brush: planes (normal, distance); a point is inside when
/// `dot(normal, p) <= distance` for every one.
pub type Shape = Vec<Vec<([f32; 3], f32)>>;

/// Brush model `n`'s brushes.
pub fn model_shape(clip: &ClipMap, n: usize) -> Shape {
    let Some(m) = clip.cmodels.get(n) else { return Vec::new() };
    let mut out = Vec::new();
    let mut stack = vec![(m.leaf_brush_node, 0)];
    while let Some((i, depth)) = stack.pop() {
        let Some(node) = usize::try_from(i).ok().and_then(|i| clip.leaf_brush_nodes.get(i)) else { continue };
        if node.leaf_brush_count > 0 {
            for &b in &node.brushes {
                let Some(brush) = clip.brushes.get(b as usize) else { continue };
                let mut planes = vec![
                    ([1.0, 0.0, 0.0], brush.maxs[0]),
                    ([-1.0, 0.0, 0.0], -brush.mins[0]),
                    ([0.0, 1.0, 0.0], brush.maxs[1]),
                    ([0.0, -1.0, 0.0], -brush.mins[1]),
                    ([0.0, 0.0, 1.0], brush.maxs[2]),
                    ([0.0, 0.0, -1.0], -brush.mins[2]),
                ];
                for &p in &brush.side_planes {
                    if let Some(pl) = clip.planes.get(p as usize) {
                        planes.push((pl.normal, pl.dist));
                    }
                }
                out.push(planes);
            }
        } else if depth < 64 {
            for off in node.child_offsets.iter().filter(|&&o| o > 0) {
                stack.push((i + *off as i32, depth + 1));
            }
        }
    }
    out
}
