//! Small, raised diamond crowns on the gun's existing coated surfaces.
//!
//! This is real geometry: each gem has eight flat crown facets and a flat
//! octagonal table, so its silhouette and highlights move with the gun.
//! Facets share the backing mesh's draw call, skeleton and render layers.
//! UV1 marks the original backing (0) and diamond vertices (1) for the camo
//! shader; neither the original alpha nor the gun's UV atlas is changed.

use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use std::collections::HashMap;

/// Bound the extra geometry even on unusually large imported surfaces.
pub(super) const MAX_STUDS: usize = 1200;
const SPACING: f32 = 0.0045;
const RADIUS: f32 = 0.0018;
const HEIGHT: f32 = 0.0012;
const BASE_OFFSET: f32 = 0.00008;
const TABLE_RADIUS: f32 = RADIUS * 0.43;
const SIDES: usize = 8;
#[cfg(test)]
const VERTICES_PER_STUD: usize = SIDES * 4 + SIDES + 1;
#[cfg(test)]
const INDICES_PER_STUD: usize = SIDES * 6 + SIDES * 3;

#[derive(Clone, Copy)]
struct Triangle {
    vertices: [usize; 3],
    normal: Vec3,
    end_area: f32,
}

/// Add deterministically spaced faceted studs to a CPU-readable native gun
/// surface. The caller caches the resulting mesh before GPU extraction.
/// Unsupported or already extracted meshes are left unchanged by returning
/// `None`; malformed indices never reach Bevy's mesh preparation.
#[cfg(test)]
pub(super) fn studded(source: &Mesh) -> Option<Mesh> {
    studded_with_budget(source, MAX_STUDS)
}

/// As [`studded`], with this surface's share of the model-wide crown budget.
/// Small pieces still get area-based spacing rather than filling the budget.
pub(super) fn studded_with_budget(source: &Mesh, budget: usize) -> Option<Mesh> {
    let budget = budget.min(MAX_STUDS);
    if budget == 0 {
        return None;
    }
    if source.primitive_topology() != PrimitiveTopology::TriangleList {
        return None;
    }
    // Native gun meshes have this small, known attribute set. Refuse other
    // layouts rather than leave an unextended vertex stream in the result.
    let allowed = [
        Mesh::ATTRIBUTE_POSITION.id,
        Mesh::ATTRIBUTE_NORMAL.id,
        Mesh::ATTRIBUTE_UV_0.id,
        Mesh::ATTRIBUTE_UV_1.id,
        Mesh::ATTRIBUTE_TANGENT.id,
        Mesh::ATTRIBUTE_COLOR.id,
        Mesh::ATTRIBUTE_JOINT_INDEX.id,
        Mesh::ATTRIBUTE_JOINT_WEIGHT.id,
    ];
    if source.try_attributes().ok()?.any(|(a, _)| !allowed.contains(&a.id)) {
        return None;
    }
    let VertexAttributeValues::Float32x3(positions) = source.try_attribute(Mesh::ATTRIBUTE_POSITION).ok()? else {
        return None;
    };
    let VertexAttributeValues::Float32x3(normals) = source.try_attribute(Mesh::ATTRIBUTE_NORMAL).ok()? else {
        return None;
    };
    let VertexAttributeValues::Float32x2(uvs) = source.try_attribute(Mesh::ATTRIBUTE_UV_0).ok()? else {
        return None;
    };
    let vertices = positions.len();
    if vertices < 3 || normals.len() != vertices || uvs.len() != vertices {
        return None;
    }
    let mut tangents = match source.try_attribute_option(Mesh::ATTRIBUTE_TANGENT).ok()? {
        Some(VertexAttributeValues::Float32x4(v)) if v.len() == vertices => Some(v.clone()),
        None => None,
        _ => return None,
    };
    let mut colors = match source.try_attribute_option(Mesh::ATTRIBUTE_COLOR).ok()? {
        Some(VertexAttributeValues::Float32x4(v)) if v.len() == vertices => Some(v.clone()),
        None => None,
        _ => return None,
    };
    let skin = match (
        source.try_attribute_option(Mesh::ATTRIBUTE_JOINT_INDEX).ok()?,
        source.try_attribute_option(Mesh::ATTRIBUTE_JOINT_WEIGHT).ok()?,
    ) {
        (Some(VertexAttributeValues::Uint16x4(j)), Some(VertexAttributeValues::Float32x4(w)))
            if j.len() == vertices && w.len() == vertices =>
        {
            Some((j, w))
        }
        (None, None) => None,
        _ => return None,
    };
    let mut indices: Vec<u32> = match source.try_indices_option().ok()? {
        Some(i) => i.iter().map(|i| i as u32).collect(),
        None => (0..u32::try_from(vertices).ok()?).collect(),
    };
    if !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i as usize >= vertices) {
        return None;
    }
    let mut triangles = Vec::new();
    let mut area = 0.0;
    for tri in indices.chunks_exact(3) {
        let ids = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let [a, b, c] = ids.map(|i| Vec3::from_array(positions[i]));
        let cross = (b - a).cross(c - a);
        let double_area = cross.length();
        if !double_area.is_finite() || double_area < 1e-10 {
            continue;
        }
        let mut normal = cross / double_area;
        let average = ids.iter().map(|&i| Vec3::from_array(normals[i])).sum::<Vec3>();
        if normal.dot(average) < 0.0 {
            normal = -normal;
        }
        area += double_area * 0.5;
        triangles.push(Triangle { vertices: ids, normal, end_area: area });
    }
    if triangles.is_empty() || !area.is_finite() {
        return None;
    }
    let wanted = ((area / (SPACING * SPACING)) as usize).clamp(1, budget);
    let mut out_positions = positions.clone();
    let mut out_normals = normals.clone();
    let mut out_uvs = uvs.clone();
    let mut tags = vec![[0.0, 0.0]; vertices];
    let mut out_skin = skin.map(|(j, w)| (j.clone(), w.clone()));
    let mut occupied: HashMap<[i32; 3], Vec<Vec3>> = HashMap::new();
    let mut added = 0;
    // A bounded low-discrepancy sequence distributes candidates by actual
    // triangle area rather than by vertex count or the source UV islands.
    for candidate in 1..=wanted * 32 {
        let at = radical_inverse(candidate, 2) * area;
        let tri = &triangles[triangles.partition_point(|t| t.end_area <= at).min(triangles.len() - 1)];
        let root = radical_inverse(candidate, 3).sqrt();
        let split = radical_inverse(candidate, 5);
        let bary = [1.0 - root, root * (1.0 - split), root * split];
        let center = blend3(positions, tri.vertices, bary);
        let cell = cell_of(center);
        if !center.is_finite() || too_close(&occupied, cell, center) {
            continue;
        }
        occupied.entry(cell).or_default().push(center);
        let smooth = blend3(normals, tri.vertices, bary).normalize_or(tri.normal);
        // Do not let an unusual smoothed vertex normal flip a gem inward.
        let normal = if smooth.dot(tri.normal) > 0.35 { smooth } else { tri.normal };
        let tangent = normal.any_orthonormal_vector();
        let bitangent = normal.cross(tangent);
        let uv =
            tri.vertices.iter().zip(bary).fold(Vec2::ZERO, |v, (&i, w)| v + Vec2::from_array(uvs[i]) * w).to_array();
        let joints = skin.map(|(j, w)| blend_skin(j, w, tri.vertices, bary));
        let mut append = |point: Vec3, n: Vec3, t: Vec3| {
            let id = out_positions.len() as u32;
            out_positions.push(point.to_array());
            out_normals.push(n.to_array());
            out_uvs.push(uv);
            tags.push([1.0, 1.0]);
            if let Some(values) = &mut tangents {
                values.push([t.x, t.y, t.z, 1.0]);
            }
            if let Some(values) = &mut colors {
                values.push([1.0; 4]);
            }
            if let (Some((j, w)), Some((js, ws))) = (&mut out_skin, joints) {
                j.push(js);
                w.push(ws);
            }
            id
        };
        let radial = |i: usize| {
            let a = i as f32 * std::f32::consts::TAU / SIDES as f32;
            tangent * a.cos() + bitangent * a.sin()
        };
        for side in 0..SIDES {
            let r0 = radial(side);
            let r1 = radial(side + 1);
            let bottom0 = center + normal * BASE_OFFSET + r0 * RADIUS;
            let bottom1 = center + normal * BASE_OFFSET + r1 * RADIUS;
            let top0 = center + normal * HEIGHT + r0 * TABLE_RADIUS;
            let top1 = center + normal * HEIGHT + r1 * TABLE_RADIUS;
            let edge = (bottom1 - bottom0).normalize();
            let facet = (bottom1 - bottom0).cross(top1 - bottom0).normalize();
            let start = append(bottom0, facet, edge);
            append(bottom1, facet, edge);
            append(top1, facet, edge);
            append(top0, facet, edge);
            indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
        }
        let table = append(center + normal * HEIGHT, normal, tangent);
        for side in 0..SIDES {
            append(center + normal * HEIGHT + radial(side) * TABLE_RADIUS, normal, tangent);
        }
        for side in 0..SIDES {
            indices.extend_from_slice(&[table, table + 1 + side as u32, table + 1 + ((side + 1) % SIDES) as u32]);
        }
        added += 1;
        if added == wanted {
            break;
        }
    }
    if added == 0 {
        return None;
    }
    let mut mesh = source.clone();
    // Old bounds describe just the backing; Bevy recomputes them with the
    // raised crown points during extraction.
    mesh.final_aabb = None;
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, out_positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, out_normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, out_uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, tags);
    if let Some(t) = tangents {
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, t);
    }
    if let Some(c) = colors {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, c);
    }
    if let Some((j, w)) = out_skin {
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(j));
        mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, w);
    }
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

fn blend3(values: &[[f32; 3]], ids: [usize; 3], bary: [f32; 3]) -> Vec3 {
    ids.into_iter().zip(bary).fold(Vec3::ZERO, |v, (i, w)| v + Vec3::from_array(values[i]) * w)
}

/// The crown remains rigid: every facet vertex takes the same interpolated
/// center weights. Combine repeated joints before keeping the largest four.
fn blend_skin(joints: &[[u16; 4]], weights: &[[f32; 4]], ids: [usize; 3], bary: [f32; 3]) -> ([u16; 4], [f32; 4]) {
    let mut merged = Vec::<(u16, f32)>::with_capacity(12);
    for (i, fraction) in ids.into_iter().zip(bary) {
        for (joint, weight) in joints[i].into_iter().zip(weights[i]) {
            let weight = weight * fraction;
            if !weight.is_finite() || weight <= 0.0 {
                continue;
            }
            if let Some((_, old)) = merged.iter_mut().find(|(j, _)| *j == joint) {
                *old += weight;
            } else {
                merged.push((joint, weight));
            }
        }
    }
    merged.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let sum: f32 = merged.iter().take(4).map(|x| x.1).sum();
    if sum <= 0.0 {
        return ([joints[ids[0]][0], 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);
    }
    let (mut j, mut w) = ([0; 4], [0.0; 4]);
    for (i, (joint, weight)) in merged.into_iter().take(4).enumerate() {
        j[i] = joint;
        w[i] = weight / sum;
    }
    (j, w)
}

fn radical_inverse(mut n: usize, base: usize) -> f32 {
    let mut result = 0.0;
    let mut denominator = 1.0;
    while n != 0 {
        denominator *= base as f32;
        result += (n % base) as f32 / denominator;
        n /= base;
    }
    result
}

fn cell_of(p: Vec3) -> [i32; 3] {
    (p / SPACING).floor().as_ivec3().to_array()
}

fn too_close(cells: &HashMap<[i32; 3], Vec<Vec3>>, cell: [i32; 3], point: Vec3) -> bool {
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if cells
                    .get(&[cell[0] + dx, cell[1] + dy, cell[2] + dz])
                    .is_some_and(|points| points.iter().any(|p| p.distance_squared(point) < SPACING * SPACING))
                {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::RenderAssetUsages;

    fn plane(size: f32) -> Mesh {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0, 0.0, 0.0], [size, 0.0, 0.0], [0.0, size, 0.0]]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 3]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
        mesh.insert_indices(Indices::U32(vec![0, 1, 2]));
        mesh
    }

    #[test]
    fn raised_facets_keep_backing_and_obey_geometry_budget() {
        let original = plane(1.0);
        let result = studded(&original).unwrap();
        let VertexAttributeValues::Float32x3(p) = result.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else { panic!() };
        let VertexAttributeValues::Float32x3(n) = result.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap() else { panic!() };
        let VertexAttributeValues::Float32x2(tags) = result.attribute(Mesh::ATTRIBUTE_UV_1).unwrap() else { panic!() };
        assert_eq!(p[..3], [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(tags[..3], [[0.0; 2]; 3]);
        assert!(tags[3..].iter().all(|uv| *uv == [1.0; 2]));
        let studs = (p.len() - 3) / VERTICES_PER_STUD;
        assert_eq!(studs, MAX_STUDS);
        assert_eq!(result.indices().unwrap().len(), 3 + studs * INDICES_PER_STUD);
        assert!(p[3..].iter().all(|point| point[2] >= BASE_OFFSET - 1e-7 && point[2] <= HEIGHT + 1e-7));
        assert!(p[3..].iter().any(|point| (point[2] - HEIGHT).abs() < 1e-7));
        assert!(n[3..].iter().any(|n| n[2] < 0.9), "crown facets need angled normals");
        assert!(n.iter().all(|n| (Vec3::from_array(*n).length() - 1.0).abs() < 1e-4));
        assert!(result.indices().unwrap().iter().all(|i| i < p.len()));
        for (_, values) in result.attributes() {
            assert_eq!(values.len(), p.len(), "every vertex stream must include the facets");
        }
        // The bounds include a true raised surface, not just painted shading.
        assert!((p.iter().map(|p| p[2]).fold(0.0f32, f32::max) - HEIGHT).abs() < 1e-7);
    }

    #[test]
    fn placement_is_deterministic_and_studs_stay_spaced() {
        let source = plane(0.08);
        let a = studded(&source).unwrap();
        let b = studded(&source).unwrap();
        assert_eq!(a.attribute(Mesh::ATTRIBUTE_POSITION), b.attribute(Mesh::ATTRIBUTE_POSITION));
        assert_eq!(a.indices(), b.indices());
        let VertexAttributeValues::Float32x3(p) = a.attribute(Mesh::ATTRIBUTE_POSITION).unwrap() else { panic!() };
        let centers: Vec<Vec3> =
            p[3..].chunks_exact(VERTICES_PER_STUD).map(|v| Vec3::from_array(v[SIDES * 4])).collect();
        for (i, a) in centers.iter().enumerate() {
            for b in centers.iter().skip(i + 1) {
                assert!(a.distance(*b) >= SPACING - 1e-6);
            }
        }
    }

    #[test]
    fn crowns_follow_the_original_skin_with_normalized_weights() {
        let mut source = plane(0.02);
        source.insert_attribute(Mesh::ATTRIBUTE_TANGENT, vec![[1.0, 0.0, 0.0, 1.0]; 3]);
        source.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.7, 0.8, 0.9, 1.0]; 3]);
        source.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(vec![[1, 2, 0, 0]; 3]));
        source.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, vec![[0.25, 0.75, 0.0, 0.0]; 3]);
        let result = studded(&source).unwrap();
        let VertexAttributeValues::Uint16x4(joints) = result.attribute(Mesh::ATTRIBUTE_JOINT_INDEX).unwrap() else {
            panic!()
        };
        let VertexAttributeValues::Float32x4(weights) = result.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT).unwrap() else {
            panic!()
        };
        assert_eq!(joints[..3], [[1, 2, 0, 0]; 3]);
        assert_eq!(weights[..3], [[0.25, 0.75, 0.0, 0.0]; 3]);
        for (j, w) in joints[3..].iter().zip(&weights[3..]) {
            assert_eq!(*j, [2, 1, 0, 0]);
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
            assert!((w[0] - 0.75).abs() < 1e-6 && (w[1] - 0.25).abs() < 1e-6);
        }
        let VertexAttributeValues::Float32x4(colors) = result.attribute(Mesh::ATTRIBUTE_COLOR).unwrap() else {
            panic!()
        };
        assert_eq!(colors[..3], [[0.7, 0.8, 0.9, 1.0]; 3]);
        assert!(colors[3..].iter().all(|c| *c == [1.0; 4]));
    }

    #[test]
    fn skin_combines_duplicates_and_keeps_the_four_largest_influences() {
        let j = [[1, 2, 0, 0], [2, 3, 4, 0], [5, 6, 0, 0]];
        let w = [[0.6, 0.4, 0.0, 0.0], [0.5, 0.3, 0.2, 0.0], [0.8, 0.2, 0.0, 0.0]];
        let (j, w) = blend_skin(&j, &w, [0, 1, 2], [0.2, 0.3, 0.5]);
        assert_eq!(j, [5, 2, 1, 6]);
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!(w.windows(2).all(|w| w[0] >= w[1]));
    }

    #[test]
    fn malformed_and_degenerate_surfaces_are_not_extended() {
        assert!(studded(&plane(0.0)).is_none());
        let mut bad = plane(0.1);
        bad.insert_indices(Indices::U32(vec![0, 1, 99]));
        assert!(studded(&bad).is_none());
        let mut bad = plane(0.1);
        bad.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, vec![[1.0, 0.0, 0.0, 0.0]; 3]);
        assert!(studded(&bad).is_none(), "half a skin stream cannot animate correctly");
    }

    #[test]
    fn per_surface_budget_bounds_the_appended_crowns() {
        let source = plane(1.0);
        assert!(studded_with_budget(&source, 0).is_none());
        for budget in [1, 5, 37] {
            let result = studded_with_budget(&source, budget).unwrap();
            assert_eq!(result.count_vertices(), source.count_vertices() + budget * VERTICES_PER_STUD);
            assert_eq!(result.indices().unwrap().len(), 3 + budget * INDICES_PER_STUD);
        }
    }
}
