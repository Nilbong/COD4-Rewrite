//! The frontend's content: the UI zones (`code_post_gfx_mp`,
//! `localized_code_post_gfx_mp`, `ui_mp`) and their materials as Bevy images.

use crate::textures::TextureCache;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use iw3::menu::{Font, Menu, StringTable, UiData};
use iw3::zone::{Asset, AssetId, ParseOptions, Zone};
use std::collections::HashMap;
use std::sync::Arc;

const ZONES: [&str; 3] = ["code_post_gfx_mp", "localized_code_post_gfx_mp", "ui_mp"];

#[derive(Clone)]
pub struct UiImage {
    pub handle: Handle<Image>,
    /// Pixel size, for converting glyph UVs to sprite rects.
    pub size: Vec2,
}

pub struct UiAssets {
    zones: Vec<Zone>,
    vfs: Arc<iw3::iwd::Vfs>,
    textures: TextureCache,
    /// Black Ops' menu pictures (its guns', attachments' and camos').
    bo1_textures: TextureCache,
    /// World at War's pictures and kill icons (`t4/` materials).
    waw_textures: TextureCache,
    /// Lower-cased menu name -> menu.
    pub menus: HashMap<String, Arc<Menu>>,
    pub fonts: Vec<Font>,
    /// Localized strings by upper-cased key.
    strings: HashMap<String, String>,
    /// String tables by lower-cased name.
    pub tables: HashMap<String, StringTable>,
    /// Sound aliases by lower-cased name.
    pub sounds: HashMap<String, Vec<crate::audio::bank::Variant>>,
    material_index: HashMap<String, (usize, AssetId)>,
    materials: HashMap<String, Option<UiImage>>,
    /// Materials of other zones (the match's): name -> image name.
    other_materials: HashMap<String, String>,
}

impl UiAssets {
    pub fn load() -> anyhow::Result<UiAssets> {
        let t0 = std::time::Instant::now();
        let install = iw3::Install::locate()?;
        let vfs = Arc::new(iw3::iwd::Vfs::mount(&install.iwd_paths()?)?);
        let mut zones = Vec::new();
        for name in ZONES {
            let data = iw3::fastfile::load(&install.zone_path(name))?;
            let zone = Zone::parse(&data, ParseOptions::default())?;
            if let Some(stop) = &zone.stats.stopped_at {
                warn!("{name}: stopped early ({stop})");
            }
            zones.push(zone);
        }
        let mut out = UiAssets {
            zones: Vec::new(),
            vfs,
            textures: TextureCache::default(),
            bo1_textures: TextureCache::default(),
            waw_textures: TextureCache::default(),
            menus: HashMap::new(),
            fonts: Vec::new(),
            strings: HashMap::new(),
            tables: HashMap::new(),
            sounds: HashMap::new(),
            material_index: HashMap::new(),
            other_materials: HashMap::new(),
            materials: HashMap::new(),
        };
        for (zi, zone) in zones.iter().enumerate() {
            let data = UiData::from_zone(zone);
            for m in data.menus {
                out.menus.insert(m.window.name.to_ascii_lowercase(), Arc::new(m));
            }
            out.fonts.extend(data.fonts);
            out.strings.extend(data.strings.into_iter().map(|(k, v)| (k.to_ascii_uppercase(), v)));
            for t in data.tables {
                out.tables.insert(t.name.to_ascii_lowercase(), t);
            }
            out.sounds.extend(crate::audio::bank::aliases(zone));
            for (id, asset) in zone.assets.iter().enumerate() {
                if let Asset::Material(m) = asset {
                    // Names with a leading comma are references to another zone's copy.
                    if !m.name.starts_with(',') && !m.textures.is_empty() {
                        out.material_index.entry(m.name.to_ascii_lowercase()).or_insert((zi, id));
                    }
                }
            }
        }
        out.zones = zones;
        // Black Ops' guns, attachments and camos, and World at War's guns and
        // attachments, for Create a Class.
        super::bo1::extend(&mut out.strings, &mut out.tables);
        super::waw::extend(&mut out.strings, &mut out.tables);
        super::camos::add(&mut out.strings, &mut out.tables, &mut out.menus);
        // Options > Game's rows for the sniper scope's style and the film's
        // tint.
        super::options::add(&mut out.menus);
        info!(
            "ui: {} menus, {} fonts, {} strings, {} tables, {} materials in {:.2?}",
            out.menus.len(),
            out.fonts.len(),
            out.strings.len(),
            out.tables.len(),
            out.material_index.len(),
            t0.elapsed()
        );
        Ok(out)
    }

    pub fn vfs(&self) -> Arc<iw3::iwd::Vfs> {
        self.vfs.clone()
    }

    pub fn menu(&self, name: &str) -> Option<Arc<Menu>> {
        self.menus.get(&name.to_ascii_lowercase()).cloned()
    }

    /// `@KEY` -> its localized text; anything else is returned as is.
    pub fn localize(&self, text: &str) -> String {
        match text.strip_prefix('@') {
            Some(key) => self.strings.get(&key.to_ascii_uppercase()).cloned().unwrap_or_else(|| key.to_owned()),
            None => text.to_owned(),
        }
    }

    pub fn table(&self, name: &str) -> Option<&StringTable> {
        self.tables.get(&name.to_ascii_lowercase())
    }

    /// Make another zone's materials drawable (the HUD's compass and
    /// overlays are in `common_mp` and the map's zone).
    pub fn add_materials(&mut self, zone: &Zone) {
        for asset in &zone.assets {
            let Asset::Material(m) = asset else { continue };
            if m.name.starts_with(',') {
                continue;
            }
            let image = m.textures.first().and_then(|t| t.image).and_then(|i| zone.image(i)).map(|i| i.name.clone());
            if let Some(image) = image {
                self.other_materials.entry(m.name.to_ascii_lowercase()).or_insert(image);
            }
        }
    }

    /// Draw `material` with the image `image` (for materials in zones not
    /// loaded).
    pub fn add_material(&mut self, material: &str, image: &str) {
        self.other_materials.entry(material.to_ascii_lowercase()).or_insert_with(|| image.to_owned());
    }

    /// A UI image, clamped so stretched gradients don't bleed in the
    /// opposite edge.
    fn clamped(handle: Handle<Image>, images: &mut Assets<Image>) -> Option<UiImage> {
        let mut img = images.get_mut(&handle)?;
        if let ImageSampler::Descriptor(d) = &mut img.sampler {
            *d = ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::ClampToEdge,
                address_mode_v: ImageAddressMode::ClampToEdge,
                ..d.clone()
            };
        }
        Some(UiImage { size: img.size().as_vec2(), handle })
    }

    /// The image behind a 2D material, loaded on first use. Names no zone
    /// has as a material are tried as images (`damage_feedback`).
    pub fn material(&mut self, name: &str, images: &mut Assets<Image>) -> Option<UiImage> {
        let key = name.trim_start_matches(',').to_ascii_lowercase();
        if let Some(m) = self.materials.get(&key) {
            return m.clone();
        }
        if key == super::camos::PLATINUM_SWATCH || key == super::camos::DIAMOND_SWATCH {
            let handle = if key == super::camos::DIAMOND_SWATCH {
                crate::gunmodel::platinum::diamond_texture(images)?
            } else {
                crate::gunmodel::platinum::texture(images)?
            };
            let image = Self::clamped(handle, images);
            self.materials.insert(key, image.clone());
            return image;
        }
        // World at War's own materials, from its zones and iwds; a kill icon
        // only once its guns' content has loaded, so not found isn't kept.
        if let Some(waw_name) = key.strip_prefix(crate::waw::MATERIAL_PREFIX) {
            let data = crate::waw::data()?;
            let image = data.material_image(waw_name)?;
            let handle = self.waw_textures.get(&image, true, &data.vfs, images)?;
            let image = Self::clamped(handle, images);
            self.materials.insert(key, image.clone());
            return image;
        }
        // Black Ops' scope pictures, likewise once its guns have loaded.
        if let Some(bo1_name) = key.strip_prefix(crate::bo1::MATERIAL_PREFIX) {
            let data = crate::bo1::data()?;
            let image = data.material_image(bo1_name)?;
            let handle = self.bo1_textures.get(&image, true, &data.vfs, images)?;
            let image = Self::clamped(handle, images);
            self.materials.insert(key, image.clone());
            return image;
        }
        let image_name = match self.material_index.get(&key) {
            Some(&(zi, id)) => {
                let zone = &self.zones[zi];
                zone.material(id).and_then(|mat| mat.textures.first()?.image.and_then(|i| zone.image(i))).map(|i| i.name.clone())
            }
            None => Some(self.other_materials.get(&key).cloned().unwrap_or_else(|| key.clone())),
        };
        let image = image_name.and_then(|image_name| {
            let handle = self.textures.get(&image_name, true, &self.vfs, images).or_else(|| {
                let bo1 = crate::bo1::data()?;
                self.bo1_textures.get(&image_name, true, &bo1.vfs, images)
            })?;
            Self::clamped(handle, images)
        });
        if image.is_none() {
            debug!("ui material {name} not found");
        }
        self.materials.insert(key, image.clone());
        image
    }
}
