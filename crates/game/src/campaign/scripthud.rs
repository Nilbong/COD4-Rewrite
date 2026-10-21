//! The level scripts' HUD elements (`newhudelem`) on screen: the Coup's
//! black-outs and pulsing fades, its credits typing on, subtitles. Placed
//! as CoD places them: x/y on a 640x480 virtual screen, scaled by the
//! height, anchored by `horzalign`/`vertalign` and aligned on themselves
//! by `alignx`/`aligny`; text height 12 x `fontscale` virtual pixels.

use bevy::prelude::*;
use gsc::level::HudDraw;

pub(super) fn build(app: &mut App) {
    app.init_resource::<Pool>()
        .add_systems(Update, draw.after(super::script::Ticked).run_if(super::active))
        .add_systems(OnExit(crate::state::GameState::InGame), |mut commands: Commands, mut pool: ResMut<Pool>| {
            for e in pool.0.drain(..) {
                commands.entity(e).try_despawn();
            }
        });
}

#[derive(Resource, Default)]
struct Pool(Vec<Entity>);

#[derive(Component)]
struct ScriptHud {
    /// The text its spans were made for.
    shown: String,
    /// The material its picture shows (`setshader`), if one.
    image: Option<String>,
}

/// Text after a `^n` colour code: (colour index or None for the
/// element's own, text).
fn colour_runs(text: &str) -> Vec<(Option<u8>, String)> {
    let mut out: Vec<(Option<u8>, String)> = vec![(None, String::new())];
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^'
            && let Some(d) = chars.peek().and_then(|d| d.to_digit(10))
        {
            chars.next();
            out.push(((d != 7).then_some(d as u8), String::new()));
            continue;
        }
        out.last_mut().unwrap().1.push(c);
    }
    out.retain(|(_, t)| !t.is_empty());
    out
}

/// CoD's `^n` colours.
fn code_colour(n: u8, alpha: f32) -> Color {
    let [r, g, b] = match n {
        0 => [0.0, 0.0, 0.0],
        1 => [1.0, 0.36, 0.36],
        2 => [0.0, 1.0, 0.0],
        3 => [1.0, 1.0, 0.0],
        4 => [0.0, 0.0, 1.0],
        5 => [0.0, 1.0, 1.0],
        6 => [1.0, 0.36, 1.0],
        _ => [1.0, 1.0, 1.0],
    };
    Color::srgba(r, g, b, alpha)
}

/// Where a virtual x (or y) lands on a screen `size` wide (tall), by the
/// element's screen alignment; and the scale of its sizes.
fn place(v: f32, align: &str, size: f32, other: f32, horizontal: bool) -> (f32, f32) {
    let s = if horizontal { other / 480.0 } else { size / 480.0 };
    let sub = if horizontal { (size - 640.0 * s) * 0.5 } else { 0.0 };
    match align {
        "fullscreen" => {
            let k = size / if horizontal { 640.0 } else { 480.0 };
            (v * k, k)
        }
        "noscale" => (v, 1.0),
        "left" | "top" => (v * s, s),
        "center" | "middle" => (size * 0.5 + v * s, s),
        "right" | "bottom" => (size + v * s, s),
        "center_safearea" => (size * 0.5 + v * s, s),
        // subleft/subtop and the rest: the 4:3 middle of the screen.
        _ => (sub + v * s, s),
    }
}

/// A span's `^n` colour (None: the element's).
#[derive(Component)]
struct SpanCode(Option<u8>);

fn draw(
    mut commands: Commands,
    script: NonSend<super::script::Script>,
    mut content: Option<ResMut<crate::content::Content>>,
    mut images: ResMut<Assets<Image>>,
    mut pictures: Local<std::collections::HashMap<String, Option<Handle<Image>>>>,
    window: Single<&Window, With<bevy::window::PrimaryWindow>>,
    mut pool: ResMut<Pool>,
    mut q: Query<(&mut ScriptHud, &mut Node, &mut BackgroundColor, &mut Text, &mut TextFont, &mut TextColor, &mut TextShadow, &mut GlobalZIndex, &mut Visibility, &ComputedNode, Option<&Children>)>,
    mut spans: Query<(&mut TextFont, &mut TextColor, &SpanCode), (With<TextSpan>, Without<ScriptHud>)>,
) {
    let elems: Vec<HudDraw> = script.hud();
    while pool.0.len() < elems.len() {
        let e = commands
            .spawn((
                ScriptHud { shown: String::new(), image: None },
                Node { position_type: PositionType::Absolute, ..default() },
                BackgroundColor(Color::NONE),
                Text::new(""),
                // (CoD's HUD text never wraps: a right-aligned block at the
                // screen's edge would otherwise be squeezed to a letter a
                // line, as the AC-130's was.)
                TextLayout::no_wrap(),
                TextFont::default(),
                TextColor(Color::WHITE),
                TextShadow::default(),
                GlobalZIndex(100),
                Visibility::Hidden,
            ))
            .id();
        pool.0.push(e);
    }
    let (w, h) = (window.width(), window.height());
    for (i, &e) in pool.0.iter().enumerate() {
        let Ok((mut hud, mut node, mut bg, mut text, mut font, mut color, mut shadow, mut z, mut vis, computed, children)) = q.get_mut(e) else { continue };
        let Some(d) = elems.get(i) else {
            vis.set_if_neq(Visibility::Hidden);
            continue;
        };
        let (x, sx) = place(d.x, &d.screen_align.0, w, h, true);
        let (y, sy) = place(d.y, &d.screen_align.1, h, w, false);
        let c = |a: f32| Color::srgba(d.color[0], d.color[1], d.color[2], a);
        let (bw, bh) = match &d.shader {
            Some((m, sw, sh)) => {
                // Plain black and white fill; other materials show their
                // picture, tinted by the element's colour.
                let tint = match m.as_str() {
                    "black" => Color::srgba(0.0, 0.0, 0.0, d.alpha),
                    "white" => c(d.alpha),
                    _ => Color::NONE,
                };
                bg.0 = tint;
                let picture = (!matches!(m.as_str(), "black" | "white"))
                    .then(|| {
                        pictures
                            .entry(m.clone())
                            .or_insert_with(|| content.as_deref_mut().and_then(|ct| ct.material_image(m, &mut images)))
                            .clone()
                    })
                    .flatten();
                match picture {
                    Some(h) => {
                        commands.entity(e).insert(ImageNode::new(h).with_color(c(d.alpha)));
                        hud.image = Some(m.clone());
                    }
                    None if hud.image.is_some() => {
                        commands.entity(e).remove::<ImageNode>();
                        hud.image = None;
                    }
                    None => {}
                }
                node.width = Val::Px(sw * sx);
                node.height = Val::Px(sh * sy);
                (sw * sx, sh * sy)
            }
            None => {
                bg.0 = Color::NONE;
                if hud.image.take().is_some() {
                    commands.entity(e).remove::<ImageNode>();
                }
                node.width = Val::Auto;
                node.height = Val::Auto;
                let size = computed.size() * computed.inverse_scale_factor();
                (size.x, size.y)
            }
        };
        let words = if d.text.is_empty() || d.shader.is_some() {
            String::new()
        } else {
            let full = directives(&content.as_deref().map_or(d.text.clone(), |ct| super::hud::localize(ct, &d.text, &d.text)), script.level_time());
            match d.letters {
                Some(n) => full.chars().take(n).collect(),
                None => full,
            }
        };
        let size = FontSize::Px(12.0 * d.font_scale * sy);
        // Colour codes: the first run is the text, the rest spans.
        if hud.shown != words {
            let runs = colour_runs(&words);
            if let Some(ch) = children {
                for &k in ch {
                    commands.entity(k).try_despawn();
                }
            }
            let (first, rest) = match runs.split_first() {
                Some(((code, t), rest)) if code.is_none() => (t.clone(), rest.to_vec()),
                _ => (String::new(), runs.clone()),
            };
            text.0 = first;
            for (code, t) in rest {
                commands.spawn((TextSpan::new(t), TextFont { font_size: size, ..default() }, TextColor(Color::WHITE), SpanCode(code), ChildOf(e)));
            }
            hud.shown = words;
        }
        font.font_size = size;
        color.0 = c(d.alpha);
        if let Some(ch) = children {
            for &k in ch {
                if let Ok((mut f, mut col, code)) = spans.get_mut(k) {
                    f.font_size = size;
                    col.0 = code.0.map_or(c(d.alpha), |n| code_colour(n, d.alpha));
                }
            }
        }
        shadow.color = match d.glow {
            Some((g, a)) => Color::srgba(g[0], g[1], g[2], a * d.alpha * 0.8),
            None => Color::srgba(0.0, 0.0, 0.0, 0.8 * d.alpha),
        };
        let ax = match d.align.0.as_str() {
            "center" => 0.5,
            "right" => 1.0,
            _ => 0.0,
        };
        let ay = match d.align.1.as_str() {
            "middle" => 0.5,
            "bottom" => 1.0,
            _ => 0.0,
        };
        node.left = Val::Px(x - bw * ax);
        node.top = Val::Px(y - bh * ay);
        z.0 = 100 + i as i32;
        vis.set_if_neq(if d.alpha > 0.0 { Visibility::Inherited } else { Visibility::Hidden });
    }
}

/// A string's `[{...}]` directives filled in: a key's binding
/// (`[{+activate}]`), or `[{FAKE_INTRO_SECONDS:nn}]`, the intro's clock
/// ticking from nn with the level's time.
fn directives(text: &str, time: f64) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("[{") {
        let Some(j) = rest[i..].find("}]") else { break };
        out.push_str(&rest[..i]);
        let d = &rest[i + 2..i + j];
        let (name, arg) = d.split_once(':').unwrap_or((d, ""));
        if name.eq_ignore_ascii_case("FAKE_INTRO_SECONDS") {
            let n: u32 = arg.trim().parse().ok().filter(|n| *n <= 40).unwrap_or(0);
            out.push_str(&format!("{:02}", n + time as u32));
        } else {
            out.push_str(match name.to_ascii_lowercase().as_str() {
                "+activate" | "+usereload" => "F",
                "+attack" => "MOUSE1",
                "+speed_throw" | "+toggleads_throw" | "+speed" | "toggleads" => "MOUSE2",
                "+melee" => "V",
                "+gostand" => "SPACE",
                "+frag" => "G",
                "+smoke" => "4",
                "+reload" => "R",
                "+stance" | "togglecrouch" | "+movedown" => "C",
                "toggleprone" | "+prone" => "CTRL",
                "+breath_sprint" | "+sprint" => "SHIFT",
                "+holdbreath" => "SHIFT",
                "weapnext" => "1",
                "+actionslot 1" => "5",
                "+actionslot 2" => "6",
                "+actionslot 3" => "7",
                "+actionslot 4" => "8",
                other => other,
            });
        }
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    out
}
