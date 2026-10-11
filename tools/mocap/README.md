# Phone-video mocap → viewmodel clips

Turns a phone video of your own hand (filmed from the eye, first person) into
viewmodel animation clips the game plays (`crates/game/assets/mocap/*.json`,
loaded by `crates/game/src/killstreaks/mocap.rs`). Everything runs locally:
a venv here, the MediaPipe 0.10.14 wheel from PyPI, which bundles its hand
models, so nothing else is downloaded.

## Setup (once)

```bash
cd "D:/cod4 rewrite/tools/mocap"
python -m venv .venv
PIP_CACHE_DIR=.pipcache TMP=.tmp TEMP=.tmp .venv/Scripts/python -m pip install opencv-python "mediapipe==0.10.14" numpy scipy
```

## Steps (the phone take as the example)

1. **Look at the video first.** Check which hand, the camera angle, and the frames the motion spans.
2. **Track** the hand (21 keypoints a frame; camera-space pose by solvePnP):
   `.venv/Scripts/python -I track.py <video> out/take1/track.npz 140 330`
3. **Entry**: a hand cut off by the frame's edge isn't detected. Optical flow follows it
   back from the first found frame:
   `.venv/Scripts/python -I track_entry.py <video> out/take1/track.npz out/take1/entry.npz 30`
4. **Rig**: dump CoD4's viewhands, gun skeleton and the weapon's anims (crates/iw3 example):
   `cargo build --profile fast -p iw3 --example mocapref -j 6` then
   `target/fast/examples/mocapref.exe radar_mp viewmodel_base_viewhands > tools/mocap/ref/radar_mp_viewmodel_base_viewhands.json`
5. **Solve** and write the clips (all settings live in the take's JSON, `takes/phone_take1.json`):
   `.venv/Scripts/python -I solve.py takes/phone_take1.json`
6. **Preview** without the game (stick figure from the eye at the game's 80° view, plus
   palm-on and side-on views of the grip):
   `.venv/Scripts/python -I preview.py takes/phone_take1.json out/take1/preview.jpg phone_raise 12`
7. **In game**: rebuild (the clips are compiled in), then
   `bash tools/mocap/run_ingame.sh <out dir> 1` (and `0` for CoD4's own anims), then
   `.venv/Scripts/python -I compare.py takes/phone_take1.json <out>/shots sheet.jpg 4 <out0>/shots`.
   Don't run it while `cod4rw-play.exe` (the user's game) is open.

## What the solver does

- **Smoothing**: zero-phase Butterworth (filtfilt). This is offline, so there's no lag, unlike a One Euro filter.
- **Wrist**: the hand's frame (across the knuckles, to the fingers, out of the palm) is turned
  from camera axes to view axes as filmed. Its position is pinned so the take's hold lands on
  CoD4's phone hold (`viewmodel_C4_detonator_fire` frame 0), and displacements are scaled by
  `displacement_scale`. Past the last tracked frame the hand moves on in the image plane only,
  then drops out of view (`exit_drop`).
- **Arm**: two-bone IK from CoD4's shoulder at fixed bone lengths. The elbow bends in the hold
  pose's plane, and `j_wristtwist_ri` takes half the forearm twist.
- **Grip**: the phone's box is fitted to the take's own fingers at the hold: its back rests on
  the fingers' plane, its far edge sits just inside the last knuckles, and it is turned about its
  screen so it leans as the remote does in the video (`video_phone_lean_deg`; that measure is
  sign-sensitive: + means the top leans right). Each finger's three joints are then optimised
  (no penetration, middle and tip bones touching, close to the take). The tracked finger motion
  rides on top at `finger_motion` share, capped at `finger_motion_max_deg`. The thumb press is a
  synthesized bump toward the face.
- **Phone**: rigid in the hand through `tag_clip2`, the joint `hands.rs` PropAnchor follows.
  The prop's default knob offsets (`hands.phone.*`) are taken out, so those knobs stay small
  live adjustments on top.
- **Left arm**: held at CoD4's hold pose (down, out of view). The C4 pullout and putaway are two-handed and would bring it up holding nothing (`left_arm_from_hold`). The torso tags come from the C4 anim each clip replaces, retimed.

## Gotchas found on the way

- MediaPipe's handedness label assumes a mirrored selfie: on a rear camera, "Left" means the right hand.
- `com_cellphone_on`: its bottom is at z = 0 and +z runs up to the antenna; the screen is its -y face.
- `com_cellphone_on` is also a material name, so look the XModel up by type.
- xanim names in weapon files are mixed case (`viewmodel_C4_pullout`); assets are lowercase.
- The arms go through the phone if each joint is closed toward the palm normal: use one hinge axis per finger.

## New takes (reloads, inspects, the laptop)

Film from the eye, holding something the size of the prop, with the motion in frame. Copy
`takes/phone_take1.json`, set the frames for each clip and the replaced base anim, run 2–6,
and add a loader entry like `phone_anim` for the new clips.
