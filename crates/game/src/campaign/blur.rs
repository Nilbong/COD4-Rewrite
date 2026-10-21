//! The view's blur as the level's scripts set it: `setblur(radius, time)`
//! and the player's shellshocks (their `shock/<name>.shock`: a "blurred"
//! screen blended in, held, faded out at the end). Drawn by the world
//! camera's depth of field (`crate::ui::scope3d`'s blur takes the larger).

use std::sync::Mutex;
use std::time::Instant;

struct State {
    /// `setblur`: from, to, since, over (seconds).
    set: (f32, f32, Instant, f32),
    /// A shellshock: since, how long, blend in, fade out (seconds).
    shock: Option<(Instant, f32, f32, f32)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// How strong a shellshock's blur is (pixels at 640x480).
const SHOCK_PIXELS: f32 = 6.0;

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(|| State { set: (0.0, 0.0, Instant::now(), 0.0), shock: None });
    f(s)
}

/// `setblur(radius, time)`.
pub fn set(radius: f32, time: f32) {
    let now = with(|s| current_set(s));
    with(|s| s.set = (now, radius.max(0.0), Instant::now(), time.max(0.0)));
}

/// A shellshock (its `.shock` text, if the level has it); none for 0 s.
pub fn shock(duration: f32, shock_file: Option<&str>) {
    let num = |key: &str, default: f32| {
        shock_file
            .and_then(|t| {
                let mut it = t.split('\\');
                while let Some(k) = it.next() {
                    let v = it.next()?;
                    if k.trim().eq_ignore_ascii_case(key) {
                        return v.trim().trim_matches('"').parse().ok();
                    }
                }
                None
            })
            .unwrap_or(default)
    };
    let blurred = shock_file.is_none_or(|t| t.contains("\"blurred\""));
    with(|s| {
        s.shock = (duration > 0.0 && blurred).then(|| (Instant::now(), duration, num("bg_shock_screenBlurBlendTime", 0.4), num("bg_shock_screenBlurBlendFadeTime", 1.0)));
    });
}

fn current_set(s: &State) -> f32 {
    let (from, to, at, over) = s.set;
    if over <= 0.0 {
        return to;
    }
    let k = (at.elapsed().as_secs_f32() / over).clamp(0.0, 1.0);
    from + (to - from) * k
}

/// The blur now (pixels at 640x480).
pub fn pixels() -> f32 {
    if !super::active() {
        return 0.0;
    }
    with(|s| {
        let shock = s.shock.map_or(0.0, |(at, len, blend, fade)| {
            let t = at.elapsed().as_secs_f32();
            let inn = if blend > 0.0 { (t / blend).min(1.0) } else { 1.0 };
            let out = if fade > 0.0 { ((len - t) / fade).clamp(0.0, 1.0) } else { f32::from(t < len) };
            SHOCK_PIXELS * inn * out
        });
        current_set(s).max(shock)
    })
}

/// A new level: none.
pub fn reset() {
    with(|s| {
        s.set = (0.0, 0.0, Instant::now(), 0.0);
        s.shock = None;
    });
}
