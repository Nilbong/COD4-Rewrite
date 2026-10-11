//! The new UI's design tokens: colours, type scale, spacing, motion. Every
//! screen and HUD element takes its look from here, so a change here
//! changes the whole UI.

use super::font::Cut;

pub type Rgba = [f32; 4];

/// Hex `0xRRGGBB` at `a`.
pub const fn hex(rgb: u32, a: f32) -> Rgba {
    [((rgb >> 16) & 0xff) as f32 / 255.0, ((rgb >> 8) & 0xff) as f32 / 255.0, (rgb & 0xff) as f32 / 255.0, a]
}

pub const fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], a]
}

// --- Surfaces (dark, slightly warm-green neutral greys) -------------------

/// The scrim over background art behind content.
pub const SCRIM: Rgba = hex(0x07090a, 0.55);
/// A panel.
pub const PANEL: Rgba = hex(0x0e1213, 0.78);
/// A raised panel (cards, popups).
pub const RAISED: Rgba = hex(0x171c1d, 0.88);
/// A row or card at rest.
pub const ROW: Rgba = hex(0x0e1213, 0.62);
/// A row or card under focus (with the accent bar).
pub const ROW_FOCUS: Rgba = hex(0x73d959, 0.16);
/// Pressed.
pub const ROW_PRESS: Rgba = hex(0x73d959, 0.30);

// --- Lines ----------------------------------------------------------------

pub const HAIRLINE: Rgba = hex(0xffffff, 0.10);
pub const LINE: Rgba = hex(0xffffff, 0.20);
pub const LINE_STRONG: Rgba = hex(0xffffff, 0.42);

// --- Ink --------------------------------------------------------------------

/// Primary text: a warm off-white.
pub const INK: Rgba = hex(0xeceae2, 1.0);
/// Secondary text.
pub const INK_DIM: Rgba = hex(0xa9aa9f, 1.0);
/// Disabled, captions.
pub const INK_MUTE: Rgba = hex(0x6d7069, 1.0);
/// Text on the accent.
pub const INK_ON_ACCENT: Rgba = hex(0x0b0d0a, 1.0);

// --- Accent and signals ---------------------------------------------------

/// The one accent: CoD4's "Modern Warfare" green, cleaned up.
pub const ACCENT: Rgba = hex(0x73d959, 1.0);
pub const ACCENT_DIM: Rgba = hex(0x4a8f3a, 1.0);
/// Friendlies (HUD, scoreboard). Not green, so it never reads as the accent.
pub const FRIENDLY: Rgba = hex(0x5cb8ff, 1.0);
pub const ENEMY: Rgba = hex(0xff5a47, 1.0);
pub const WARN: Rgba = hex(0xf0b43c, 1.0);
/// New / unlocked / rewards.
pub const REWARD: Rgba = hex(0xf2c94c, 1.0);

// --- Type scale (design units, 1080p reference) ---------------------------

/// (cut, size, tracking in em, upper-case).
#[derive(Clone, Copy, Debug)]
pub struct Type {
    pub cut: Cut,
    pub size: f32,
    pub tracking: f32,
    pub caps: bool,
}

/// Screen titles: "CREATE A CLASS".
pub const DISPLAY: Type = Type { cut: Cut::Display, size: 64.0, tracking: 0.02, caps: true };
/// Big numbers (ammo, score, timer).
pub const NUMERAL: Type = Type { cut: Cut::Display, size: 56.0, tracking: 0.0, caps: false };
/// Section titles, card titles.
pub const H1: Type = Type { cut: Cut::Display, size: 40.0, tracking: 0.02, caps: true };
pub const H2: Type = Type { cut: Cut::Semi, size: 28.0, tracking: 0.0, caps: false };
/// Top tabs.
pub const TAB: Type = Type { cut: Cut::Semi, size: 24.0, tracking: 0.12, caps: true };
/// List rows and buttons.
pub const ROW_TEXT: Type = Type { cut: Cut::Semi, size: 26.0, tracking: 0.01, caps: false };
pub const BODY: Type = Type { cut: Cut::Regular, size: 20.0, tracking: 0.0, caps: false };
/// Over-lines: "MULTIPLAYER", "PRIMARY".
pub const LABEL: Type = Type { cut: Cut::Semi, size: 16.0, tracking: 0.2, caps: true };
pub const CAPTION: Type = Type { cut: Cut::Regular, size: 17.0, tracking: 0.02, caps: false };
pub const MICRO: Type = Type { cut: Cut::Semi, size: 13.0, tracking: 0.16, caps: true };

// --- Spacing (8-unit grid) and layout --------------------------------------

/// The design canvas: 1920x1080, scaled uniformly to the window and centred.
pub const CANVAS: (f32, f32) = (1920.0, 1080.0);
/// Title-safe margins.
pub const SAFE_X: f32 = 96.0;
pub const SAFE_Y: f32 = 54.0;
/// 12 columns inside the safe area.
pub const COLUMNS: usize = 12;
pub const GUTTER: f32 = 24.0;
pub const fn col_width() -> f32 {
    (CANVAS.0 - 2.0 * SAFE_X - (COLUMNS as f32 - 1.0) * GUTTER) / COLUMNS as f32
}
/// Left edge of column `c` (0-based).
pub fn col_x(c: usize) -> f32 {
    SAFE_X + c as f32 * (col_width() + GUTTER)
}
/// Width spanning `n` columns.
pub fn cols(n: usize) -> f32 {
    n as f32 * col_width() + (n as f32 - 1.0).max(0.0) * GUTTER
}

pub const SPACE: f32 = 8.0;
/// The top bar (tabs) and the footer (prompts).
pub const TOP_BAR: f32 = 112.0;
pub const FOOTER_Y: f32 = 1080.0 - 54.0 - 40.0;
pub const ROW_H: f32 = 56.0;
/// Corner cut on panels and cards.
pub const CUT: f32 = 14.0;
/// The focus bar.
pub const FOCUS_BAR: f32 = 4.0;

// --- Motion (seconds) ---------------------------------------------------------

pub const FAST: f32 = 0.09;
pub const MEDIUM: f32 = 0.18;
pub const SLOW: f32 = 0.32;

/// Ease out (cubic).
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}
