//! Conversions between CoD world space and Bevy world space.
//!
//! CoD: right-handed, Z up, X forward, Y left, units are inches.
//! Bevy: right-handed, Y up, -Z forward, units are metres.
//! The mapping `(x, y, z) -> (x, z, -y)` is a pure rotation, so triangle
//! winding and handedness are preserved.

use bevy::math::{Mat3, Quat, Vec3};

/// One CoD unit (an inch) in metres.
pub const INCH: f32 = 0.0254;

/// Convert a length in CoD units to metres.
pub const fn u(v: f32) -> f32 {
    v * INCH
}

/// Convert a CoD-space point to a Bevy-space point.
pub fn pos(p: [f32; 3]) -> Vec3 {
    dir(p) * INCH
}

/// Convert a Bevy-space point back to CoD space.
pub fn to_cod(p: Vec3) -> [f32; 3] {
    [p.x / INCH, -p.z / INCH, p.y / INCH]
}

/// Convert a CoD-space direction (no scaling).
pub fn dir(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0], p[2], -p[1])
}

/// Basis change matrix: `bevy = B * cod`.
pub fn basis() -> Mat3 {
    Mat3::from_cols(Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, 1.0, 0.0))
}

/// Rotation for a CoD axis triple (forward, left, up), expressed in Bevy space.
pub fn axis_rotation(axis: [[f32; 3]; 3]) -> Quat {
    let m = Mat3::from_cols(axis[0].into(), axis[1].into(), axis[2].into());
    let b = basis();
    Quat::from_mat3(&(b * m * b.transpose())).normalize()
}

/// Yaw (radians, Bevy convention: rotation about +Y) from CoD yaw in degrees.
///
/// CoD yaw 0 faces +X; Bevy's identity faces -Z, so +X is a yaw of -90°.
pub fn yaw_from_cod_degrees(yaw: f32) -> f32 {
    (yaw - 90.0).to_radians()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_is_up() {
        assert_eq!(dir([0.0, 0.0, 1.0]), Vec3::Y);
        assert_eq!(dir([1.0, 0.0, 0.0]), Vec3::X);
        assert_eq!(dir([0.0, 1.0, 0.0]), -Vec3::Z);
    }

    #[test]
    fn yaw_zero_faces_cod_x() {
        let q = Quat::from_rotation_y(yaw_from_cod_degrees(0.0));
        assert!((q * Vec3::NEG_Z - Vec3::X).length() < 1e-5);
        let q = Quat::from_rotation_y(yaw_from_cod_degrees(90.0));
        assert!((q * Vec3::NEG_Z - dir([0.0, 1.0, 0.0])).length() < 1e-5);
    }

    #[test]
    fn identity_axis() {
        let q = axis_rotation([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        assert!(q.angle_between(Quat::IDENTITY) < 1e-5);
    }
}
