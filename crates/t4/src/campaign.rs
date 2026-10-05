//! World at War's campaign characters: the named cast (Roebuck, Sullivan,
//! Polonsky, Reznov, Chernov, ...), the co-op player characters, and every
//! soldier type of the Marines, Raiders, Navy and PBY crews, the Red Army,
//! the Imperial Japanese Army and the Wehrmacht, in their dry and wet
//! (rain, swamp) gear.
//!
//! Each is put together by a script in the level zones
//! (`character/char_usa_marine_r_rifle.gsc`): a body plus attached head, hat
//! and gear models. Soldier types pick each part at random from a list
//! (`xmodelalias/char_usa_marine_headalias.gsc`), so a [`Look`] holds every
//! choice; the named cast have one model per part, or a single body with the
//! head built in.

/// A model, or the models the game picks one of at random.
pub type Choices = Vec<String>;

/// The levels, in campaign order, then `common` (the co-op player
/// characters' scripts; their models are in the levels).
pub const CAMPAIGN_ZONES: [&str; 16] = [
    "mak", "pel1", "pel1a", "sniper", "see1", "pel1b", "pel2", "oki2", "oki3", "pby_fly", "see2", "ber1", "ber2", "ber3", "ber3b",
    "common",
];

/// What a character script attaches. `head`, `hat` and `gear` hang off the
/// body's own bones (attached with no tag).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub body: Choices,
    pub head: Choices,
    pub hat: Choices,
    pub gear: Choices,
    /// `american`, `russian`, `japanese` or `german`.
    pub voice: String,
}

impl Look {
    /// Parse a character script's `main()`. `alias` gives the models of an
    /// `xmodelalias` list by name (`char_usa_marine_headalias`).
    pub fn parse(text: &str, alias: impl Fn(&str) -> Choices) -> Option<Look> {
        let main = text.split("precache()").next().unwrap_or(text);
        let mut look = Look::default();
        for line in main.lines().map(str::trim) {
            // A part is a literal model or a random pick from an alias list.
            let choices = || match line.split_once("xmodelalias\\") {
                Some((_, rest)) => alias(rest.split("::").next().unwrap_or_default()),
                None => quoted(line).into_iter().collect(),
            };
            if line.starts_with("self setModel(") || line.contains("setModelFromArray(") {
                look.body = choices();
            } else if line.starts_with("self.headModel =") {
                look.head = choices();
            } else if line.starts_with("self.hatModel =") {
                look.hat = choices();
            } else if line.starts_with("self.gearModel =") {
                look.gear = choices();
            } else if line.starts_with("self.voice =") {
                look.voice = quoted(line).unwrap_or_default();
            }
        }
        (!look.body.is_empty()).then_some(look)
    }

    /// Every model it may use.
    pub fn models(&self) -> impl Iterator<Item = &str> {
        [&self.body, &self.head, &self.hat, &self.gear].into_iter().flatten().map(String::as_str)
    }
}

/// The models of an `xmodelalias` script (`a[0] = "model"; ...`).
pub fn alias_models(text: &str) -> Choices {
    text.lines().filter(|l| l.trim_start().starts_with("a[")).filter_map(quoted).collect()
}

fn quoted(line: &str) -> Option<String> {
    let start = line.find('"')? + 1;
    let len = line[start..].find('"')?;
    Some(line[start..start + len].to_owned())
}

/// A campaign character.
#[derive(Clone, Debug)]
pub struct CampaignCharacter {
    /// The script's name without `character/char_`: `usa_marine_h_roebuck`,
    /// `rus_r_ppsh`, `jap_makpel_rifle`, ...
    pub name: String,
    pub look: Look,
    /// A zone holding every model of the look.
    pub zone: String,
}

/// One zone's contents for [`characters`]: its name, its models and its raw
/// files `(path, text)`.
pub type ZoneContents<'a> = (&'a str, Vec<&'a str>, Vec<(&'a str, &'a str)>);

/// Every campaign character whose models are all in one of `zones` (given in
/// [`CAMPAIGN_ZONES`] order; the first zone with everything is chosen).
/// A choice missing from every zone is dropped, keeping the rest.
pub fn characters(zones: &[ZoneContents]) -> Vec<CampaignCharacter> {
    let mut aliases = std::collections::HashMap::new();
    let mut scripts = std::collections::BTreeMap::new();
    for (_, _, raw) in zones {
        for &(path, text) in raw {
            if let Some(name) = path.strip_prefix("xmodelalias/").and_then(|p| p.strip_suffix(".gsc")) {
                aliases.entry(name).or_insert_with(|| alias_models(text));
            } else if let Some(name) = path.strip_prefix("character/char_").and_then(|p| p.strip_suffix(".gsc")) {
                scripts.entry(name).or_insert(text);
            }
        }
    }
    // Scripts name models in any case (`char_usa_marine_helmF`); the
    // catalog uses the zone's spelling.
    let spelling: Vec<std::collections::HashMap<String, &str>> =
        zones.iter().map(|z| z.1.iter().map(|m| (m.to_ascii_lowercase(), *m)).collect()).collect();
    let mut out = Vec::new();
    for (name, text) in scripts {
        let Some(mut look) = Look::parse(text, |a| aliases.get(a).cloned().unwrap_or_default()) else { continue };
        let has = |z: usize, m: &str| spelling[z].contains_key(&m.to_ascii_lowercase());
        let zone = (0..zones.len()).find(|&z| look.models().all(|m| has(z, m))).or_else(|| {
            // Otherwise the zone with the most of it, keeping what it has.
            (0..zones.len()).min_by_key(|&z| std::cmp::Reverse(look.models().filter(|m| has(z, m)).count()))
        });
        let Some(z) = zone else { continue };
        for part in [&mut look.body, &mut look.head, &mut look.hat, &mut look.gear] {
            *part = part.iter().filter_map(|m| spelling[z].get(&m.to_ascii_lowercase()).map(|s| s.to_string())).collect();
        }
        if look.body.is_empty() {
            continue;
        }
        out.push(CampaignCharacter { name: name.to_owned(), look, zone: zones[z].0.to_owned() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_campaign_scripts() {
        let script = "main()\n{\n\tcodescripts\\character::setModelFromArray(xmodelalias\\bodies::main());\n\
            \tself.headModel = \"head_a\";\n\tself attach(self.headModel, \"\", true);\n\
            \tself.hatModel = codescripts\\character::randomElement(xmodelalias\\hats::main());\n\
            \tself.voice = \"american\";\n}\n\nprecache()\n{\n\tprecacheModel(\"other\");\n}";
        let bodies = "main()\n{\n\ta[0] = \"body_a\";\n\ta[1] = \"body_b\";\n\treturn a;\n}";
        let hats = "main()\n{\n\ta[0] = \"hat_a\";\n\treturn a;\n}";
        let raw = vec![("character/char_usa_x.gsc", script), ("xmodelalias/bodies.gsc", bodies), ("xmodelalias/hats.gsc", hats)];
        // Model names match in any case; the zone's spelling wins.
        let chars = characters(&[("zone_a", vec!["body_a", "Head_A", "hat_a"], raw)]);
        let c = &chars[0];
        assert_eq!((c.name.as_str(), c.zone.as_str()), ("usa_x", "zone_a"));
        // `body_b` is in no zone, so it is dropped.
        assert_eq!(c.look.body, ["body_a"]);
        assert_eq!((c.look.head.as_slice(), c.look.hat.as_slice(), c.look.voice.as_str()), (&["Head_A".to_owned()][..], &["hat_a".to_owned()][..], "american"));
        assert!(c.look.gear.is_empty());
    }
}
