//! The game's settings: every option the menus show (Options and Controls,
//! [`crate::ui::settings_menu`]), its dvar, how it's chosen and its default.
//! Their values live in the menus' dvars and are saved with the player's
//! stats; [`Settings`] holds them for the game (defaults when there are no
//! menus), and [`crate::settings_apply`] puts them into effect.

use crate::bindings::Action;
use bevy::prelude::*;
use std::collections::HashMap;

/// How a setting is chosen.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// Off / On ("0" / "1").
    Toggle,
    /// One of these (label, value).
    Choice(&'static [(&'static str, &'static str)]),
    /// A number from `min` to `max` in `step`s, shown with `decimals` and a
    /// `suffix`.
    Slider { min: f32, max: f32, step: f32, decimals: u8, suffix: &'static str },
    /// The keys bound to an action ([`crate::bindings`]).
    Bind(Action),
}

/// One setting: its dvar, its row's label, a line about it, how it's
/// chosen, and its default.
#[derive(Clone, Copy, Debug)]
pub struct Setting {
    pub dvar: &'static str,
    pub label: &'static str,
    pub desc: &'static str,
    pub kind: Kind,
    pub default: &'static str,
}

/// A page of settings: the CoD4 menu it takes over and its rows.
pub struct Page {
    pub menu: &'static str,
    pub title: &'static str,
    pub rows: &'static [Setting],
}

const OFF_ON: Kind = Kind::Toggle;
const HOLD_TOGGLE: Kind = Kind::Choice(&[("Hold", "hold"), ("Toggle", "toggle")]);
const PERCENT: Kind = Kind::Slider { min: 0.0, max: 100.0, step: 5.0, decimals: 0, suffix: "%" };

const fn s(dvar: &'static str, label: &'static str, desc: &'static str, kind: Kind, default: &'static str) -> Setting {
    Setting { dvar, label, desc, kind, default }
}

const fn bind(action: Action, label: &'static str) -> Setting {
    Setting { dvar: "", label, desc: "Select, then press a key or mouse button (Backspace clears, Escape cancels).", kind: Kind::Bind(action), default: "" }
}

/// Options > Graphics.
pub const GRAPHICS: &[Setting] = &[
    s("r_preset", "Quality Preset", "Sets every quality setting at once. Changing one afterwards makes it Custom.", Kind::Choice(&[("Low", "low"), ("Medium", "medium"), ("High", "high"), ("Ultra", "ultra"), ("Custom", "custom")]), "high"),
    s("r_displaymode", "Display Mode", "Windowed, borderless full screen, or exclusive full screen.", Kind::Choice(&[("Windowed", "windowed"), ("Borderless", "borderless"), ("Fullscreen", "fullscreen")]), "windowed"),
    s("r_resolution", "Resolution", "The window's size (full screen: the screen's own unless set).", Kind::Choice(&[("Desktop", "desktop"), ("1280x720", "1280x720"), ("1366x768", "1366x768"), ("1600x900", "1600x900"), ("1920x1080", "1920x1080"), ("2560x1440", "2560x1440"), ("3440x1440", "3440x1440"), ("3840x2160", "3840x2160")]), "desktop"),
    s("r_vsync", "Sync Every Frame", "Wait for the display's refresh (no tearing, a little more latency).", OFF_ON, "1"),
    s("com_maxfps", "Frame Rate Limit", "The most frames a second drawn.", Kind::Choice(&[("30", "30"), ("60", "60"), ("90", "90"), ("120", "120"), ("144", "144"), ("165", "165"), ("240", "240"), ("Unlimited", "0")]), "0"),
    s("cg_fov", "Field of View", "CoD4's is 65; 80 was its widest.", Kind::Slider { min: 65.0, max: 110.0, step: 1.0, decimals: 0, suffix: "" }, "65"),
    s("r_gamma", "Brightness", "How bright the picture is.", Kind::Slider { min: 0.5, max: 1.5, step: 0.05, decimals: 2, suffix: "" }, "1"),
    s("r_lighting", "Lighting", "Baked is CoD4's lightmaps; ray traced lights the world live (from the next match).", Kind::Choice(&[("Baked", "baked"), ("Ray Traced Low", "rt_low"), ("Ray Traced High", "rt_high")]), "baked"),
    s("r_filmtint", "Film Tint", "Keep each map's colour tint (Killhouse's yellow, for one).", Kind::Choice(&[("Off", "off"), ("On", "on")]), "off"),
    s("cg_drawfps", "Performance Overlay", "Frames a second and frame time in the corner.", Kind::Choice(&[("Off", "0"), ("FPS", "1"), ("FPS and Frame Time", "2")]), "0"),
];

/// Options > Quality (CoD4's Texture Settings page).
pub const QUALITY: &[Setting] = &[
    s("r_shadows", "Sun Shadows", "How far and how sharp the sun's shadows are.", Kind::Choice(&[("Off", "off"), ("Low", "low"), ("Medium", "medium"), ("High", "high"), ("Ultra", "ultra")]), "high"),
    s("r_propshadows", "Model Shadows", "Static models cast sun shadows too (their own are in the lightmaps already).", OFF_ON, "1"),
    s("r_ssao", "Ambient Occlusion", "Soft shadowing in corners and creases.", OFF_ON, "1"),
    s("r_aa", "Anti-aliasing", "Smooths jagged edges.", Kind::Choice(&[("Off", "off"), ("FXAA", "fxaa"), ("SMAA", "smaa"), ("SMAA High", "smaa_high")]), "smaa_high"),
    s("r_bloom", "Glow", "Bright light glows (maps that have it).", OFF_ON, "1"),
    s("r_texfilter", "Texture Filtering", "Sharper textures at a glancing angle.", Kind::Choice(&[("Bilinear", "bilinear"), ("Trilinear", "trilinear"), ("Anisotropic 4x", "4"), ("Anisotropic 8x", "8"), ("Anisotropic 16x", "16")]), "16"),
    s("r_drawdistance", "Draw Distance", "How far away small static models are still drawn.", Kind::Choice(&[("Near", "near"), ("Medium", "medium"), ("Far", "far")]), "far"),
    s("fx_density", "Effects", "How many particles smoke, fire and impacts make.", Kind::Choice(&[("Low", "low"), ("Medium", "medium"), ("High", "high")]), "high"),
    s("fx_mapfx", "Map Effects", "The maps' ambient effects: smoke, fires, dust, insects.", OFF_ON, "1"),
    s("ragdoll_enable", "Ragdolls", "Bodies go limp and fall as they would.", Kind::Choice(&[("On", "1"), ("Off", "0")]), "1"),
    s("r_corpses", "Number of Corpses", "How many bodies stay after their players respawn.", Kind::Choice(&[("Tiny", "2"), ("Small", "4"), ("Medium", "8"), ("Large", "16")]), "8"),
    s("phys_clutter", "Clutter Physics", "Bottles, cans and boxes react to shots and blasts.", OFF_ON, "1"),
];

/// Options > Sound.
pub const SOUND: &[Setting] = &[
    s("snd_volume", "Master Volume", "Everything.", PERCENT, "90"),
    s("snd_effects", "Effects Volume", "Weapons, footsteps, explosions and the world.", PERCENT, "100"),
    s("snd_music", "Music Volume", "Menus' and matches' music.", PERCENT, "100"),
    s("snd_voice", "Voice Volume", "The announcer and battle chatter.", PERCENT, "100"),
    s("snd_ui", "Menu Volume", "Menu clicks and sounds.", PERCENT, "100"),
    s("snd_mute_unfocused", "Mute When Unfocused", "Silence the game while its window isn't in front.", OFF_ON, "0"),
    s("cg_hitmarker_sound", "Hit Marker Sound", "The tick when your shot hurts someone.", OFF_ON, "1"),
];

/// Options > Game Options.
pub const GAME: &[Setting] = &[
    s("cg_crosshair", "Crosshair", "The hip-fire crosshair (hardcore never shows it).", OFF_ON, "1"),
    s("cg_hitmarkers", "Hit Markers", "The cross that shows your shot hurt someone.", OFF_ON, "1"),
    s("cg_killfeed", "Kill Feed", "Who killed whom, at the bottom left.", OFF_ON, "1"),
    s("cg_minimap_rotate", "Minimap", "Turn the minimap with you, or keep north up.", Kind::Choice(&[("Rotating", "1"), ("North Up", "0")]), "1"),
    s("cg_damage_direction", "Damage Direction", "Red arcs showing where you're hit from.", OFF_ON, "1"),
    s("cg_scopestyle", "Sniper Scope", "CoD4's scope, or a lens you see through.", Kind::Choice(&[("Classic", "classic"), ("Lens", "lens")]), "classic"),
    s("cg_xp_popups", "Score Popups", "+10 and the like as you score.", OFF_ON, "1"),
];

/// Controls > Look.
pub const LOOK: &[Setting] = &[
    s("sensitivity", "Mouse Sensitivity", "How far the view turns as the mouse moves (CoD4's scale: 5 is its default).", Kind::Slider { min: 0.5, max: 20.0, step: 0.25, decimals: 2, suffix: "" }, "5"),
    s("cg_ads_sens", "Aiming Sensitivity", "Sensitivity while aiming down the sights, as a share of the above.", Kind::Slider { min: 0.25, max: 2.0, step: 0.05, decimals: 2, suffix: "x" }, "1"),
    s("m_invert", "Invert Mouse", "Mouse up looks down.", OFF_ON, "0"),
    s("m_raw", "Raw Mouse Input", "The mouse's own counts, with no acceleration (always: the game reads raw motion).", Kind::Choice(&[("On", "1")]), "1"),
    bind(Action::LeanLeft, "Lean Left"),
    bind(Action::LeanRight, "Lean Right"),
    s("cg_lean_mode", "Lean", "Hold the key, or press once to lean and again to stop.", HOLD_TOGGLE, "hold"),
];

/// Controls > Move.
pub const MOVE: &[Setting] = &[
    bind(Action::Forward, "Forward"),
    bind(Action::Back, "Backpedal"),
    bind(Action::Left, "Move Left"),
    bind(Action::Right, "Move Right"),
    bind(Action::Jump, "Stand / Jump"),
    bind(Action::Crouch, "Crouch"),
    bind(Action::Prone, "Prone"),
    s("cg_crouch_mode", "Crouch and Prone", "CoD4's keys toggle; held, you're down while the key is.", Kind::Choice(&[("Toggle", "toggle"), ("Hold", "hold")]), "toggle"),
    bind(Action::Sprint, "Sprint / Hold Breath"),
    s("cg_sprint_mode", "Sprint", "Hold the key, or press once to sprint until you stop.", HOLD_TOGGLE, "hold"),
];

/// Controls > Combat.
pub const COMBAT: &[Setting] = &[
    bind(Action::Fire, "Attack"),
    bind(Action::Aim, "Aim Down the Sight"),
    s("cg_ads_mode", "Aim Down the Sight", "Hold to aim, or press once to aim and again to stop.", HOLD_TOGGLE, "hold"),
    bind(Action::Melee, "Melee Attack"),
    bind(Action::Reload, "Reload Weapon"),
    bind(Action::Weapon1, "Primary Weapon"),
    bind(Action::Weapon2, "Secondary Weapon"),
    bind(Action::Frag, "Throw Frag Grenade"),
    bind(Action::Special, "Throw Special Grenade"),
    bind(Action::Equipment, "Equipment"),
    bind(Action::Killstreak, "Kill Streak Reward"),
    bind(Action::Inspect, "Inspect Weapon"),
];

/// Controls > Interact.
pub const INTERACT: &[Setting] = &[
    bind(Action::Use, "Use"),
    bind(Action::NightVision, "Night Vision"),
    bind(Action::Scores, "Show Objectives/Scores"),
    bind(Action::ThirdPerson, "Third Person"),
];

/// Controls > Controller (CoD4's Multiplayer Controls page).
pub const CONTROLLER: &[Setting] = &[
    s("pad_sens", "Look Sensitivity", "How fast the right stick turns the view.", Kind::Slider { min: 0.2, max: 3.0, step: 0.1, decimals: 1, suffix: "x" }, "1"),
    s("pad_ads_sens", "Aiming Sensitivity", "The right stick while aiming down the sights, as a share of the above.", Kind::Slider { min: 0.25, max: 2.0, step: 0.05, decimals: 2, suffix: "x" }, "1"),
    s("pad_deadzone_left", "Move Deadzone", "How far the left stick moves before it counts.", Kind::Slider { min: 0.02, max: 0.4, step: 0.01, decimals: 2, suffix: "" }, "0.15"),
    s("pad_deadzone_right", "Look Deadzone", "How far the right stick moves before it counts.", Kind::Slider { min: 0.02, max: 0.4, step: 0.01, decimals: 2, suffix: "" }, "0.15"),
    s("pad_invert", "Invert Look", "Stick up looks down.", OFF_ON, "0"),
    s("pad_aim_assist", "Aim Assist", "The view slows over enemies and follows them a little.", OFF_ON, "1"),
    s("pad_rumble", "Vibration", "The controller rumbles when you fire and are hit.", OFF_ON, "1"),
    s("pad_layout", "Button Layout", "Default, or Tactical (crouch and melee swapped).", Kind::Choice(&[("Default", "default"), ("Tactical", "tactical")]), "default"),
    s("pad_glyphs", "Button Icons", "Which controller's buttons the hints show.", Kind::Choice(&[("Automatic", "auto"), ("Xbox", "xbox"), ("PlayStation", "ps")]), "auto"),
];

/// The pages, by the CoD4 menu each takes over.
pub const PAGES: &[Page] = &[
    Page { menu: "options_graphics", title: "Graphics", rows: GRAPHICS },
    Page { menu: "options_graphics_texture", title: "Quality", rows: QUALITY },
    Page { menu: "options_sound", title: "Sound", rows: SOUND },
    Page { menu: "options_game", title: "Game Options", rows: GAME },
    Page { menu: "options_look", title: "Look", rows: LOOK },
    Page { menu: "options_move", title: "Move", rows: MOVE },
    Page { menu: "options_shoot", title: "Combat", rows: COMBAT },
    Page { menu: "options_misc", title: "Interact", rows: INTERACT },
    Page { menu: "controls_multi", title: "Controller", rows: CONTROLLER },
];

/// Every setting.
pub fn all() -> impl Iterator<Item = &'static Setting> {
    PAGES.iter().flat_map(|p| p.rows.iter())
}

/// The dvar a row stores in (a bind's is its action's).
pub fn dvar_of(s: &Setting) -> &'static str {
    match s.kind {
        Kind::Bind(a) => a.dvar(),
        _ => s.dvar,
    }
}

/// A setting by its dvar.
pub fn find(dvar: &str) -> Option<&'static Setting> {
    all().find(|s| dvar_of(s).eq_ignore_ascii_case(dvar))
}

/// Is this dvar a setting's (saved with the stats)?
pub fn is_setting(dvar: &str) -> bool {
    find(dvar).is_some()
}

/// A setting's default value.
pub fn default_of(s: &Setting) -> String {
    match s.kind {
        Kind::Bind(a) => a.default_value(),
        _ => s.default.to_owned(),
    }
}

/// The quality preset's values, Low to Ultra.
pub const PRESET_DVARS: &[(&str, [&str; 4])] = &[
    ("r_shadows", ["low", "medium", "high", "ultra"]),
    ("r_propshadows", ["0", "0", "1", "1"]),
    ("r_ssao", ["0", "0", "1", "1"]),
    ("r_aa", ["fxaa", "smaa", "smaa_high", "smaa_high"]),
    ("r_bloom", ["0", "1", "1", "1"]),
    ("r_texfilter", ["trilinear", "4", "16", "16"]),
    ("r_drawdistance", ["near", "medium", "far", "far"]),
    ("fx_density", ["low", "medium", "high", "high"]),
];

/// Which preset (0 Low .. 3 Ultra) the dvars match, if one.
pub fn preset_of(value: impl Fn(&str) -> String) -> Option<usize> {
    (0..4).find(|&i| PRESET_DVARS.iter().all(|(d, vals)| value(d) == vals[i]))
}

pub fn preset_index(name: &str) -> Option<usize> {
    ["low", "medium", "high", "ultra"].iter().position(|p| *p == name)
}

/// The settings in force: each setting's value (its default until set).
#[derive(Resource, Clone, Debug)]
pub struct Settings(HashMap<&'static str, String>);

impl Default for Settings {
    fn default() -> Self {
        let mut values: HashMap<&'static str, String> = all().map(|s| (dvar_of(s), default_of(s))).collect();
        // Debug runs straight into a match can still set these.
        if let Ok(v) = std::env::var("COD4RW_LIGHTING") {
            values.insert("r_lighting", v.to_ascii_lowercase());
        }
        if let Ok(v) = std::env::var("COD4RW_FILMTINT") {
            values.insert("r_filmtint", v);
        }
        Settings(values)
    }
}

impl Settings {
    /// A setting's value.
    pub fn text(&self, dvar: &str) -> &str {
        self.0.get(dvar).map_or("", |s| s.as_str())
    }

    pub fn num(&self, dvar: &str) -> f32 {
        self.text(dvar).trim().parse().unwrap_or(0.0)
    }

    pub fn on(&self, dvar: &str) -> bool {
        let v = self.text(dvar);
        !(v.is_empty() || v == "0" || v.eq_ignore_ascii_case("off"))
    }

    /// Set from the menus' dvars: whether anything changed.
    pub fn update(&mut self, value: impl Fn(&str) -> Option<String>) -> bool {
        let mut changed = false;
        for s in all() {
            let d = dvar_of(s);
            let v = value(d).unwrap_or_else(|| default_of(s));
            if self.0.get(d) != Some(&v) {
                self.0.insert(d, v);
                changed = true;
            }
        }
        changed
    }

    /// The bindings these settings give.
    pub fn bindings(&self) -> crate::bindings::Bindings {
        crate::bindings::Bindings::from_values(|d| self.0.get(d).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_fit_and_dvars_are_unique() {
        for p in PAGES {
            assert!(p.rows.len() <= 18, "{} has {} rows", p.menu, p.rows.len());
        }
        let mut seen = std::collections::HashSet::new();
        for s in all() {
            assert!(seen.insert(dvar_of(s)), "{} twice", dvar_of(s));
        }
    }

    #[test]
    fn presets_are_recognised() {
        let values: HashMap<&str, &str> = PRESET_DVARS.iter().map(|(d, v)| (*d, v[1])).collect();
        assert_eq!(preset_of(|d| values.get(d).map_or(String::new(), |v| v.to_string())), Some(1));
    }
}
