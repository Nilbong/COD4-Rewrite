//! A lost GPU device (the driver reset, or the GPU ran out of memory) ends the
//! game with one clear line in the log, `GPU device lost: <reason>`, and in
//! normal play a message box saying so, rather than a wall of panics.
//!
//! Bevy notices the loss a frame late: by then its GPU readbacks (SSAO's
//! thickness buffer, render statistics) have already panicked on their
//! worker threads mapping buffers that died with the device, which ends the
//! game before Bevy's handler runs. So the first of those follow-on panics
//! ([`follow_on`]) is taken as the loss: one log line, the message box, and
//! a clean exit ([`first_loss`], [`lost_exit`]).

use bevy::prelude::*;
use bevy::render::error_handler::{ErrorType, RenderError, RenderErrorHandler, RenderErrorPolicy};

pub struct DeviceLostPlugin;

impl Plugin for DeviceLostPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(RenderErrorHandler(on_render_error));
        // Test aid: `COD4RW_LOSEDEVICE=<seconds>` destroys the GPU device
        // that long after rendering starts, to check what a loss does.
        if let Some(secs) = std::env::var("COD4RW_LOSEDEVICE").ok().and_then(|v| v.parse::<f32>().ok()) {
            if let Some(render) = app.get_sub_app_mut(bevy::render::RenderApp) {
                render.add_systems(bevy::render::Render, move |device: Res<bevy::render::renderer::RenderDevice>, mut since: Local<Option<std::time::Instant>>, mut done: Local<bool>| {
                    let since = *since.get_or_insert_with(std::time::Instant::now);
                    if !*done && since.elapsed().as_secs_f32() >= secs {
                        *done = true;
                        warn!("COD4RW_LOSEDEVICE: destroying the GPU device");
                        device.wgpu_device().destroy();
                    }
                });
            }
        }
    }
}

/// As Bevy's own handler (quit on any render error), with the device loss
/// said plainly.
fn on_render_error(error: &RenderError, main_world: &mut World, _render_world: &mut World) -> RenderErrorPolicy {
    if error.ty == ErrorType::DeviceLost {
        if first_loss() {
            error!("GPU device lost: {}", error.description);
            if !crate::audio::silent_run() {
                tell_player();
            }
        }
    } else if error.ty == ErrorType::Validation && validation_tolerated() {
        // One bad frame (a view's textures out of step for a frame while a
        // setting changes them: the player's game closed in the settings,
        // 2026-10-14) is dropped and play goes on; only errors that keep
        // coming end the game.
        warn!("render error, frame dropped: {}", error.description);
        return RenderErrorPolicy::Ignore;
    } else {
        error!("Quitting: {:?} render error: {}", error.ty, error.description);
    }
    main_world.write_message(AppExit::error());
    RenderErrorPolicy::StopRendering
}

/// Validation errors tolerated: at most [`TOLERATED`] in any
/// [`TOLERATED_SECS`] seconds.
const TOLERATED: usize = 3;
const TOLERATED_SECS: f32 = 5.0;

fn validation_tolerated() -> bool {
    static RECENT: std::sync::Mutex<Vec<std::time::Instant>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut recent) = RECENT.lock() else { return false };
    let now = std::time::Instant::now();
    recent.retain(|t| now.duration_since(*t).as_secs_f32() < TOLERATED_SECS);
    recent.push(now);
    recent.len() <= TOLERATED
}

/// A panic that comes from using GPU buffers after the device was lost.
pub fn follow_on(message: &str) -> bool {
    message.contains("get_mapped_range") && (message.contains("Validation Error") || message.contains("is invalid"))
}

/// From the panic hook, on the first [`follow_on`] panic (true only for the
/// first, whichever thread): these come a frame before Bevy's render error
/// handler would see the loss, and take the game down on their own, so the
/// hook ends it there, once, as the handler would ([`lost_exit`]).
pub fn first_loss() -> bool {
    static ONCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    !ONCE.swap(true, std::sync::atomic::Ordering::SeqCst)
}

/// Tell the player (normal play only) and exit.
pub fn lost_exit() -> ! {
    if !crate::audio::silent_run() {
        tell_player();
    }
    std::process::exit(1)
}

fn tell_player() {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
        }
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let text = wide(
            "The graphics device was lost (the GPU driver reset, or ran out of memory), so the game has to close.\n\n\
             Closing other games and programs that use the GPU can help. Details are in cod4rw.log next to the game.",
        );
        let caption = wide("CoD4 Rewrite");
        // MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST
        unsafe { MessageBoxW(0, text.as_ptr(), caption.as_ptr(), 0x10 | 0x1_0000 | 0x4_0000) };
    }
}
