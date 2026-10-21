# Android build (proof of concept)

The same game, built natively for arm64 Android (Vulkan), as one
NativeActivity. Target device: AYN Odin 2 Pro (Snapdragon 8 Gen 2 / Adreno
740, Android 13).

## How it's put together

- `Cargo.toml` here is its own workspace (nothing in it touches the Windows
  build). Its library is `crates/game/src/main.rs` compiled as `libcod4rw.so`;
  `Cargo.lock` is a copy of the root one, so versions match.
- `crates/game/src/android.rs` (compiled only for Android): `android_main`,
  the folders the game reads, `env.txt`, and the built-in pad.
- `patches/winit`: winit 0.30.13 with two changes. Cursor grab and position
  succeed as no-ops (an error made Bevy undo the game's grab every frame,
  and the game reads an ungrabbed cursor as "not playing"). And a hook
  (`GAMEPAD_HOOK`): stock winit
  turns a pad's stick motion into touches. The game takes pad events and
  feeds them to Bevy as a gamepad, as `gamepad/sony.rs` does for HID pads.
- `AndroidManifest.xml`, `res/`: the APK. No Java (`hasCode=false`), so no
  Gradle: `build.py` links it with aapt2, adds the .so uncompressed,
  zipaligns and signs it with `debug.keystore` (made on first build).

## Building

Tools (all in `D:\android-sdk`): NDK r27d, build-tools 34.0.0, platform
android-34, platform-tools (adb). Rust target: `rustup target add
aarch64-linux-android`. Paths are in `.cargo/config.toml`.

```bash
python android/build.py
```

Output: `android/out/cod4rw.apk`. The build itself goes to
`target/android/`.

## Running on the Odin 2

Once: Settings > About > tap "Build number" 7 times; Settings > System >
Developer options > USB debugging on; plug in USB and allow the PC.

```bash
python android/run.py
```

`run.py` installs `out/cod4rw.apk`, lets the app read shared storage
(`appops ... MANAGE_EXTERNAL_STORAGE allow`), waits until the device is
unlocked, starts the game, watches it for 120 s, then saves logcat, the
game's `cod4rw.log` and a screenshot in `target/android/runs/<time>/` and
prints the fps lines. `--match [seconds]` starts a timed Killhouse TDM with
bots instead of the menus (`COD4RW_PERF=1` in `env.txt`). `--map NAME` and
`--bots N` (a team) pass the game's own arguments (`GAME_ARGS` in `env.txt`:
Android has no command line), `--env K=V` adds any `env.txt` line. The peak
memory (PSS from `dumpsys meminfo`, every 10 s) is printed too.

## Where the game looks for CoD4

1. `COD4_PATH` (from `env.txt`), else
2. the app's own folder `/sdcard/Android/data/com.cod4rewrite.game/files/cod4/`
   (`main/` and `zone/`; no permission needed), else
3. a copy on shared storage: `Download/Call of Duty 4`, `Call of Duty 4`,
   `Games/Call of Duty 4`, `Download/cod4` or `cod4` (needs "All files
   access": Settings > Apps > CoD4 Rewrite > Permissions, or `run.py` grants
   it over adb).

If none has `main/` and `zone/`, the game shows where to copy them.
Black Ops, MW2 and World at War go in `bo1/`, `mw2/`, `waw/` beside the
app's `cod4/` folder, optional.

## Android specifics in the code

- Textures: Adreno has no BC (DXT) formats, so `textures.rs` decodes CoD4's
  DXT images to RGBA8 on Android (`android::decode_dxt`). 4x/8x the memory
  of BC (ETC2 below cuts the opaque ones back).
- Allocator: mimalloc (`local_dynamic_tls`, as the game is dlopen'd).
  Android's scudo caps each small size class at 256 MB and the menus' load
  ran out ("memory allocation of 280 bytes failed").
- Input watchdog: NativeActivity delivers input on the main loop's thread;
  a map load (5 s) or the first frames' shaders stalled it past Android's
  5 s input timeout ("not responding"). While the main loop stalls for over
  a second, a thread takes the events (pad input kept, the rest dropped).
- The screen stays on while the game is in front (KEEP_SCREEN_ON).
- Mobile settings defaults: `android::MOBILE_DEFAULTS`.
- Textures over 1024 a side drop their top mip levels (`TEXTURE_MAX_SIZE`
  in `env.txt`, e.g. 512 or 4096 to compare).
- Opaque textures are ETC2 (`android/etc2.rs`, checked against Google's
  etc1tool decoder: it matches or beats etc1tool's own encoder), the rest
  RGBA8. Killhouse on the Odin 2: 30 fps with RGBA8, 37 with ETC2 (world
  pass 24.9 to 17.4 ms), map load 7.3 to 10.3 s. `TEXTURE_FORMAT=rgba8` in
  `env.txt` turns it off.
- Every 10 s the log has `android: fps ..., MB, battery ... C`.

## Debug aids

`files/env.txt` (beside `cod4/`): `KEY=value` lines set before the game starts,
e.g. `COD4RW_PERF=1` (any `COD4RW_*` variable skips the menus).
`files/cod4rw.log` is the game's log (as on Windows); logcat has the same
under `RustStdoutStderr`.
