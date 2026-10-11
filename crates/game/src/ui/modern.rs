//! The menu style switch (Options > Game > Menu Style, `ui_menustyle`:
//! `modern`, the default, or `classic`). With `modern`, CoD4's main menu
//! (`main_text`) is shown as the new UI's main menu ([`super::next::home`]).

use super::*;

/// The setting: `classic` keeps CoD4's main menu.
pub const STYLE_DVAR: &str = "ui_menustyle";

pub(super) fn build(_app: &mut App) {}

/// Whether the new main menu is wanted.
pub(super) fn wanted(fe: &Frontend) -> bool {
    !fe.dvar(STYLE_DVAR).eq_ignore_ascii_case("classic")
}

/// The new main menu built from the dressed `main_text`; `None` if none of
/// its buttons are found.
pub(super) fn menu(fe: &Frontend, classic: &Menu) -> Option<Menu> {
    super::next::home::menu(fe, classic)
}

/// Note the opening, for the fade in.
pub(super) fn opened(fe: &Frontend) {
    super::next::home::opened(fe);
}

impl Frontend {
    /// The new main menu's drawing, under its (invisible) rows.
    pub(super) fn paint_modern(&self, om: &OpenMenu, pl: &Placement, ops: &mut Vec<Op>) {
        super::next::home::paint(self, om, pl, ops);
        super::next::cac::paint_select(self, om, pl, ops);
        super::next::cac::paint_editor(self, om, pl, ops);
        super::next::picker::paint(self, om, pl, ops);
        super::next::camo_edit::paint(self, om, pl, ops);
        super::next::color::paint(self, om, pl, ops);
        super::next::character::paint(self, om, pl, ops);
        super::next::lobby::paint(self, om, pl, ops);
        super::next::maps::paint(self, om, pl, ops);
        super::next::modes::paint(self, om, pl, ops);
        super::next::popup::paint(self, om, pl, ops);
        super::next::drops::paint(self, om, pl, ops);
        super::next::settings::paint(self, om, pl, ops);
        super::next::match_menus::paint(self, om, pl, ops);
        super::next::profiles::paint(self, om, pl, ops);
    }
}

/// The background picture and the emblem, as UI materials (see
/// `UiAssets::material`).
pub(super) const BACKGROUND: &str = "modern:background";
pub(super) const EMBLEM_MATERIAL: &str = "modern:emblem";

/// The embedded pictures for those materials.
pub(super) fn picture(name: &str) -> Option<&'static [u8]> {
    match name {
        "modern:background" => Some(include_bytes!("../../assets/ui/menu_background.png")),
        "modern:emblem" => Some(include_bytes!("../../../launcher/assets/icon.png")),
        _ => None,
    }
}
