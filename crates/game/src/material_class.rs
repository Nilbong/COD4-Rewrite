//! The material-class layer on world surfaces and props
//! (`shaders/material_class.wgsl`): anti-tiling, detail normals and
//! roughness by CoD4 surface type (`surfaceTypeBits`), and specular
//! anti-aliasing. `COD4RW_MATCLASS=0` leaves it off (A/B). Knobs, read at
//! map load (`crate::tune`): `matclass.detail` (detail bumps' strength),
//! `matclass.antitile` (colour variation, ±fraction).

use bevy::prelude::*;

pub struct MaterialClassPlugin;

impl Plugin for MaterialClassPlugin {
    fn build(&self, app: &mut App) {
        bevy::shader::load_shader_library!(app, "shaders/material_class.wgsl");
    }
}

pub fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("COD4RW_MATCLASS").is_ok_and(|v| v == "0"))
}

/// `WorldParams::surface` for a CoD4 material's `surfaceTypeBits`: IW3
/// stores type N (`crate::collision::SURFACE_NAMES`) as `1 << (N - 1)`,
/// none ("default") as 0.
pub fn surface(bits: u32) -> Vec4 {
    let class = if bits == 0 { 0 } else { (bits.trailing_zeros() + 1).min(28) };
    Vec4::new(
        class as f32,
        on() as u32 as f32,
        crate::tune::get("matclass.detail", 1.0),
        crate::tune::get("matclass.antitile", 0.08),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn surface_bits() {
        // IW3: `1 << (type - 1)`; brick is type 2, concrete 5.
        assert_eq!(super::surface(1 << 1).x, 2.0);
        assert_eq!(super::surface(1 << 4).x, 5.0);
        assert_eq!(super::surface(0).x, 0.0);
    }
}
