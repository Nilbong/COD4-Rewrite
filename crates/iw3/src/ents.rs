//! The Radiant entity string: `{ "key" "value" ... }` blocks.

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct Entity {
    pub fields: HashMap<String, String>,
}

impl Entity {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    pub fn classname(&self) -> &str {
        self.get("classname").unwrap_or("")
    }

    pub fn vec3(&self, key: &str) -> Option<[f32; 3]> {
        let mut it = self.get(key)?.split_whitespace().map(|s| s.parse::<f32>());
        Some([it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?])
    }

    pub fn origin(&self) -> Option<[f32; 3]> {
        self.vec3("origin")
    }

    /// Pitch/yaw/roll in degrees, from `angles` or a bare `angle` (yaw).
    pub fn angles(&self) -> [f32; 3] {
        if let Some(a) = self.vec3("angles") {
            return a;
        }
        let yaw = self.get("angle").and_then(|s| s.parse().ok()).unwrap_or(0.0);
        [0.0, yaw, 0.0]
    }
}

pub fn parse(s: &str) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut cur: Option<Entity> = None;
    let mut chars = s.chars().peekable();
    let mut pending_key: Option<String> = None;
    while let Some(c) = chars.next() {
        match c {
            '{' => cur = Some(Entity::default()),
            '}' => {
                if let Some(e) = cur.take() {
                    out.push(e);
                }
                pending_key = None;
            }
            '"' => {
                let mut tok = String::new();
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    tok.push(c);
                }
                match pending_key.take() {
                    None => pending_key = Some(tok),
                    Some(k) => {
                        if let Some(e) = cur.as_mut() {
                            e.fields.insert(k, tok);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_blocks() {
        let ents = super::parse(
            "{\n\"classname\" \"worldspawn\"\n}\n{\n\"origin\" \"1 2 3\"\n\"classname\" \"mp_tdm_spawn\"\n\"angles\" \"0 90 0\"\n}\n",
        );
        assert_eq!(ents.len(), 2);
        assert_eq!(ents[1].classname(), "mp_tdm_spawn");
        assert_eq!(ents[1].origin(), Some([1.0, 2.0, 3.0]));
        assert_eq!(ents[1].angles(), [0.0, 90.0, 0.0]);
    }
}
