//! Black Ops weapon files: `weapons/mp/<variant>` in the `.iwd` archives,
//! plain text (`WEAPONFILE\key\value\...`) with every stat, model, sound and
//! animation of one weapon variant, e.g. `ak47_reflex_mp`.
//!
//! Like CoD4, attachments are parts of one gun model: each variant names the
//! same `gunModel` and lists the tags it hides (`hideTags`), so a variant
//! shows exactly its own attachments.

use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct WeaponFile {
    /// The variant name, e.g. `ak47_reflex_mp`.
    pub name: String,
    fields: HashMap<String, String>,
}

impl WeaponFile {
    /// Parse `WEAPONFILE\key\value\key\value...`.
    pub fn parse(name: &str, text: &str) -> Option<WeaponFile> {
        let mut parts = text.split('\\');
        if parts.next()? != "WEAPONFILE" {
            return None;
        }
        let mut fields = HashMap::new();
        while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
            fields.insert(k.to_owned(), v.to_owned());
        }
        Some(WeaponFile { name: name.to_owned(), fields })
    }

    /// A field's text ("" if absent).
    pub fn get(&self, key: &str) -> &str {
        self.fields.get(key).map_or("", String::as_str)
    }

    pub fn float(&self, key: &str) -> f32 {
        self.get(key).trim().parse().unwrap_or(0.0)
    }

    pub fn int(&self, key: &str) -> i32 {
        self.get(key).trim().parse::<f32>().map_or(0, |v| v as i32)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.fields.keys().map(String::as_str)
    }

    pub fn gun_model(&self) -> &str {
        self.get("gunModel")
    }

    pub fn world_model(&self) -> &str {
        self.get("worldModel")
    }

    pub fn hand_model(&self) -> &str {
        self.get("handModel")
    }

    /// Tags of the gun model this variant hides (attachments it lacks).
    pub fn hide_tags(&self) -> Vec<String> {
        self.get("hideTags").lines().map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned).collect()
    }

    /// A viewmodel animation by its key (`idleAnim`, `fireAnim`, ...).
    pub fn anim(&self, key: &str) -> Option<&str> {
        Some(self.get(key)).filter(|a| !a.is_empty())
    }
}

/// Every multiplayer weapon file, by variant name.
pub fn mp_weapons(vfs: &iw3::iwd::Vfs) -> Vec<WeaponFile> {
    let names: Vec<String> = vfs.files().filter(|f| f.starts_with("weapons/mp/")).map(str::to_owned).collect();
    let mut out: Vec<WeaponFile> = names
        .iter()
        .filter_map(|path| {
            let data = vfs.read(path).ok().flatten()?;
            WeaponFile::parse(path.trim_start_matches("weapons/mp/"), &String::from_utf8_lossy(&data))
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_weapon_files() {
        let w = WeaponFile::parse("x_mp", "WEAPONFILE\\displayName\\WEAPON_X\\hideTags\\tag_a\ntag_b\n\\clipSize\\30").unwrap();
        assert_eq!(w.get("displayName"), "WEAPON_X");
        assert_eq!(w.hide_tags(), ["tag_a", "tag_b"]);
        assert_eq!(w.int("clipSize"), 30);
        assert_eq!(w.get("missing"), "");
    }
}
