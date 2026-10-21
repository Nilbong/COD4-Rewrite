//! The airstrike's laptop screen: the location map drawn into a picture
//! ([`crate::killstreaks::hands::SCREEN_IMAGE`]) that the laptop in the
//! player's hands shows on its lid, and picked from there with a cursor of
//! its own: while the laptop is open the mouse (and a pad's right stick)
//! move it about the screen instead of turning the view, a click (the
//! pad's trigger or A) calls the strike in under it and a right click (B)
//! puts the laptop away, as the HUD's map does otherwise
//! (`hud::pick_airstrike`, which stands down meanwhile).
//!
//! The picture is a small UI of its own, drawn by a camera of its own while
//! the laptop is in hand: the minimap's image, north up, the screen's
//! height (a dimmed copy stretched behind it fills the rest); friendlies
//! and the player; the cursor, with the strike's reach
//! (`map_artillery_selector`) round it.

use super::Frontend;
use super::hud::Minimap;
use crate::combat::{Dead, Pawn};
use crate::killstreaks::airstrike::Selecting;
use crate::killstreaks::hands::{HandsPhase, HardpointHands, is_laptop, set_laptop_open};
use crate::movement::ViewAngles;
use crate::player::LocalPlayer;
use crate::splitscreen::PlayerInput;
use avian3d::prelude::SpatialQuery;
use bevy::camera::RenderTarget;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

/// The picture (pixels) and the map's square in it.
const W: f32 = 640.0;
const H: f32 = 400.0;
const MAP: f32 = H;
const MAP_AT: Vec2 = Vec2::new((W - MAP) * 0.5, 0.0);
/// `beginLocationSelection( "map_artillery_selector", artilleryDangerMaxRadius * 1.2 )`.
const SELECTOR_RADIUS: f32 = 450.0 * 1.2;
const FRIENDLIES: usize = 12;

pub(super) fn build(app: &mut App) {
    let image = app.world_mut().resource_mut::<Assets<Image>>().add(Image::new_target_texture(W as u32, H as u32, TextureFormat::Rgba8UnormSrgb, None));
    crate::killstreaks::hands::SCREEN_IMAGE.set(image).ok();
    // (A match started without the menus has its `Frontend` a moment later.)
    app.init_resource::<LaptopCursor>()
        .add_systems(Update, (aim, screen, pick).chain().run_if(crate::state::in_game.and_then(resource_exists::<super::Frontend>)));
}

/// The cursor on the laptop's screen's map (picture pixels), while it's
/// open.
#[derive(Resource, Default)]
struct LaptopCursor(Option<Vec2>);

/// The screen's UI.
struct Scene {
    camera: Entity,
    map: Entity,
    player: Entity,
    friendlies: Vec<Entity>,
    selector: Entity,
    cross: [Entity; 2],
    backdrop: Entity,
    map_image: Option<Handle<Image>>,
}

fn accent() -> Color {
    use crate::tune::get;
    Color::srgb(get("airstrike_map.accent_red", 0.35), get("airstrike_map.accent_green", 0.9), get("airstrike_map.accent_blue", 0.95))
}

fn spawn_scene(commands: &mut Commands, image: Handle<Image>) -> Scene {
    let camera = commands
        .spawn((
            Name::new("laptop screen camera"),
            Camera2d,
            Camera { order: -30, is_active: false, clear_color: ClearColorConfig::Custom(Color::srgb(0.02, 0.03, 0.035)), ..default() },
            RenderTarget::Image(image.into()),
        ))
        .id();
    let abs = |x: f32, y: f32, w: f32, h: f32| Node { position_type: PositionType::Absolute, left: px(x), top: px(y), width: px(w), height: px(h), ..default() };
    let root = commands
        .spawn((Name::new("laptop screen"), Node { width: px(W), height: px(H), ..default() }, BackgroundColor(Color::srgb(0.02, 0.03, 0.035)), UiTargetCamera(camera)))
        .id();
    let text = |commands: &mut Commands, s: &str, x: f32, y: f32, size: f32, color: Color| {
        commands.spawn((Text::new(s), TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }, TextColor(color), Node { position_type: PositionType::Absolute, left: px(x), top: px(y), ..default() }, ChildOf(root)));
    };
    // The map, dimmed and stretched, behind it all.
    let backdrop = commands.spawn((ImageNode { color: Color::srgb(0.22, 0.26, 0.28), ..default() }, abs(0.0, 0.0, W, H), ChildOf(root))).id();
    let map = commands
        .spawn((
            ImageNode::default(),
            Node { border: UiRect::horizontal(px(2.0)), ..abs(MAP_AT.x - 2.0, MAP_AT.y, MAP + 4.0, MAP) },
            BorderColor::all(accent()),
            ChildOf(root),
        ))
        .id();
    // The words in the margins either side of the map.
    text(commands, "AIRSTRIKE", 10.0, 12.0, 22.0, Color::WHITE);
    text(commands, "SELECT\nLOCATION", 10.0, 40.0, 14.0, accent());
    text(commands, "CLICK\ncall in", W - 110.0, H - 96.0, 14.0, Color::srgba(0.85, 0.85, 0.85, 0.95));
    text(commands, "RIGHT CLICK\ncancel", W - 110.0, H - 52.0, 14.0, Color::srgba(0.85, 0.85, 0.85, 0.95));
    let icon = |commands: &mut Commands, size: f32| commands.spawn((ImageNode::default(), abs(0.0, 0.0, size, size), UiTransform::default(), Visibility::Hidden, ChildOf(root))).id();
    let friendlies = (0..FRIENDLIES).map(|_| icon(commands, 16.0)).collect();
    let player = icon(commands, 20.0);
    let selector = icon(commands, 40.0);
    // The cursor: a cross, white with a dark edge.
    let bar = |commands: &mut Commands, w: f32, h: f32| {
        commands
            .spawn((abs(0.0, 0.0, w, h), BackgroundColor(Color::WHITE), Outline::new(px(1.0), px(0.0), Color::srgba(0.0, 0.0, 0.0, 0.8)), Visibility::Hidden, ChildOf(root)))
            .id()
    };
    let cross = [bar(commands, CROSS, 2.0), bar(commands, 2.0, CROSS)];
    Scene { camera, map, player, friendlies, selector, cross, backdrop, map_image: None }
}

/// The laptop is in hand (coming up, held, going away), or on its way (the
/// gun going down first): the HUD's map stays down meanwhile, rather than
/// showing for a moment.
fn laptop_in_hand(hands: &Query<&HardpointHands, With<LocalPlayer>>) -> bool {
    hands.iter().any(|h| is_laptop(h.def) && matches!(h.phase, HandsPhase::GunDown | HandsPhase::Raise | HandsPhase::Hold | HandsPhase::Press | HandsPhase::PutAway))
}

/// The cursor's cross (pixels across).
const CROSS: f32 = 22.0;

/// The mouse and the right stick move the cursor about the screen (from
/// its middle when the laptop opens).
fn aim(
    time: Res<Time>,
    motion: Res<AccumulatedMouseMotion>,
    inputs: Query<&PlayerInput, With<LocalPlayer>>,
    mut cursor: ResMut<LaptopCursor>,
) {
    if !crate::killstreaks::hands::laptop_open() {
        cursor.0 = None;
        return;
    }
    let c = cursor.0.unwrap_or(Vec2::new(W, H) * 0.5);
    let input = inputs.iter().next();
    // (Mouse counts to pixels by the game's sensitivity; the stick, at most
    // the screen's height a second.)
    let mouse = if input.is_some_and(|i| i.live) { motion.delta * crate::tune::get("laptop.mouse", 0.5) * crate::settings_apply::mouse_scale(0.0) } else { Vec2::ZERO };
    let stick = input.map_or(Vec2::ZERO, |i| i.pad.look) * Vec2::new(1.0, -1.0) * crate::tune::get("laptop.stick", 1.0) * H * time.delta_secs();
    // Kept on the map, so the strike's reach always shows round it (in the
    // margins beside the map it was a lone cross over the words, at the
    // screen's edge half off it; a jump of the mouse, coming back to the
    // window, put it there).
    cursor.0 = Some((c + mouse + stick).clamp(MAP_AT, MAP_AT + MAP));
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn screen(
    mut commands: Commands,
    mut fe: ResMut<Frontend>,
    mut images: ResMut<Assets<Image>>,
    minimap: Option<Res<Minimap>>,
    selecting: Option<Res<Selecting>>,
    hands: Query<&HardpointHands, With<LocalPlayer>>,
    me: Query<(&Pawn, &Transform, &ViewAngles), With<LocalPlayer>>,
    pawns: Query<(&Pawn, &Transform, &ViewAngles, Has<Dead>), Without<LocalPlayer>>,
    cursor: Res<LaptopCursor>,
    mut scene: Local<Option<Scene>>,
    mut cameras: Query<&mut Camera>,
    mut nodes: Query<(&mut Node, &mut Visibility, Option<&mut UiTransform>, Option<&mut ImageNode>)>,
) {
    let active = laptop_in_hand(&hands) && minimap.is_some();
    set_laptop_open(active && selecting.is_some());
    let Some(image) = crate::killstreaks::hands::SCREEN_IMAGE.get() else { return };
    // The match's end clears its entities, the screen's camera and UI with
    // them: the next match's laptop builds its own (else it showed the last
    // one's last picture, its map and a cursor that no longer moved).
    if scene.as_ref().is_some_and(|s| cameras.get(s.camera).is_err()) {
        *scene = None;
    }
    if scene.is_none() {
        if !active {
            return;
        }
        *scene = Some(spawn_scene(&mut commands, image.clone()));
        info!("laptop screen: built");
        return;
    }
    let Some(s) = scene.as_mut() else { return };
    if let Ok(mut c) = cameras.get_mut(s.camera) {
        if c.is_active != active {
            c.is_active = active;
        }
    }
    let (Some(m), true) = (minimap, active) else { return };
    let mut picture = |name: &str| fe.assets.material(name, &mut images).map(|i| i.handle);
    if s.map_image.is_none() {
        s.map_image = picture(m.material_name());
        for e in [s.map, s.backdrop] {
            if let (Some(h), Ok((_, _, _, Some(mut img)))) = (s.map_image.clone(), nodes.get_mut(e)) {
                img.image = h;
            }
        }
    }
    let north = m.north_angle();
    let flat = |v: Vec3| {
        let c = crate::units::to_cod(v);
        Vec2::new(c[0], c[1])
    };
    let facing = |yaw: f32| {
        let cod = yaw + std::f32::consts::FRAC_PI_2;
        Vec2::new(cod.cos(), cod.sin())
    };
    // An icon at a pawn's place, turned to its facing (or hidden).
    let place = |nodes: &mut Query<(&mut Node, &mut Visibility, Option<&mut UiTransform>, Option<&mut ImageNode>)>, e: Entity, at: Option<(Vec3, f32)>, size: f32, pic: Option<Handle<Image>>| {
        let Ok((mut node, mut vis, turn, img)) = nodes.get_mut(e) else { return };
        let Some((pos, yaw)) = at else {
            vis.set_if_neq(Visibility::Hidden);
            return;
        };
        let uv = m.uv(flat(pos));
        let c = MAP_AT + uv * MAP;
        node.left = px(c.x - size * 0.5);
        node.top = px(c.y - size * 0.5);
        let f = facing(yaw);
        if let Some(mut t) = turn {
            t.rotation = Rot2::radians(north - f.y.atan2(f.x));
        }
        if let (Some(mut img), Some(pic)) = (img, pic) {
            if img.image != pic {
                img.image = pic;
            }
        }
        vis.set_if_neq(Visibility::Inherited);
    };
    let friendly_pic = picture("cod4rw_mmicon_friendly_100_100").or_else(|| picture("compassping_friendly"));
    let player_pic = picture("cod4rw_mmicon_player_100_100").or_else(|| picture("compassping_player"));
    let me = me.iter().next();
    let team = me.map(|(p, ..)| p.team);
    let mut friends = pawns.iter().filter(|(p, _, _, dead)| Some(p.team) == team && !crate::combat::free_for_all() && !dead);
    for &e in &s.friendlies {
        let at = friends.next().map(|(_, tf, view, _)| (tf.translation, view.yaw));
        place(&mut nodes, e, at, 16.0, friendly_pic.clone());
    }
    place(&mut nodes, s.player, me.map(|(_, tf, view)| (tf.translation, view.yaw)), 20.0, player_pic);
    // The strike's reach round the cursor, on the map.
    let on_map = cursor.0.filter(|c| c.cmpge(MAP_AT).all() && c.cmple(MAP_AT + MAP).all());
    let r = SELECTOR_RADIUS / m.width().max(1.0) * MAP;
    let selector_pic = picture("map_artillery_selector");
    if let Ok((mut node, mut vis, _, img)) = nodes.get_mut(s.selector) {
        match on_map {
            Some(c) => {
                node.left = px(c.x - r);
                node.top = px(c.y - r);
                node.width = px(2.0 * r);
                node.height = px(2.0 * r);
                if let (Some(mut img), Some(pic)) = (img, selector_pic) {
                    if img.image != pic {
                        img.image = pic;
                    }
                }
                vis.set_if_neq(Visibility::Inherited);
            }
            None => {
                vis.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for e in s.cross {
        let Ok((mut node, mut vis, _, _)) = nodes.get_mut(e) else { continue };
        match cursor.0 {
            Some(c) => {
                let (Val::Px(w), Val::Px(h)) = (node.width, node.height) else { continue };
                node.left = px(c.x - w * 0.5);
                node.top = px(c.y - h * 0.5);
                vis.set_if_neq(Visibility::Inherited);
            }
            None => {
                vis.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

/// A click (the pad's trigger or A) calls the strike in under the cursor; a
/// right click (B) puts the laptop away.
#[allow(clippy::too_many_arguments)]
fn pick(
    mut commands: Commands,
    selecting: Option<Res<Selecting>>,
    mouse: Res<ButtonInput<MouseButton>>,
    players: Query<&PlayerInput, With<LocalPlayer>>,
    pads: Query<&Gamepad>,
    minimap: Option<Res<Minimap>>,
    cursor: Res<LaptopCursor>,
    spatial: SpatialQuery,
    mut inputs: Query<(&mut crate::killstreaks::HardpointInput, &Transform)>,
) {
    let (Some(sel), Some(m), true) = (selecting, minimap, crate::killstreaks::hands::laptop_open()) else { return };
    let player = players.iter().next();
    let pad = player.and_then(|p| p.pad_entity).and_then(|e| pads.get(e).ok());
    let pressed = |b: MouseButton, g: GamepadButton| mouse.just_pressed(b) || player.is_some_and(|p| p.mouse.just_pressed(b)) || pad.is_some_and(|p| p.just_pressed(g));
    if pressed(MouseButton::Right, GamepadButton::East) {
        commands.remove_resource::<Selecting>();
        return;
    }
    let confirm = pressed(MouseButton::Left, GamepadButton::South) || pad.is_some_and(|p| p.just_pressed(GamepadButton::RightTrigger2));
    let Some(c) = cursor.0.filter(|_| confirm) else { return };
    // A click off the map (on the screen round it) is taken to its edge.
    let uv = ((c - MAP_AT) / MAP).clamp(Vec2::ZERO, Vec2::ONE);
    let xy = m.world_at(uv);
    if let Ok((mut input, tf)) = inputs.get_mut(sel.owner) {
        input.use_now = true;
        input.item = Some(crate::killstreaks::Hardpoint::Airstrike);
        input.target = Some(crate::killstreaks::airstrike::ground_below(&spatial, crate::units::pos([xy.x, xy.y, 0.0]), tf.translation.y));
    }
    commands.remove_resource::<Selecting>();
}
