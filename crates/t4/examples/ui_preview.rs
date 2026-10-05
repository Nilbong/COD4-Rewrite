//! `cargo run --release -p t4 --example ui_preview -- <out dir>`: draw World
//! at War's HUD art, its fonts and its main menu to PNGs, to check what
//! `t4::ui` and `t4::hud` load. A rough static picture (no menu scripts or
//! expressions run); the game's menu interpreter does the real thing.

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use iw3::iwi::{Format, Iwi};
use iw3::menu::{Font, Item, Token, UiData};
use std::collections::HashMap;
use std::path::PathBuf;

const W: f32 = 1280.0;
const H: f32 = 720.0;
/// The map whose minimap is shown.
const MAP: &str = "mp_airfield";
/// HUD art drawn on the first page.
const HUD: &[&str] = &[
    "reticle_side_small",
    "reticle_center_cross",
    "damage_feedback",
    "hit_direction",
    "overlay_low_health",
    "scope_overlay_mp",
    "compass_map_mp_airfield",
    "minimap_background",
    "compassping_player",
    "compassping_friendly_mp",
    "compassping_enemy",
    "compass_radarline",
    "hud_icon_thompson",
    "hud_icon_kar98k",
    "killiconheadshot",
    "killiconmelee",
    "stance_stand",
    "stance_crouch",
    "stance_prone",
    "ammo_counter_bullet",
    "specialty_bulletdamage",
    "specialty_fastreload",
    "specialty_weapon_flamethrower",
    "hud_icon_artillery",
    "faction_128_american",
    "faction_128_japan",
    "faction_128_soviet",
    "faction_128_german",
    "rank_pvt1",
    "rank_prestige10",
    "waypoint_capture_a",
    "loadscreen_mp_airfield_ig",
];

#[derive(Resource)]
struct Data {
    ui: UiData,
    strings: HashMap<String, String>,
    /// Material name -> colour image name.
    materials: HashMap<String, String>,
    vfs: iw3::iwd::Vfs,
    out: PathBuf,
    page: usize,
    frames: u32,
    spawned: Vec<Entity>,
    images: HashMap<String, Option<(Handle<Image>, Vec2)>>,
}

fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "t4_ui".into()));
    std::fs::create_dir_all(&out)?;
    let install = t4::Install::locate()?;
    let mut ui = UiData::default();
    let mut materials = HashMap::new();
    for name in t4::ui::MP_ZONES.iter().chain(&[MAP]) {
        let zone = t4::zone::Zone::parse(&t4::fastfile::load(&install.zone_path(name))?, Default::default())?;
        let data = t4::ui::ui_data(&zone);
        ui.menus.extend(data.menus);
        ui.fonts.extend(data.fonts);
        ui.strings.extend(data.strings);
        let converted = t4::convert::to_iw3(&zone);
        for a in &converted.assets {
            let iw3::zone::Asset::Material(m) = a else { continue };
            let t = m.textures.iter().find(|t| t.semantic == iw3::zone::TextureSemantic::Color).or(m.textures.first());
            if let Some(img) = t.and_then(|t| converted.image(t.image?)) {
                materials.entry(m.name.trim_start_matches(',').to_owned()).or_insert_with(|| img.name.trim_start_matches(',').to_owned());
            }
        }
    }
    let strings = ui.strings.iter().map(|(k, v)| (k.to_ascii_uppercase(), v.clone())).collect();
    let vfs = install.vfs()?;
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window { title: "t4 ui".into(), resolution: (W as u32, H as u32).into(), ..default() }),
            ..default()
        }))
        .insert_resource(Data { ui, strings, materials, vfs, out, page: 0, frames: 0, spawned: Vec::new(), images: HashMap::new() })
        .insert_resource(ClearColor(Color::srgb(0.16, 0.17, 0.19)))
        .add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
        })
        .add_systems(Update, step)
        .run();
    Ok(())
}

const PAGES: [&str; 3] = ["hud", "fonts", "main_menu"];

fn step(mut commands: Commands, mut data: ResMut<Data>, mut images: ResMut<Assets<Image>>, mut exit: MessageWriter<AppExit>) {
    data.frames += 1;
    let wait = if data.page <= 1 { 120 } else { 30 };
    if data.page > 0 {
        if data.frames == wait {
            let path = data.out.join(format!("{}.png", PAGES[data.page - 1]));
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
        }
        if data.frames < wait + 10 {
            return;
        }
        for e in std::mem::take(&mut data.spawned) {
            commands.entity(e).despawn();
        }
    }
    let Some(page) = PAGES.get(data.page) else {
        exit.write(AppExit::Success);
        return;
    };
    data.page += 1;
    data.frames = 0;
    let mut d = Draw { commands: &mut commands, data: &mut data, images: &mut images, z: 0.0 };
    match *page {
        "hud" => d.hud(),
        "fonts" => d.fonts(),
        _ => d.main_menu(),
    }
}

struct Draw<'a, 'w, 's> {
    commands: &'a mut Commands<'w, 's>,
    data: &'a mut Data,
    images: &'a mut Assets<Image>,
    z: f32,
}

impl Draw<'_, '_, '_> {
    /// The image of a material, and its pixel size.
    fn material(&mut self, name: &str) -> Option<(Handle<Image>, Vec2)> {
        let image = self.data.materials.get(name).cloned().unwrap_or_else(|| name.to_owned());
        if let Some(cached) = self.data.images.get(&image) {
            return cached.clone();
        }
        let loaded = texture(&self.data.vfs, &image).map(|img| {
            let size = img.size().as_vec2();
            (self.images.add(img), size)
        });
        if loaded.is_none() {
            warn!("no image for {name} ({image})");
        }
        self.data.images.insert(image, loaded.clone());
        loaded
    }

    /// A textured quad with its top-left corner at `pos` (window pixels).
    fn quad(&mut self, image: Handle<Image>, pos: Vec2, size: Vec2, rect: Option<Rect>, color: Color) {
        self.z += 0.01;
        let tf = Transform::from_xyz(pos.x + size.x * 0.5 - W * 0.5, H * 0.5 - (pos.y + size.y * 0.5), self.z);
        let e = self.commands.spawn((Sprite { image, color, custom_size: Some(size), rect, ..default() }, tf)).id();
        self.data.spawned.push(e);
    }

    fn label(&mut self, text: &str, pos: Vec2, width: f32) {
        let e = self
            .commands
            .spawn((
                Text::new(text),
                TextFont { font_size: FontSize::Px(12.0), ..default() },
                TextColor(Color::srgb(1.0, 0.9, 0.4)),
                TextLayout::justify(Justify::Center),
                Node { position_type: PositionType::Absolute, left: px(pos.x), top: px(pos.y), width: px(width), ..default() },
            ))
            .id();
        self.data.spawned.push(e);
    }

    /// One line of text in a World at War font, baseline at `y`, `height`
    /// pixels tall.
    fn text(&mut self, font: &Font, text: &str, mut x: f32, y: f32, height: f32, color: Color) {
        let Some((image, size)) = self.material(&font.material) else { return };
        let k = height / font.pixel_height as f32;
        for c in text.chars() {
            let Some(g) = font.glyph(c) else {
                x += font.pixel_height as f32 * 0.3 * k;
                continue;
            };
            if g.pixel_width > 0 && g.pixel_height > 0 {
                let rect = Rect::new(g.s0 * size.x, g.t0 * size.y, g.s1 * size.x, g.t1 * size.y);
                let pos = Vec2::new(x + g.x0 as f32 * k, y + g.y0 as f32 * k);
                self.quad(image.clone(), pos, Vec2::new(g.pixel_width as f32, g.pixel_height as f32) * k, Some(rect), color);
            }
            x += g.dx as f32 * k;
        }
    }

    fn hud(&mut self) {
        let (cols, cell) = (8, Vec2::new(W / 8.0, 175.0));
        for (i, name) in HUD.iter().enumerate() {
            let at = Vec2::new((i % cols) as f32 * cell.x, (i / cols) as f32 * cell.y + 10.0);
            if let Some((image, size)) = self.material(name) {
                let fit = (Vec2::new(cell.x - 20.0, cell.y - 40.0) / size).min_element().min(4.0);
                let s = size * fit;
                self.quad(image, at + Vec2::new((cell.x - s.x) * 0.5, (cell.y - 40.0 - s.y) * 0.5), s, None, Color::WHITE);
            }
            self.label(name, at + Vec2::new(0.0, cell.y - 32.0), cell.x);
        }
    }

    fn fonts(&mut self) {
        let fonts = self.data.ui.fonts.clone();
        let mut y = 20.0;
        for f in &fonts {
            self.label(&f.name, Vec2::new(10.0, y), 220.0);
            let height = (f.pixel_height as f32 * 1.5).min(48.0);
            self.text(f, "CALL OF DUTY: WORLD AT WAR  0123456789 abcdefg", 240.0, y + height * 0.85, height, Color::WHITE);
            y += height + 28.0;
        }
    }

    fn main_menu(&mut self) {
        let Some(menu) = self.data.ui.menus.iter().find(|m| m.window.name == "main_text").cloned() else { return };
        let pl = Placement { scale: H / 480.0, sub_left: (W - 640.0 * H / 480.0) * 0.5 };
        for item in &menu.items {
            if !shown(item) {
                continue;
            }
            let (pos, size) = pl.rect(&item.window.rect);
            if let Some(bg) = item.window.background.clone() {
                if let Some((image, _)) = self.material(&bg) {
                    let c = item.window.fore_color;
                    let color = if c[3] > 0.0 { Color::srgba(c[0], c[1], c[2], c[3]) } else { Color::WHITE };
                    self.quad(image, pos, size, None, color);
                }
            }
            let text = match item.text_exp.as_slice() {
                [Token::Op(_), Token::Str(s)] => s.clone(),
                _ => item.text.clone(),
            };
            let text = match text.strip_prefix('@') {
                Some(key) => self.data.strings.get(&key.to_ascii_uppercase()).cloned().unwrap_or(text),
                None => text,
            };
            if text.is_empty() {
                continue;
            }
            // `textscale * 48` virtual units tall, as the game draws it.
            let height = item.text_scale * 48.0 * pl.scale;
            let font = font_for(&self.data.ui.fonts, item.font_enum, height);
            let x = pos.x + item.text_align_x * pl.scale;
            let y = pos.y + item.text_align_y * pl.scale + if item.text_align_mode & 12 == 0 { 0.0 } else { height };
            let c = item.window.fore_color;
            self.text(&font, &text, x, y, height, Color::srgba(c[0], c[1], c[2], c[3].max(0.2)));
        }
    }
}

/// Items always shown, or shown unless the back button is hidden (the
/// usual main menu state). Highlights and other dvar-driven items are not.
fn shown(item: &Item) -> bool {
    match item.visible_exp.as_slice() {
        [] => true,
        [Token::Op(16), Token::Op(7), Token::Op(59), Token::Str(s), Token::Op(1)] => s == "ui_hideBack",
        _ => false,
    }
}

fn font_for(fonts: &[Font], font_enum: i32, px: f32) -> Font {
    let name = match font_enum {
        2 => "fonts/bigFont",
        3 => "fonts/smallFont",
        4 => "fonts/boldFont",
        5 => "fonts/consoleFont",
        6 => "fonts/objectiveFont",
        _ if px <= 12.0 => "fonts/smallFont",
        _ if px <= 20.0 => "fonts/normalFont",
        _ if px <= 34.0 => "fonts/bigFont",
        _ => "fonts/extraBigFont",
    };
    fonts.iter().find(|f| f.name.eq_ignore_ascii_case(name)).or(fonts.first()).cloned().expect("fonts")
}

/// The game's `ScreenPlacement` (see `crates/game/src/ui/draw.rs`).
struct Placement {
    scale: f32,
    sub_left: f32,
}

impl Placement {
    fn rect(&self, r: &iw3::menu::Rect) -> (Vec2, Vec2) {
        let x = match r.horz_align {
            1 => r.x * self.scale,
            2 | 7 => W * 0.5 + r.x * self.scale,
            3 => W + r.x * self.scale,
            4 | 6 => r.x * W / 640.0,
            5 => r.x,
            _ => self.sub_left + r.x * self.scale,
        };
        let y = match r.vert_align {
            2 | 7 => H * 0.5 + r.y * self.scale,
            3 => H + r.y * self.scale,
            4 | 6 => r.y * H / 480.0,
            5 => r.y,
            _ => r.y * self.scale,
        };
        let sx = match r.horz_align {
            4 | 6 => W / 640.0,
            5 => 1.0,
            _ => self.scale,
        };
        let sy = match r.vert_align {
            4 | 6 => H / 480.0,
            5 => 1.0,
            _ => self.scale,
        };
        (Vec2::new(x, y), Vec2::new(r.w * sx, r.h * sy))
    }
}

fn texture(vfs: &iw3::iwd::Vfs, name: &str) -> Option<Image> {
    let data = vfs.read(&format!("images/{name}.iwi")).ok()??;
    let iwi = Iwi::parse(&data).ok()?;
    let size = Extent3d { width: iwi.width, height: iwi.height, depth_or_array_layers: 1 };
    let (format, bytes) = match iwi.format {
        Format::Dxt1 => (TextureFormat::Bc1RgbaUnormSrgb, iwi.levels[0].clone()),
        Format::Dxt3 => (TextureFormat::Bc2RgbaUnormSrgb, iwi.levels[0].clone()),
        Format::Dxt5 => (TextureFormat::Bc3RgbaUnormSrgb, iwi.levels[0].clone()),
        _ => (TextureFormat::Rgba8UnormSrgb, iwi.to_rgba8(0)?),
    };
    if iwi.format.is_compressed() && (iwi.width % 4 != 0 || iwi.height % 4 != 0) {
        return None;
    }
    let mut image = Image::new_uninit(size, TextureDimension::D2, format, RenderAssetUsages::RENDER_WORLD);
    image.data = Some(bytes);
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..default()
    });
    Some(image)
}
