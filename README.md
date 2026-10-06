# CoD4 Rewrite

A reimplementation of *Call of Duty 4: Modern Warfare* multiplayer on [Bevy](https://bevyengine.org) 0.19.

The repository contains **no game data**. Maps, models and textures are read at runtime from your own
CoD4 installation, the way OpenMW works for Morrowind.

## Download and play

1. Download this repository (Code > Download ZIP) and unzip it anywhere.
2. Double-click **`Launcher.exe`**. It downloads the latest build of the game from this repository's
   [Releases](https://github.com/Nilbong/COD4-Rewrite/releases) into a `game` folder beside it, keeps it up to
   date (it checks each time it starts, in about a second) and starts the game.

No Rust or other tools are needed: GitHub builds `cod4rw.exe` for every release. You need Windows, a graphics
card with Vulkan, and your own installed copy of CoD4 (Steam or retail, patch 1.7); the game reads its maps,
models and sounds from it, and none of them are downloaded. `Launcher.exe --build` builds from this folder's
source instead (needs Rust, see below).

For maintainers: pushing a version tag (`git tag v0.1.0 && git push origin v0.1.0`) makes the `Release`
workflow (`.github/workflows/release.yml`) build `cod4rw.exe` and `Launcher.exe` and attach them to the release.

## Running

You need a Rust toolchain and an installed copy of CoD4 (Steam or retail, patch 1.7).

Installs of *Call of Duty: Black Ops* (`BLACKOPS_PATH`, the folder with `BlackOps.exe`) and *Call of Duty: World at
War* (`WAW_PATH`, the folder with `CoDWaWmp.exe`) are optional. When found, their weapons and characters are added to
the game; without them it runs with CoD4's content alone. Like CoD4, they are found automatically in common Steam
library locations.

```bash
cargo run --release -p game
```

This opens CoD4's main menu; *Private Match* then *Start Match* begins a match. Pass `--map` to skip the menus and go
straight into a match:

```bash
cargo run --release -p game -- --map mp_killhouse --bots 6
```

The install is found automatically in common Steam library locations. If it isn't, set `COD4_PATH` to
the folder containing `iw3mp.exe`.

Options:

| flag | default | |
| --- | --- | --- |
| `--map` | `mp_killhouse` | start this map directly (see map support below) |
| `--bots` | `6` | players per team (you take one Marines slot) |
| `--skill` | `0.6` | bot skill, 0..1 |
| `--bodycam` | off | start with Bodycam gunplay |
| `--hardcore` | off | Hardcore rules (also a Private Match option) |
| `--spectate` | | watch the bots instead of playing: a free camera, or follow a bot by name (`Gaz`) or role (`anchor`, `rusher`, `flanker`) |
| `--record` | off | record your play to `recordings/<map>-<time>.csv` (see "Bots that play like you") |
| `--bot-profile` | | bots play like the player in this profile; `--skill 0.5` is their level |
| `--splitscreen` | | splitscreen co-op: each player's device, comma separated: `kbm` (keyboard and mouse) or `pad` (the next free controller), e.g. `kbm,pad,pad` |

### Bots that play like you

1. Play a few matches with `--record` (10+ minutes in total; each match writes a file under `recordings/`).
2. `python tools/fit_player.py recordings/*.csv -o profiles/me.txt` measures your reaction time and aim speed, how you
   strafe, crouch, jump and push in fights, how close you hip-fire (standing and on the move), the range you take fights at, and how much you hold
   still and sprint. The profile is plain `key = value` lines you can read and edit.
3. Play with `--bot-profile profiles/me.txt`. `--skill 0.5` makes bots your level; higher or lower scales their
   timing from there, and each bot still varies a little.

Real play is also recorded on its own (any match without `COD4RW_*` debug variables) to
`%LOCALAPPDATA%\cod4rw\recordings`, for training **learned models** of how you play (`tools/learn`,
`crates/game/src/bots/nets`): small networks run on the CPU. The first is aim: `python tools/learn/aim_data.py
<recording dirs> -o aim.npz`, `python tools/learn/train_aim.py aim.npz -o aim_model.json` (it reports how the model
aims at held-out sightings against you), then `COD4RW_NETAIM=aim_model.json` makes bots aim with it while engaging.
`python tools/learn/aim_eval.py player=<dir> bots=<dir>` compares any recordings' flicks (time to settle, overshoot,
tracking error). With minutes of recordings the hand-built aim is still closer to the player, so it stays the default;
the learned one needs hours. CoD4X demos (10 snapshots a second) are too coarse for aim but fine for movement.

### Controls

**F10: bug report.** Takes a screenshot, pauses the game and asks what's wrong. Type a description, Enter saves it
(Esc discards). Each report is a PNG and a text file in `bugreports/` with your description plus the map, the time,
where you and the camera are, and what every bot was doing.

Spectating (`--spectate`): mouse to look, WASD to fly, Space/Ctrl up and down, Shift faster, mouse wheel for speed.
Click to follow a bot (left/right: next/previous), V for first/third person, F to fly free again.

Click to capture the mouse. Esc releases it; in a match started from the menus, Esc opens CoD4's in-game menu
(Choose Class).

| | |
| --- | --- |
| WASD | move |
| Shift | sprint |
| Space | jump / stand up |
| C | crouch (toggle) |
| Ctrl | prone (toggle) |
| Q / E | lean (Bodycam gunplay) |
| B | switch between CoD4 and Bodycam gunplay |
| I | inspect the weapon (firing, aiming, reloading, sprinting or switching stops it) |
| F5 | switch between first and third person |
| LMB / RMB | fire / aim down sights |
| R | reload |
| G | frag grenade (hold to cook) |
| 4 | special grenade (flashbang, stun or smoke, by class) |
| 5 | equipment (C4, claymores, RPG-7) or the rifle's grenade launcher |
| F, F | detonate C4 (also: aim with C4 in hand) |
| 1 / 2, mouse wheel | primary / secondary weapon |
| Tab | scoreboard |

**Controllers.** Xbox and PlayStation pads work alongside keyboard and mouse, with CoD4's console layout. Whichever
was used last is in charge: with a pad, the hints show its buttons (Xbox or PlayStation glyphs, picked from the pad),
the menus get a footer and a focus you move with the D-pad or left stick, and aim assist helps the right stick
(never the mouse): slower turning over an enemy, the view following them while you move or aim, and a snap onto
one near the crosshair when you pull the aim trigger. Firing and taking hits rumble.

| Xbox | PlayStation | |
| --- | --- | --- |
| left / right stick | left / right stick | move / look |
| LS | L3 | sprint (lasts while you keep going forward) |
| A | cross | jump / stand up; in menus select |
| B | circle | crouch, hold for prone; in menus back |
| X | square | reload |
| Y | triangle | switch weapon |
| LT / RT | L2 / R2 | aim down sights / fire |
| RB / LB | R1 / L1 | frag (hold to cook) / special grenade |
| View | Share / Create, or the touchpad | scoreboard (hold) |
| Menu | Options | in-game menu |
| D-pad down / up | D-pad down / up | inspect the weapon (View/Share + D-pad down: Bodycam gunplay) / night vision |
| RS | R3 | knife (View/Share + RS/R3: third person) |
| D-pad left / right | D-pad left / right | equipment or grenade launcher / kill streak |
| LS / RS while aiming in Bodycam gunplay | L3 / R3 likewise | lean left / right |
| right stick in menus | right stick in menus | turn the gun preview |

`COD4RW_PAD_SENS=<x>` scales the stick's look speed, `COD4RW_PAD_INVERT=1` inverts it, `COD4RW_PAD_AIM_ASSIST=0` turns
aim assist off, `COD4RW_PAD_RUMBLE=0` rumble, and `COD4RW_PAD=ps|xbox` forces the glyphs.

On Windows, DualSense (and Edge) and DualShock 4 pads are read directly over HID, by USB or Bluetooth, sticks,
analog triggers, buttons and rumble, as SDL does, rather than through Windows.Gaming.Input (`COD4RW_PAD_HID=0` goes
back to that). Looking for newly plugged-in pads runs on a thread of its own: listing HID devices takes about half a second on Windows, and inside the read loop it made buttons seem to stick. `cargo run --release -p game --example padcheck` shows what the game sees of every pad plugged in.

## What works

* **Fastfile loader** (`crates/iw3`): decompresses `.ff` zones and parses the IW3 asset stream (technique sets,
  materials, images, xmodels, comWorld, gfxWorld, gameWorldMp, clipMap, mapEnts, rawfiles). It tracks the game's
  memory blocks exactly, so every cross-asset reference resolves.
* **iwd / iwi**: the `.iwd` archives are mounted as one layered filesystem, and DXT1/3/5 textures are uploaded to the
  GPU compressed with their full mip chains. Wavelet-compressed images (the menu logo, perk icons) are decoded.
* **Menus** (`crates/game/src/ui`): the game's own menus from `ui_mp.ff`, run by an interpreter of the IW3 menu system:
  item layout and alignment, materials, the game's bitmap fonts, localized text, visibility/text/material expressions,
  scripts (`open`, `close`, local vars, dvars, stats, `exec`, `uiScript`), focus highlights, text fields and the
  settings item types, and the menus' sounds (hover and click sounds, and the main menu music, which stops when a
  match loads). The main menu, Private Match and Create a Class work.
* **Create a Class**: the original menus, with player stats (custom classes, unlocks, progress) saved to
  `%LOCALAPPDATA%\cod4rw\stats.txt`. Weapons are shown as rotatable 3D models instead of pictures (drag them),
  with the chosen attachments and camo on the gun; hovering an attachment or camo previews it. Like Black Ops, your character stands in the middle
  of the screen holding the class's primary weapon, attachments and camo included. Unlike CoD4, a gun can
  carry several attachments: one sight (red dot or ACOG), a silencer and a grip; the grenade launcher stays on its
  own.
* **Combat Record** (`crates/game/src/ui/combat_record`): replaces Rank & Challenges in the menus. It uses the
  original `menu_challenges` artwork, fonts, title/footer, button highlights and rank-panel materials from the
  installed CoD4 assets. Calling cards reuse its original camo artwork. The overview shows K/D, W/L, match
  playtime, headshots, assists, best streak, favourite weapons, rank and XP to the next rank.
  The weapon pages keep separate kills, headshots, shots and time held for CoD4, Black Ops and World at War guns;
  these new counters start from this update, without guessing historical weapon totals. Challenge pages show
  every stage from CoD4's challenge tables, with progress, rewards and filters for attachments, camos, career
  and individual weapons. Existing gameplay limits on which challenges count still apply, as listed below.
  Identity includes a saved 16-character name, four-character clan tag, 16x16 pixel emblem editor (palette,
  eraser, undo, clear, reset, save/cancel) and six calling cards unlocked through career milestones. Names and
  clan tags appear in matches; emblems/cards appear in the record's player card. These are local Player 1
  stats, saved in the existing `stats.txt`, and are not yet verified or shared online. Playtime is recorded as
  you play, including matches left early. Debug runs still never write the profile.
* **Progression** (`crates/game/src/ui/progression.rs`): CoD4's XP (10 a kill, 5 in free-for-all, doubled for a
  headshot; 2 an assist; 30 an objective; the match bonus at the end) climbs the 55 ranks of `mp/rankTable.csv`
  with "You've been promoted!", the rank's icon and what it brings ("New Weapon: M4 Carbine"). Each CoD4 gun's
  Marksman (kills) and Expert (headshots) challenges count up from `mp/challengeTable_tier*.csv`, each level giving
  its XP and unlock (red dot, silencer, ACOG; digital, blue and red tiger camos) with "Challenge Completed!".
  CoD4's other challenges count too where the game has what they need (`crates/game/src/ui/challenges.rs`):
  crouch, prone, through-wall, grenade, cooked-grenade, Martyrdom, claymore, knife (and backstab), airstrike and
  helicopter kills; kills while downed, stunned or flashed, of stunned, flashed or airborne enemies; multi-kills
  from one blast or sniper bullet; Fearless, Slasher, Airborne, The Brink, Tango Down, Extreme Cruelty, Rival,
  Counter-MVP, Fast Swap; hardpoint calls, Flyswatter, assists, Marathon, Base Jump, Goodbye, Invincible,
  Survivalist; S&D's bomb kills, Hero and Last Man Standing; and match wins, placings, MVPs, The Edge, Flawless and
  Star Player. Ones needing cars, shootable explosives, weapon or grenade pickups, mounted guns or grenade impacts
  don't count.
  Kills, deaths, assists, headshots, wins and time played go in CoD4's own stats. Unlocks are your choice: a new
  profile gets CoD4's (weapons, perks, the Demolitions and Sniper classes and Create a Class at their ranks;
  attachments and camos from the challenges; Black Ops' and World at War's guns are always open), while a
  profile saved before the choice existed keeps everything unlocked. Change it with the line
  `dvar cod4rw_unlocks all` (or `cod4`) in `stats.txt`, or run once with `COD4RW_UNLOCKS=all` (or `cod4`), which
  is saved. XP, ranks, challenges and their notices work the same either way.
* **Splitscreen co-op** (`crates/game/src/splitscreen.rs`): in Private Match, *Splitscreen* goes Off, 2, 3, 4 Players;
  the local players are listed in the team panels ("Player 2 - DualSense Edge") and clicking one moves them to the
  next device: the keyboard and mouse, each controller (named from its USB ids: DualSense, DualSense Edge, DualShock 4,
  Xbox, Switch Pro) or *Any Controller* (the next one connected; a controller that goes away is replaced the same
  way). A new player joins Player 1's team; the arrow at the end of a player's row moves just them to the other team
  (co-op or versus), and *All Join ...* moves everyone. Each plays in their part of the window (two one over the other,
  three with Player 1 along the top, four in quarters) with their own camera, gun, HUD (ammo, compass, crosshair,
  hit markers, damage arrows, scoreboard on View/Tab, scopes, night vision) and controls (move, look and aim assist,
  fire, aim, grenades, knife, equipment, kill streaks, objectives, Last Stand, rumble on their own pad). Each sees
  the others' bodies and gun flashes, never their own from inside. Menus are per player too (`ui/split.rs`): at the
  start everyone picks a class at once, each in their own part of the window, and a player's Menu button (Escape for
  the keyboard) opens the pause menu for them alone while the others play on. A controller works its player's menus
  (D-pad or stick, A, B); the keyboard and mouse player's has the mouse while it's up. Bevy's ambient occlusion is
  off in splitscreen (it reads the whole window's depth, not a view's).
  Player 1 is the one whose stats, XP and challenges are saved and whose view the sound is heard from. Left out
  with more than one player: killcams, Bodycam gunplay, lens scopes (classic is used), the sun's flare, and the
  airstrike's map (it strikes where the player aims). Effects' sprites face Player 1's view. Each view draws the
  sun's shadows anew, so in splitscreen they're lighter: two cascades, and props cast none (their shadows are in the
  lightmaps). On the test machine with 12 bots: 1 player 10 ms a frame, 2 players 16 ms, 4 players 24 ms.
* **Hardcore** (`--hardcore`, or Hardcore: On in Private Match): CoD4's `hardcore_settings.cfg`: 30 health
  (still regenerating), friendly fire hurts teammates (`scr_team_fftype 1`; a team kill isn't a kill), 10 s to
  respawn, no killcams. The HUD is CoD4's Hardcore one: no compass unless your team's UAV is up, no crosshair,
  ammo counter, equipment or hardpoint icons, score bar, XP popups, use hints or frag warnings; the kill feed,
  hit markers, damage arrows and notices stay. No "time running out" music. Any game type can be Hardcore.
* **Black Ops guns** (with a Black Ops install, see `crates/game/src/bo1.rs`): picking a primary weapon or sidearm
  first asks *Call of Duty 4* or *Black Ops*. Black Ops lists its 31 primaries by group (assault rifles, SMGs, LMGs,
  shotguns, snipers) and its 5 pistols, then the gun's own attachments (one per slot: sight, underbarrel, muzzle,
  magazine/trigger: Red Dot, Reflex, ACOG, IR, Variable Zoom, Suppressor, Extended/Dual Mag, Rapid Fire, Grip,
  Grenade Launcher, Masterkey, Flamethrower, ...) and its 15 camos (with its metal and wood detail textures, masked
  like Black Ops does). They are rows of CoD4's own popups (`ui/bo1.rs`), with Black Ops' names, pictures and stats;
  in matches they use Black Ops' weapon files, models, animations (on your character's or team's arms) and sounds
  (decoded from Black Ops' sound banks by `t5::sound`, the first time each plays: shots with their ring-off layers,
  reload notetracks). Their Create a Class attribute bars are worked out from their stats on CoD4's scale
  (`ui/bo1.rs`). Black Ops' gold camo is a near-black base under a gold specular map: it's shown as CoD4's gold guns'
  colour under the shine. Not offered: dual wield, launchers and the crossbow/ballistic knife.
* **Sniper scopes and bipods**: fully aimed, a scoped gun shows its scope's picture (CoD4's snipers, Black Ops'
  snipers and infrared scopes, World at War's scoped rifles and PTRS-41) with black either side and the gun hidden;
  the HUD stays on top (`crates/game/src/ui/scope.rs`). World at War's guns with a bipod deploy it when you aim
  prone, or crouched or standing at a ledge of the right height (a hint says when you can): you can't move, the view
  stays within the bipod's arcs and the gun fires with its mounted weapon file's stats (faster, a 0.5 degree cone, a
  small jitter instead of recoil). Aiming again, moving, jumping or changing stance packs it up
  (`crates/game/src/bipod.rs`).
  World at War's guns have no sprint animations; as in World at War, sprinting carries them in their weapon file's
  sprint pose (`sprintRot` turned, `sprintOfs` moved, eased in and out over the sprint in/out times) and sways them
  with the view's bob, capped by `sprintBobH/V` (`viewmodel.rs`'s `weapon_angles`).
  **Inspecting** (I; pad: D-pad down): none of the three games has inspect animations, so it's posed: the gun turns
  up to show its left side, rolls over to show its right and comes back (3.4 s, `viewmodel.rs`'s `INSPECT_KEYS`).
  Firing, aiming, reloading, sprinting, switching, a grenade or the knife stops it.
  World at War's rifles with the rifle grenade attachment (Kar98k, M1 Garand, Gewehr 43, Springfield, Mosin-Nagant,
  Arisaka) switch to it on 5 (pad: D-pad left), with World at War's own animations: two grenades that fly from its
  weapon file's speed and go off on impact through `crate::explosives` (CoD4's M203 effects stand in for World at
  War's).
* **Lens scopes** (Options > Game > Sniper Scope, `cg_scopestyle`, or `COD4RW_SCOPE=classic|lens` for a run): instead of
  CoD4's full-screen scope picture, the view stays at a rifle's aimed view and a lens in the middle (0.42 of the screen's
  height) shows a second camera's view under the scope's reticle and rim, magnified exactly as classic (its field of
  view is the part of the gun's `adsZoomFov` the lens covers, so what's under the reticle is the same size on screen).
  Black Ops' and World at War's rifles stay drawn below the lens; CoD4's, whose aimed pose puts the eyepiece over the
  view, go once fully aimed. Classic is the default; the lens costs about 8-10 ms a frame while scoped (the extra camera
  view). `COD4RW_SCOPETEST=<dir>` aims in and screenshots the same view through both (`classic.png`, `lens.png`).
* **Game types** (`crates/game/src/modes`, the match flow in `tdm.rs`): the Private Match lobby's *Game Type* picks
  Team Deathmatch, Free-for-all, Domination, Search and Destroy, Headquarters or Sabotage
  (`--mode war|dm|dom|sd|koth|sab` from the command line). Free-for-all has no teams: everyone else is
  an enemy (`combat::hostile`), players spawn on the map's free-for-all spawns away from everyone, the first to 30 kills
  (the default) wins, the scoreboard is one list and CoD4's own HUD shows your score against the leader's.
  Domination (`--mode dom`, `modes/dom.rs`) plays the map's flags A, B and C as `_dom.gsc` does: standing in one
  takes it in 10 seconds unless an enemy is in it too (leaving starts it over), every 5 seconds each team scores a
  point per flag held, 200 wins (30 minutes). Flags fly their holder's colours; their icons show over them, on the
  compass and as a capture bar while you take one; the announcer calls securing, secured and lost flags; teams spawn
  on the map's Domination spawns, nearer their own flags.
  Search and Destroy (`--mode sd`, `modes/sd.rs`) plays CoD4's rounds: 2:30 each, one life, the attackers pick up the
  bomb where it lies and plant it at site A or B (hold F, or a pad's X, for 5 seconds); it goes off 45 seconds later
  (destroying the target and anyone near) unless a defender defuses it (5 seconds). Wiping out the defenders, or
  the attackers before they plant, also wins the round, and time running out goes to the defenders. First to 4
  rounds; the sides swap every 3. The sites, the dropped bomb, the plant/defuse bars and hints and the round's
  result are on the HUD, CoD4's clock turns into the bomb's timer once it's planted, and the announcer calls the
  bomb taken, lost, planted and defused.
  Headquarters (`--mode koth`, `modes/koth.rs`): an HQ comes up at one of the map's HQ spots in turn (its radio and
  props appear there). Standing in it with no enemy there takes it in 20 seconds; the holders score a point a second
  and can't respawn while they hold it, until the enemy destroys it (10 seconds in it) or it goes offline after 60
  seconds, and the next comes up elsewhere. 250 wins (30 minutes). Its icon, the capture/destroy bar, the time it
  has left and the announcer's HQ lines are as in CoD4.
  Sabotage (`--mode sab`, `modes/sab.rs`): one bomb in the middle and a target for each team. Either team picks the
  bomb up and plants it at the other's target (hold F or X for 2.5 seconds); it goes off 30 seconds later unless the
  target's team defuses it (2.5 seconds), when it lies there for anyone to take. Destroying the enemy's target wins
  (one target by default). If the time (20 minutes) runs out with nothing planted, sudden death: no respawns until a
  target goes up or a side is wiped out.
* **Classes in matches**: a match started from the menus opens CoD4's class menu (the five default classes and your
  custom ones) and spawns you once you pick; Esc > Choose Class changes it from your next spawn. You carry the class's
  primary and secondary (switch with 1/2 or the mouse wheel, with the putaway/pullout animations), each with its own
  ammo, attachments and camo on the viewmodel. Weapon stats for attachment combinations CoD4 doesn't have are the base
  weapon with each attachment variant's changes. Perks that affect what the game models work: Stopping Power,
  Juggernaut, Sleight of Hand, Double Tap, Steady Aim, Bandolier, Extreme Conditioning and Overkill. Grenades,
  equipment, the underbarrel grenade launcher and the other perks don't do anything yet. Bots carry the AK-47, and so
  does the player in matches started with `--map`.
* **Supply drops** (`crates/game/src/supply.rs`), after *Advanced Warfare*: every 10 minutes of match time earns a
  drop, opened from *Supply Drops* on the main menu, where its three items turn over one by one. Items are Enlisted,
  Veteran, Professional or Elite, and each is a weapon variant or a character:
  - weapon variants are guns under their own names with their stats tweaked (damage, accuracy, range, fire
    rate, handling, mobility): Enlisted +1/-1, Professional +2/-1, Elite +2 +1/-1 with a signature camo (from its
    own game's camos; World at War's guns have none). Every gun has them: CoD4's, and Black Ops' and World at War's
    when installed; a drop's variant is as likely to be from one game as another. Create a Class equips them per
    weapon (*Primary Variant*, *Secondary Variant*), and the tweaks apply to the real weapon stats in matches;
  - characters (`crates/game/src/characters.rs`) are the soldiers of CoD4 and, with a Black Ops install (read through
    `crates/t5`), of Black Ops, as each game's character scripts build them, from multiplayer and the campaigns:
    other soldiers are Enlisted, ghillie suits, hazmat, night vision and pilots Veteran, the supporting cast (Nikolai,
    Griggs, Hudson, Bowman, Weaver...) Professional, and the leads (Price, Gaz, Zakhaev, Mason, Woods, Reznov,
    Dragovich...) Elite. Black Ops' low-detail and near-identical models are left out. Everyone starts as a Spetsnaz
    rifleman; *Character* on the main menu picks the one worn, shown in 3D there and in Create a Class. In matches
    (`crates/game/src/wardrobe`) it's your body (seen in third person with F5, its shadow always) and your
    first-person arms: the character's own (Black Ops' outfits have theirs), else the nearest CoD4 hands. Bots wear
    characters too, a random cast per side from the map's own soldiers and those of one other zone; each bot's is
    logged. Their zones load alongside the map (the match waits only for yours; bots wear their team's models
    until theirs arrive). Black Ops' characters move with Black Ops' own player animations.

  The collection is saved to `%LOCALAPPDATA%\cod4rw\supply.txt`.
* **Rendering**: aiming to look better than the original, from the original data:
  - the map's baked lightmaps as indirect light (CoD4's directional lightmap formula, evaluated per pixel with the
    normal map) under a real-time sun with cascaded shadows;
  - CoD4's normal and specular maps;
  - the live sun at CoD4's own strength over the lightmaps (`sunColor * (sunLight - ambientScale) * (1 - diffuseFraction)`,
    the rest being baked in), so shade keeps its detail (`COD4RW_SUNMODEL=now` for the older, stronger sun);
  - the map's baked reflection probes through a physically based environment BRDF;
  - characters, guns and props lit from the map's light grid (the baked light CoD4 stores through the playable
    space), as an irradiance volume, so they darken indoors and pick up bounce light like the world around them;
  - the map's sky cube as a skybox;
  - HDR with auto-exposure, ambient occlusion, SMAA and 16x anisotropic filtering (and Bevy's bloom on maps without a
    vision file);
  - each map's own film grading (`vision/<map>.vision`: contrast, brightness, desaturation, light and dark tints) over
    the finished frame, with the math of CoD4's film shaders (`postfx_color`/`vertcol_film`, disassembled), rebalanced
    for this renderer: CoD4's contrast and brightness were mostly a 1.4-1.7x gain for its dark raw frame, which on an
    auto-exposed one blew out highlights and crushed shadows, so the tints and desaturation keep each map's mood but
    mid-grey stays mid-grey, black lifts slightly and desaturation is gentler (`COD4RW_FILM=cod4` for CoD4's own). Its colour
    tint (Killhouse's turns everything yellow) is off unless Options > Game > Film Tint is on (`r_filmtint`, or
    `COD4RW_FILMTINT=on|off` for a run); off, the tints are greyed, keeping how the film darkens and lightens;
  - each map's glow from the same file (`r_glow`, bloom cutoff, desaturation, intensity and radius), as CoD4's glow
    shaders do it: what's brighter than the cutoff, graded with the film, desaturated, blurred at a quarter size and
    added back (`glow_consistent_setup`, `filter_symmetric`, `glow_apply_bloom`). Maps with `r_glow 0` (crash, strike,
    backlot...) have none, as in CoD4;
  - night vision on N: the `default_night` vision set `_load.gsc` gives every map (its green film and strong glow),
    behind the goggles' overlay (`nightvision_overlay_goggles`), switched through a fade to black with
    `item_nightvision_on`/`_off`. CoD4's multiplayer guns have no goggles animations, so there are none;
  - each map's sun (`sunflare_t` in its GfxWorld): the lens flare on the sun while nothing blocks it, growing as you
    look towards it, and the screen darkening (blind) and lightening (glare) of looking into it, fading in and out
    over the map's times;
  - Black Ops' light grid brightness for models (`r_lightGridIntensity` 1.3, the usual value in its maps' art
    scripts; its `r_lightGridContrast` is left off, as it blackened models' shaded sides on CoD4's grids);
  - optional ray-traced lighting (Options > Game > Lighting: Baked, Ray Traced Low or High, `r_lighting`, or
    `COD4RW_LIGHTING=baked|rt_low|rt_high`; from the next match), with Bevy's Solari on GPUs with ray queries: the
    sun, a sky-tinted dome only the rays see and lit-up fixtures (opaque unlit materials) traced through the map with
    bounce light, in place of the light grid and shadow maps (`crate::rtgi`). World surfaces keep their baked
    lightmap as a floor under the traced light (`shaders/world_deferred.wgsl`, with CoD4's normal maps), so
    interiors are never darker than Baked. Without DLSS there's some grain in shade. Solari's shaders take about
    10 s to compile the first time it's used (the map shows a flat colour until then). Splitscreen always uses
    Baked. `COD4RW_RES=1920x1080` sizes the window for timing;
  - alpha test/blend/decal state from the original material state bits, including the screen-add blend of lamps'
    fake light beams and flares (which fade as they turn edge-on, on the world and on props) and the 2x multiply of
    doorways' HDR portals (`hdrportal_lighten`/`_darken`: what's seen through a doorway brightens or darkens with
    distance, from the material's `falloffParms`).
* **Collision**: clipMap brushes become convex hulls (solid and player-clip), and terrain/patch triangles become a
  trimesh.
* **Movement**: a port of CoD4's pmove (`bg_pmove`/`bg_slidemove`/`bg_jump`): ground and air acceleration,
  friction, stop speed, backpedal/strafe scales, sprint (4 s, 1.5x, forward-only), stand/crouch/prone with the 400 ms
  stance blend, jump height and re-jump delay, landing slowdown, and fall damage. Steps are `PM_StepSlideMove`'s
  (from the decompiled game): 18 units on the ground (and down 9 to stay on stairs), up to `jump_stepSize` 18 near a
  jump's top, landing on anything short of a wall, and costing speed by the height changed; door lips no longer catch
  the hull. Aiming down sights is CoD4's "walking": 0.4 of the speed times the gun's `adsMoveSpeedScale` (rifles
  40%, SMGs and pistols 80%). Mantling (`movement/mantle.rs`): holding jump facing one of the map's mantle volumes
  climbs onto the ledge behind it (and over, for `mantle_over`), along the mantle animations' root motion; "Press
  SPACE to mantle" shows while one is there to climb. Measured
  speeds match the game: run 190, sprint 285, crouch 123.5, prone 28.5 units/s.
* **Weapons**: the real weapon files from `common_mp.ff` (damage, fire rate, clip, reload times, per-stance hip
  spread, ADS, view kick, switch times), with CoD4's fire types: single shot (a trigger pull per shot: M14, G3,
  Dragunov, M21, Desert Eagle, shotguns), bursts (the M16's three, then `player_burstFireCooldown`'s 0.2 s), and bolt
  actions (M40A3, R700, the W1200's pump) working the action after each shot (`iRechamberTime`, with the rechamber
  animation). Bots let go and pull again at a human click rate. Shotguns and bolt actions reload a round at a time
  (`bSegmentedReload`: the W1200, M1014, M40A3, R700, Black Ops' Ithaca, SPAS-12, HS10 and Python, World at War's
  trench gun): the reload's start loads one, each cycle of its loop another, then its end, each with its own
  animation and time; a fresh trigger pull cuts it to the end once there's a round to fire.
* **Map props and clutter** (`crates/game/src/props.rs`): the maps' `script_model` props (destructible cars and
  glass, barrels, pipes, palms, cameras; game-mode objects only in their mode) and their dynamic entities
  (`dynEntDefList`: hundreds of cinder blocks, cans, bottles, boxes, crates and tyres a map) are drawn where CoD4
  puts them. Static models kept in `common_mp` (`,name`: rubble bricks, flags, cluster bombs) are found there.
* **Fog** (`crates/game/src/fog.rs`): each map's own, from its art script (`setExpFog(start, halfway, r, g, b)`;
  CoD4's density ln 2 / halfway), so the distance fades into the map's colour; none where CoD4 has none (Killhouse,
  Wet Work).
* **Bullet penetration** (every shot, players' and bots'): bullets go through surfaces as deep as CoD4's
  `info/bullet_penetration_mp` table allows for the gun's `penetrateType` (small, medium or large) and the surface
  type (6 units of concrete for a small round, 12 for medium; 12-16 of wood; 72 of glass or cloth; players' flesh
  24-40), up to five surfaces, each taking the fraction of its depth used off the damage (`Bullet_Fire`). Deep Impact
  doubles the depths. A bullet hurts each player it passes through once.
* **Perks** (`crates/game/src/perks.rs`, with the weapon ones in `loadout.rs`), CoD4's numbers throughout: Stopping
  Power (bullet damage ×1.4), Juggernaut (damage taken ×0.75), Sleight of Hand (reloads ×0.5), Double Tap (fire rate
  ×1.33), Steady Aim (hip spread ×0.65), Extreme Conditioning (sprint ×2), Bandolier (starting at the gun's
  `iMaxAmmo`), Deep Impact, Frag x3 and Special Grenades x3, Martyrdom, Sonic Boom (explosives ×1.25), Last Stand
  (a non-headshot bullet or fall downs you: prone with your pistol, or a Beretta, and all its ammo for 10 s, then
  you bleed out to whoever downed you; hold F to end it), Iron Lungs (scoped weapons sway by `adsIdleAmount`; Shift
  holds your breath 4.5 s, 9.5 with the perk, then you gasp), Dead Silence (no footsteps for others), UAV Jammer
  (left off enemy UAV sweeps), Bomb Squad (enemy C4 and claymores marked within 1500 units). Overkill's two
  primaries come from Create a Class. Eavesdrop does nothing: it lets you hear enemy voice chat, and there is none.
* **Killcam** (`crates/game/src/killcam/`): killed, you see the last 5 s (and a moment after) again 1.5 s later, through
  your killer's eyes with their gun (camo and all) in hand, firing and aiming as they did, every body where and as it
  was, their shots and explosions heard again, under a KILLCAM banner with who killed you with what; F (X / Square on a pad) skips it, and
  you respawn when it's over. An explosive kill rides the grenade or projectile. When a match ends, everyone sees its
  last kill again (FINAL KILLCAM) before the scoreboard. It's replayed from a 12 s history of every pawn,
  projectile, shot and explosion sampled 20 times a second from what the game already has; the live game carries on
  underneath.
* **Server browser** (groundwork for multiplayer; `crates/net`, `crates/game/src/net`, `ui/browser.rs`): Join Game opens
  CoD4's own Join Server menu, filled as CoD4 fills it: the server list (name, map, players, game type, ping; click a
  column to sort), Source (Local / Internet / Favorites), the Game Mode filter, Refresh List and Quick Refresh, and New
  Favorite / Del. Favorite. Games are found with CoD4's (Quake 3's) query packets: every match being played can answer
  `getinfo` on UDP port 28960 (the next free one up to 28963) and sends a heartbeat to the master server; Local
  broadcasts on the network, Internet asks the master server (`getservers`), Favorites asks each saved address
  (`%LOCALAPPDATA%\cod4rwavorites.txt`). There's no multiplayer to join yet, so Join Server says so, and a match only announces itself with
  `COD4RW_ADVERTISE=1` (otherwise Windows would ask to let the game on the network every match; it still asks once
  the first time Join Game looks for games). The master
  server is `cargo run -p cod4rw-net --bin master [port]` (UDP 20810 by default); the game uses one on this machine
  unless `COD4RW_MASTER=host:port` points elsewhere.
* **Melee** (`crates/game/src/melee.rs`): V knifes, as CoD4 does: an 0.8 s swing that hits 0.129 s in for 135
  (a kill) whoever is within 64 units in front; with an enemy within 128 units near the crosshair it's a lunge (the
  knife's charge animation, the player carried at them by `player_meleeChargeFriction`, the view drawn onto them). A
  swing cancels a reload; the kill feed shows the knife.
* **Grenades** (`crates/game/src/grenades.rs`): G throws a frag, CoD4's offhand way: the gun goes down
  (`quickDropTime`), the pin comes out (`iHoldFireTime`), it's held as long as G is, thrown (leaving the hand
  `iFireDelay` in) and the gun comes back up (`quickRaiseTime`), with the weapons' own animations. Its 3.5 s fuse starts
  with the pin, so it can be cooked (held too long, it goes off in hand; dying with it drops it live). It flies at
  `iProjectileSpeed` plus `iProjectileSpeedUp`, falls with CoD4's gravity and bounces by each surface's
  `parallelBounce`/`perpendicularBounce`, then hurts everyone in sight within `iExplosionRadius` (300 falling to 75
  for a frag; teammates spared), with `explosions/grenadeexp_default` and its sound. One frag a life, three with Frag
  x3; the HUD shows them by the ammo counter, and grenade kills get the grenade's icon in the kill feed. 4 throws the
  class's special grenade (one, three with Special Grenades x3; a flashbang without a class), each with its own
  viewmodel, effect and sound: a flashbang whites out the screen and muffles hearing for whoever sees it, longer
  nearer it (full strength inside `iExplosionRadiusMin`) and looking at it (`_flashgrenades.gsc`'s angle and distance
  scaling, up to 4.5 s); a stun grenade slows moving and turning for 2 s plus up to 4 s more by nearness
  (`Callback_PlayerDamage`'s concussion time), with a haze; smoke pours from its canister for 15 s (`SmokeCloud`).
  Neither catches the thrower's teammates. A live frag within 250 units (a flashbang within 500) shows CoD4's grenade
  indicator: its icon and pointer around the crosshair towards it. Bots (and any pawn) throw through `GrenadeInput`
  and can read `Flashed`, `Stunned` and `SmokeCloud`. Martyrdom drops a live frag where you die
  (`frag_grenade_short_mp`'s 2.5 s fuse).
* **Equipment** (`crates/game/src/explosives.rs`): the class's perk-1 item on 5 (CoD4's `+actionslot 3`): an RPG-7
  (two rockets; flies straight and goes off where it hits), C4 (two; thrown, sticks where it lands, goes off when you
  aim with it in hand or double-tap F, `watchC4AltDetonation`) or claymores (two; set down facing your way, they go
  off 0.75 s after an enemy moves into their 70° cone within 192 units, at least 20 in front, as
  `claymoreDetonation` does). A rifle with a grenade launcher (M203, or the AK-47's GP-25) switches to it on 5 as its
  alternate mode (`iAltRaiseTime`, the "to grenade" animation): its grenade falls and goes off where it hits once
  it has flown `iProjectileActivateDist` (375 units); nearer, it's a dud that hurts whoever it hits and bounces off.
  All from their weapon files (speeds, fire delays, radius and damage, projectile models, smoke trails), through
  `Damage` like everything else; the HUD shows the item and what's left by the grenades.
* **Viewmodel**: the map's viewhands with the weapon's gun model on `tag_weapon`, animated with the weapon's own xanims
  (raise, putaway, idle, fire, last shot, reload, reload empty, sprint in/loop/out, ADS up/down/fire). A red dot
  sight's dot is drawn as IW3's `mc_reflexsight` shaders do: projected to infinity along the lens so it sits on the
  aim point, sized by the material's `detailScale` (about a degree across), a white-hot core in a red glow over the
  lens grain. Guns shine like IW3's `lp_*_r0c0s0` models: their specular maps reflect the reflection probe nearest the
  camera through each material's fresnel settings (`envMapParms`), which is what makes gold guns gold.
* **Effects** (`crates/game/src/fx.rs`, `iw3::fx`): CoD4's own `FxEffectDef`s from the zones: muzzle flashes on the
  first-person gun (view effects) and on everyone else's (world effects), ejected cases that land, bullet impacts
  picked from the `FxImpactTable` by surface and bullet type with bullet holes, and blood on players (head or body,
  with an exit wound on kills). Billboard/oriented sprites, tails, omni lights, models, runners and decals are drawn,
  and so are clouds (IW3's particle clouds: specks through a growing ball, like glass glints and blood mist) and
  trails (each `FxTrailDef` cross-section swept along the points a moving effect lays every `splitDist`, its texture
  repeating and scrolling, like a rocket's smoke). Particles play their child effects: along their path
  (`effectEmitted`, the dust behind flying debris), where they first hit something (`effectOnImpact`: mud and blood
  splats, spent cases coming to rest) and where they die. An effect on something that goes (a rocket that hit)
  stops spawning, as IW3's do, and its trail fades out at both ends. Spot lights (no multiplayer effect has one) and sound
  elements are skipped. `COD4RW_FXTEST=<dir>` screenshots shots' effects; `COD4RW_FXPLAY=<dir>` plays the cloud,
  trail and child effect cases (glass, an exit wound, brick and mud impacts, an RPG trail on something flying past)
  and screenshots them; `COD4RW_FXAT=<dir>` plays one effect (`COD4RW_FXAT_EFFECT`) where a bug report's view
  (`COD4RW_FXAT_VIEW`, `x y z yaw pitch`) meets the world at `COD4RW_FXAT_SCREEN` (`u v`), optionally on something
  flying in from `COD4RW_FXAT_FROM` (CoD `x y z`) that goes where it hits, and screenshots it over 12 s;
  `COD4RW_FXLOG=1` logs particle counts and cost.
* **Kill streaks** (`crates/game/src/killstreaks/`, CoD4's `_hardpoints.gsc`): 3, 5 and 7 kills in a life earn a UAV,
  an airstrike and a helicopter, held until used with 6 (D-pad right), announced as CoD4 does (the typed-out notice,
  sound and leader's voice). The UAV sweeps every enemy onto the team's compass every 4 s for 30 s. The airstrike
  opens CoD4's location map (`fullscreenmap`: the minimap in `map_border`, `map_artillery_selector` under the mouse);
  a click sends three MiG-29s over the spot whose cluster bombs burst and scatter 12 bomblets each, 200 to 30 damage
  over 512 units to anyone they can see, credited to the caller (`death_airstrike` in the feed). The helicopter
  (`_helicopter.gsc`; a Cobra for the allies, a Hind for the axis, one at a time, none on maps without paths) flies in
  along the map's `heli_start` path, circles its loop path once and leaves. Every half second it targets the most
  threatening enemy its turret can see within 3500 units: 40-round bursts of `cobra_20mm_mp` after a 0.75 s spin-up,
  and FFAR rockets at a second target. Normal bullets hit it (1100 health; bullets do 0.3 of their damage until 500 is
  done). Past 500 it smokes and turns evasive. Past 1100 it spins down its crash path and explodes. Its kills go to the
  caller (`death_helicopter`). Bots use the same `HardpointInput` and can see `killstreaks::helicopter::Helicopter`.
  `COD4RW_STREAKTEST=<dir>` credits kills, uses each one and screenshots it (`COD4RW_STREAKTEST_LEAVE=1` lets the
  helicopter leave instead of shooting it down).
* **Player models**: the map's team bodies and heads (SAS/Marines vs. OpFor/Arab) holding the gun in their hands
  (its world model with attachments and camo, swapped on a weapon change), animated with CoD4's `pb_*` animations
  chosen from movement (stand/crouch/prone, run directions, sprint, ADS, jump take-off and landing, deaths). As in
  CoD4, the legs stay put until the view turns 45 degrees from them (or the player moves) and the spine turns and
  tilts the rest, so the chest and gun point where the player aims instead of the whole body spinning.
* **Bodycam gunplay** (B): an alternative to CoD4's, modelled on *Bodycam*. A helmet-mounted camera (wide lens with
  barrel distortion, vignette, chromatic aberration, motion blur, grain, timestamp, head bob and shake); a weapon the mouse
  steers freely of that camera, which only follows it, that sways, kicks back and climbs, tucks away from walls and shots
  leave along, with no crosshair or hitmarkers; and heavier movement (run 152, sprint 243 units/s, slower to start and stop) with leaning.
* **Audio**: CoD4's own sounds (the multiplayer aliases from `localized_common_mp` plus the map's), with each
  alias's volume and pitch ranges, distance falloff curve and channel, panned in stereo: gunfire (yours and others',
  last-round and suppressed variants), brass, bullet impacts and footsteps and landings by the surface they hit (from
  the clip brushes' materials), near-miss whizzes, the hit marker, pain and death cries per nationality, body falls,
  hurt breathing and sprint gasps, reload foley from the viewmodel animations' notetracks, others' reloads, dry fire,
  weapon switches, the team's spawn music and announcer, lead changes, the closing minute and countdown, victory or
  defeat, squad chatter and the map's ambience. Debug runs (`COD4RW_*`) are muted.
* **HUD**: CoD4's own, from its HUD menus, scripts and multiplayer config, with its materials and fonts: the rotating
  minimap (the map's `compass_map_*` image placed from its `minimap_corner`s, a heading tickertape, friendlies as
  arrows, enemies who fire unsuppressed as red pings for 2 s, 1250 units to the edge), the ammo counter (each round
  drawn in the weapon's style: magazine, rifle rounds, shells, rockets or belt; reserve count; the weapon's name after a
  switch), the weapon's own crosshair spread by its hip spread, the `damage_feedback` hitmarker, damage direction arcs,
  the pulsing low health overlay, obituaries with kill and headshot icons, the "+10" for a kill, team icons, scores and
  the clock, and "Killed by" on death. It hides under the in-game menus. A `--map` match loads the HUD's assets in the
  background, so it appears a few seconds in.
* **Combat**: hitscan with damage falloff, spread and bloom, recoil, reloads, head/torso/leg hit locations, health
  regeneration, deaths and respawns at map spawn points. Friendly fire is off.
* **Private Match** (in place of Start New Server): a lobby with the map (picture and name, any of the 21 stock
  maps), game type (Team Deathmatch), time and score limits and bot difficulty on the left, and the two teams on the
  right as in a console lobby: switch sides, "+ Add Bot" for either team, click a bot to take it out
  (`crates/game/src/ui/lobby.rs`).
* **Bots** (`crates/game/src/bots`) try to play like people:
  - mixed skill, like a public lobby: the lobby's Bot Difficulty (or `--skill`) is the middle, and each bot plays a
    difficulty either side about half the time (Recruit to Veteran), dealt so the two sides come out even. Skill
    sets their aim, reactions and noticing, and their game sense too: better bots pre-aim at head height where you'd
    appear, check more angles, strafe more in fights, get out of fights they're losing and reload only when it's
    quiet. Each shows a rank to go with it on the scoreboard. In test matches kills per
    death went from about 0.8 below 0.4 skill to 2 above 0.8;
  - a navigation graph flood-filled over the map's collision, with routes that prefer cover, and climbing the map's
    mantles (up onto crates and ledges, over walls) where that's the shorter way;
  - frag grenades (`bots/grenade.rs`): now and then one at an enemy they know is just behind cover, on an arc worked
    out to get there with nothing in the way, landing nearer the better the bot, and cooked by better bots so it goes
    off as it lands, and a flashbang or stun grenade before pushing in on someone round a corner; running from a live
    frag that lands near them if they spot it (their own always). Flashbanged they see nothing until it fades, stunned
    they move and turn slowly. In test matches frags made about 5% of kills, as in a public match;
  - CoD4 classes (`bots/class.rs`): their gun with the attachment most players put on that kind, a pistol, three
    perks (Stopping Power for most; Martyrdom and Last Stand mostly for weaker bots, as in public matches), a special
    grenade and perk-1 equipment, camouflage more often up the ranks (gold for the odd top one); and using the
    equipment (`bots/equipment.rs`): a claymore at the spot they hold facing the way they watch, an RPG at someone just
    spotted at range or at an enemy helicopter, C4 thrown round a corner at someone and set off a moment later. Enemy
    claymores they spot (better bots more surely, Bomb Squad through walls) are routed round; an enemy helicopter in
    sight gets shot at when nobody on the ground is close; downed in Last Stand they fight on prone with the pistol.
    In free-for-all everyone is an enemy (no teammates, no callouts). They knife whoever is right in front of them
    (nearly always with an empty gun);
  - the game modes' objectives (`bots/objective.rs`): in Domination they take the flags their team doesn't hold and keep
    theirs under attack, spread over them (one takes a flag, two keep one), and search a contested flag for whoever's
    hiding in it; the rest fight round the flags in play, as real players did in demos of Domination on District,
    Broadcast and Wet Work (`tools/dom_play.py`: a third of their time within 600 units of a flag, 2-9% on one, in
    visits of 2-3 s, mostly alone), so holding spots and hunts near those flags are favoured. An objective a bot
    finds no route to it leaves for 20 s rather than trying the same route again; in Search and
    Destroy the attackers take the bomb to the round's site and plant it (the nearest picks up a dropped one, the rest
    escort and then guard it), and the defenders split over the sites and, once it's planted, go for it, the nearest
    defusing (in Headquarters and Sabotage they don't play the objective yet: they
    fight as in Team Deathmatch);
  - kill streaks (`bots/hardpoints.rs`): a UAV or helicopter called in once out of a fight, an airstrike on where they
    know enemies are bunched up (never near themselves); and while their team's UAV is up they know where every enemy
    was at each sweep, as a player sees them on the compass;
  - perception that takes time to notice enemies (slower at the edge of view and at range), hears gunfire and
    footsteps, remembers, and shares callouts; hit from out of view, or hearing shots or footsteps behind them,
    they flick round after a reaction time to where the enemy would appear and shoot quickly if they find them
    (turn times match real players' in the demos: `tools/reactions.py`);
  - aim from a model of a hand on a mouse (reaction time, Fitts'-law flicks that over- and undershoot, corrections,
    laggy tracking, delayed recoil control), so no lock-on;
  - tactics chosen by personality, health and ammo: fight, take cover, investigate, flank, hold an angle, hunt, with
    crosshair placement on where an enemy would appear;
  - their own guns by taste (rushers SMGs and shotguns, patient players rifles, machine guns and the odd sniper
    rifle), with the fighting range and use of sights to match;
  - waiting the way real players in the demos did: holds of a few seconds (never the half-minute camps they used to),
    crouched about a third of the time and prone now and then (only where they can still see what they watch),
    pausing to check angles on the way, crouch-walking up to enemies sometimes, sidestepping rather than hopping when
    blocked, and walking round rather than jumping up onto things;
  - optionally a recorded player's habits (`--bot-profile`), checked by recording bots with a profile and fitting
    the profile back from them;
  - motion matching from real matches: CoD4 demos (`.dm_1`, recorded with `/record`) in the install's `main\demos`
    folder are read at startup (`iw3::demo`) and cut into clips of real players walking and looking around; bots walking
    a route play the clip that best fits their situation. More demos (Team Deathmatch especially) mean more natural
    bots. Server bots (`[BOT]...`, `bot0`...) in a demo are left out;
  - map knowledge from the same demos (`bots/learned.rs`): where real players walked shapes each map's lanes, and the
    places they stopped to hold become the bots' holding spots, each watching the directions real players watched
    from there. Maps without demos fall back to spots worked out from the map's geometry; scored against the demo
    maps' real holds (`COD4RW_HOLDFIT`, `tools/fit_holds.py`, `tools/tune_holds.py`, each map judged by weights fitted
    on the others), those find about 60% of real holding time, and neither a fitted model nor tuned weights did
    better on maps they weren't fitted on, so the worked-out score stays as it is;
  - the navigation graph (`bots/nav.rs`) counts a spot as standable if a player fits above step height (stairs left
    District's flag C and its street cut off from the rest, and Domination bots stood still for want of a route);
    `COD4RW_SIM` runs log whether each objective can be reached from the spawns and back, and where the graph's
    parts come closest; `COD4RW_NAVEXPLAIN=x,y,z;...` logs how points link, `COD4RW_FLOORLINE=x,y,dx,dy,n` the floor
    along a line.
* **TDM**: scoring to 75 kills or 10 minutes, a scoreboard, and match restart. Spawns are CoD4's (`_spawnlogic.gsc`,
  read from `common_mp`): the team's start points for the first 15 s, then the TDM spawn nearest the team and furthest
  from the enemy (enemies' distances less twice teammates'), never one an enemy can see, near a live grenade, your last
  one or one an enemy just used nearby (`combat::pick_spawn`; free-for-all keeps everyone about 1600 units away, and
  Domination favours the spawns by the team's flags). `iw3 --example rawfile common_mp <name> [dir]` extracts CoD4's
  scripts for checking such rules.

### Map support

Every multiplayer map's zone parses (`cargo run --release -p iw3 --example mapcheck`). Collision is the world's
own brushes, terrain and patches; brush entities (triggers, objective models) are stored relative to their entity
and left out (one of them is a slab across all of `mp_crossfire`). Water (`wc_water`) is drawn as IW3's
`water_l_sun`: animated waves reflecting the map's probe through a fresnel term, tinted by the material's water
colour, with a sun glint (its colour map is a placeholder, the blue checker that showed before).

## Not yet

These are the natural next milestones, roughly in order:

1. An FX asset loader (muzzle flashes, impacts) and sounds.
2. Grenades, equipment and the remaining perks.
3. Networking (client/server with prediction).
4. Other modes (DOM, S&D, HQ), perks and killstreaks.

## Layout

```
crates/iw3     format library: fastfile, zone parser, iwd, iwi, entity strings
crates/t4      World at War (T4) readers: zones, weapon files, conversion to iw3 assets
crates/t5      Black Ops (T5) readers: zones, weapon files, conversion to iw3 assets
crates/game    the Bevy game
crates/net     server query protocol (Quake 3/CoD4 connectionless packets) and the master server
```

`cargo run -p iw3 --example zoneinfo -- mp_crash` prints what a zone contains, which is handy when adding maps.

### Black Ops content (`crates/t5`)

Black Ops' multiplayer weapons, attachments and characters are loaded from a local Black Ops install (found
automatically or via `BLACKOPS_PATH`); the game offers them when an install is present:

* Every Black Ops multiplayer zone parses completely (`common_mp`, `code_post_gfx_mp` and all 26 maps), through a
  schema-driven loader generated from OpenAssetTools' T5 definitions (`python tools/gen_schema.py <dir>
  crates/t5/src/zone/schema.json T5_Assets.h`).
* `t5::load_iw3(&install, "common_mp")` returns its models, animations, materials and images as an `iw3::zone::Zone`,
  the same types the game's model pipeline and `iw3::xanim` use for CoD4 (all 3355 animations decode); textures are
  `.iwi` version 13 files in `install.vfs()`, which `iw3::iwi` reads.
* `t5::weapons` reads the plain-text weapon files (`weapons/mp/ak47_reflex_mp`: stats, models, animations,
  `hideTags`). Attachments work like CoD4's: one gun model per weapon, each variant hiding the parts it lacks.
* `t5::catalog` lists what is ready: 39 guns with their attachments (sights, suppressor, extended and dual mags, grenade
  launcher, flamethrower, Masterkey, grip, rapid fire, variable zoom, dual wield, ...), the view hands (materials in
  `code_post_gfx_mp`), and eight factions of 5 bodies and 5 heads each (CIA and Spetsnaz from `mp_nuked`, their
  winter gear from `mp_array`, SOG and NVA from `mp_cracked`, Cuban rebels and Tropas from `mp_firingrange`).

Checks: `cargo run --release -p t5 --example catalog` loads all of it and verifies every model and texture;
`--example anims` decodes every animation; `--example render -- <dir>` renders a sample (guns with attachments, hands,
characters) to PNGs; `--example zonestats`, `models` and `inspect` look inside zones. Not done yet: the wood/camo
detail textures some guns blend over their colour map.

### World at War content (`crates/t4`)

World at War's multiplayer guns, attachments, equipment and characters are loaded from a local World at War install
(found automatically or via `WAW_PATH`); the game offers them when an install is present. Same design as `crates/t5`:

* Every World at War multiplayer, campaign and zombies zone parses completely (`common_mp`, all 23 maps, their
  `localized_mp_*` team zones, the 15 campaign levels and the four zombies maps), through the schema-driven loader
  generated from OpenAssetTools' T4 definitions (`python tools/gen_schema.py <dir> crates/t4/src/zone/schema.json
  T4_Assets.h`, the dir holding `T4_Assets.h`, `T4_Commands.txt` and `XAssets/*.txt`).
* `t4::load_iw3(&install, "common_mp")` returns models, animations, materials and images as an `iw3::zone::Zone`.
  World at War is CoD4's engine one step on: animations, images and vertices are byte-for-byte CoD4's (all 1223
  `common_mp` animations decode with `iw3::xanim`), textures are CoD4 `.iwi` version 6 files in `install.vfs()`.
* `t4::weapons` reads the plain-text weapon files (`weapons/mp/thompson_aperture_mp`); attachments are CoD4-style
  `hideTags` on one view model per gun, and silencer/bayonet/flash variants also name their own world model.
  Animation names in weapon files are not always the zone's case (`viewmodel_Walther_p38_idle`): look them up
  case-insensitively.
* `t4::catalog` lists what is ready: 27 guns with their attachments (aperture and telescopic sights, scopes, rifle
  grenades, bayonets, suppressors, drum mags, flash hiders, bipods, grips, sawn-off), 12 pieces of equipment (bazooka,
  M2 flamethrower, frag/sticky/molotov/smoke/tabun grenades, signal flare, satchel charge, Bouncing Betty), and 20
  characters: body, head and first-person arms for each class (rifle, cqb, assault, lmg, smg) of the four factions.
  World at War's view hands belong to the character (`setViewmodel` in `character/*.gsc`), not the gun. Marines and
  Japanese are in `localized_mp_makin`, Red Army and Wehrmacht in `localized_mp_dome` (every Pacific/European map's team
  zone carries the same set).

* `t4::campaign` lists the campaign's 113 characters from the scripts in the 15 levels (`mak` .. `ber3b`): the named
  cast (Roebuck, Sullivan, Polonsky, Reznov, Chernov, Ansel, ...), the co-op player characters, and every soldier type
  of the Marines/Raiders, Navy and PBY crews, Red Army, Imperial Japanese Army and Wehrmacht/Honor Guard, dry and wet.
  Each `Look` is a body plus attached head, hat and gear; soldier types list every model the game picks from at random
  (`xmodelalias` lists), and each character names the level that holds all of its models. Attached parts are modelled
  around their own root bone: place them on the body's bone of the same name.

* `t4::ui::ui_data(&zone)` returns menus, fonts, localized strings and string tables as the game's own `iw3::menu`
  types (World at War's structures are CoD4's plus a few fields; expression opcodes match up to 82). The
  multiplayer frontend is spread over `t4::ui::MP_ZONES`: the main menu (`main_text`), Create a Class and the rest in
  `ui_mp`, in-game menus in `common_mp`, the nine fonts and most strings in `localized_code_post_gfx_mp`.
* `t4::sound` reads sound aliases (`aliases(&zone)`: variations with volume, pitch, distance, looping, positional or
  not, bus and falloff curve) and the mixer (`Mixer::from_zone` on `code_post_gfx_mp`: World at War's buses
  replace CoD4's channels; an alias's bus is `flags >> 22`). Sounds are WAV files in the zones or in
  `sound/...` in the iwds, almost all MS-ADPCM: `decode_wav` turns them into 16-bit PCM WAVs Bevy can play.
  `music_files` lists the 269 tracks in `sound/stream/music` (music aliases: `music_mainmenu`, `mp_victory_*`,
  `mp_spawn_*`, `mp_suspense_*`, ...). Not decoded: xWMA files (192 in-zone sounds, mostly map ambience, distant
  explosions and debris, and one music file).
* `t4::hud` sorts the HUD art by role (`Hud::build` over material names; groups `crosshair`, `hitmarker`,
  `damage`, `scope`, `minimap`, `weapon_icons`, `kill_icons`, `ammo`, `stance`, `perks`, `ranks`, `teams`,
  `objectives`, `killstreaks`, `load_screens`, ...): the shared art is in `t4::hud::HUD_ZONES`, each map's minimap
  (`hud::minimap("mp_airfield")`) in its zone. `Crosshair::of(&weapon_file)` gives a weapon's reticle pieces and
  sizes, scope overlay, and HUD and kill icons.

Checks: `cargo run --release -p t4 --example catalog` loads all of it and verifies every model, texture (300/300) and
gun animation (510/510); `--example campaign` does the same for the campaign characters (505 models, 844/844
textures); `--example render -- <dir> [--campaign [name...]]` renders a sample to PNGs; `--example ui` checks the menus,
fonts and their art; `--example sounds -- [dir]` decodes every multiplayer sound and music file (and writes a few
samples); `--example ui_preview -- <dir>` draws the HUD art, the fonts and the main menu to PNGs;
`--example zonestats`, `models`, `inspect`, `anims` and `rawfiles` look inside zones.

### Debugging aids

Any `COD4RW_*` variable (other than `COD4RW_UNLOCKS`) makes a debug run, which neither reads nor saves your
`stats.txt` (`COD4RW_STATSFILE=<file>` reads one) and has everything unlocked unless `COD4RW_UNLOCKS=cod4`.

* `COD4RW_SHOT=<dir>`: save four screenshots after loading, then exit.
* `COD4RW_SIM=<seconds>`: log every pawn's state every 5 s, then exit.
* Bots against real players: dump a demo's players with `cargo run --release -p iw3 --example demodump -- <demo>
  <dir>`, record every bot in a sim (`COD4RW_SIM=240 COD4RW_RECORD=<dir2>/bots.csv COD4RW_RECORD_WHO=all ...
  --spectate`), then `python tools/compare_play.py --real <dir> --bots <dir2>` prints movement, view, stance and
  aiming-down-sights measures side by side, flagging the bots' figures outside the range real players span.
* `COD4RW_MOVETEST=1`: run a scripted movement test and log the measured speeds.
* `COD4RW_LIPTEST=<cases.txt>`: walk the player over each low obstacle in the file (`iw3 --example lips <map> <file>`
  writes them) and log how many it got over; with `COD4RW_LIPTEST_JUMP=1`, hold jump at each mantle volume instead
  (`iw3 --example mantles <map> <file>`).
* `COD4RW_LOADOUT=<gun spec>[@camo]`: start with that primary (`ak47@6` is the gold AK-47, `t5_ak47:reflex@115` a
  gold Black Ops AK-47 with a reflex sight).
* `COD4RW_NOSHINE=1`: guns without their specular reflections, for comparison.
* `COD4RW_CLUTTERTEST=<dir>` (with `COD4RW_SPAWN` by some clutter): shoot every piece of the map's clutter in view
  within 12 m, then set off a blast 3 m ahead, screenshotting before and as things fly and settle; logs what moved.
  `iw3 --example dynents <map> [filter]` lists a map's clutter and breakables with their physics presets.
  `iw3 --example impactaudit <map>` lists the bullet impact effects by surface and which impact sounds exist;
  `iw3 --example terrainsurf <map>...` what each map's terrain and patch collision is made of.
* `COD4RW_NOMAPFX=1`: without the map's ambient effects (its createfx smoke, fires, dust and birds), for comparison.
* `COD4RW_GUNLOOK=cod4`: guns as CoD4 drew them, for comparison. By default they're normal-mapped (as characters
  are), their camos have more contrast and colour, and their specular reflection is stronger.
* `COD4RW_VMTEST=<dir>`: screenshot the viewmodel idle, firing, in ADS, reloading, sprinting (in, loop, out),
  walking and inspected (each side), then exit.
* `COD4RW_VMANIM=<xanim>`: hold one viewmodel animation at 0.3x speed.
* `COD4RW_FRAMES=<dir>`: save every frame from 10 s (`COD4RW_FRAMES_COUNT`, default 30) with the view held still,
  then exit: for finding what flickers.
* `COD4RW_BUGTEST=<text>`: file an F10 bug report with that text a few seconds in (through the real keyboard path),
  then exit; `COD4RW_BUGTEST_SHOT=<file.png>` also saves a picture of the prompt.
* `COD4RW_LEAVETEST=<dir>`: leave the match after 8 s, start another from the menu, screenshot both (`menu.png`,
  `second_match.png`) and exit; the log counts entities at each step.
* `COD4RW_DUMMY=1`: spawn a lineup of frozen character models in front of the player.
* `COD4RW_NOVISION=1`: leave out the map's film grading, the sun's flare/blind/glare and the light grid tweaks, to
  compare. `COD4RW_SUNTEST=<dir>`: look into the map's sun, screenshot it (`sun.png`) and 15 degrees beside it
  (`beside.png`), then exit. `COD4RW_NVTEST=<dir>`: switch night vision on and off, screenshotting the fades, then
  exit.
* `COD4RW_PERF=1`: log frame times every 5 s in a match (average, median, p95, worst; real time), with entity counts
  and the GPU's time per render pass, and the run's totals on exit. It can also take comma-separated things to leave
  out, to measure what they cost: `novsync`, `noshadows`, `nossao`, `noviewmodel`, `nofx`, `cascades2`,
  `nopropshadows`, `noprops`. For per-system times, build with `--features bevy/trace_chrome` (the trace lands in the
  working directory, ~100 MB a second with 32 pawns).
  `COD4RW_SHADOWTEST=<dir>`: screenshot the same three views (player held at its spawn) under each sun shadow setting
  in `perf::SHADOW_TRIALS`, then exit.
* Physics is avian for colliders and spatial queries only (nothing is a rigid body; `main.rs`'s `physics_plugins`
  leaves out the solver, contacts, joints and interpolation). `COD4RW_FULL_PHYSICS=1` runs all of avian to compare;
  `COD4RW_PHYSICS_STRICT=1` makes avian's physics schedule ambiguity check an error again.
* `COD4RW_GRENADETEST=<dir>`: cook a frag from 6 s and throw it, screenshotting each part of the throw and the blast,
  then exit. `COD4RW_GRENADETEST_SPECIAL=flash|stun|smoke` throws that special grenade instead;
  `COD4RW_GRENADETEST_PITCH=<radians>` aims it (default -0.25; -1.2 lands at your feet).
* `COD4RW_EQUIPTEST=<dir>` (with `COD4RW_LOADOUT=m16` and `COD4RW_INVENTORY=rpg_mp|c4_mp|claymore_mp`, or
  `COD4RW_LOADOUT=m16:gl`): press 5 at 5.5 s, fire at 7 s, aim (detonating C4) at 9 s, screenshotting each step,
  then exit.
* `COD4RW_PERKS=specialty_a,specialty_b` (with `COD4RW_LOADOUT`): give the debug class those perks.
  `COD4RW_LASTSTANDTEST=<dir>` (with `specialty_pistoldeath`): a bullet would kill you at 6 s; screenshots downed
  and after bleeding out, then exits.
* `COD4RW_CHALLENGETEST=<dir>`: the gun in hand's Marksman and Expert challenges are a kill from done and you
  get a headshot kill with it at 5 s; screenshots of the notices, the unlocks logged, then exit.
* `COD4RW_KILLCAMTEST=<dir>`: you run, turn and jump from 3 s, then an enemy "kills" you at 8 s; screenshots of the
  killcam and after, then exit. `COD4RW_KILLCAMTEST_FINAL=1` makes that kill end the match (the final killcam);
  `COD4RW_KILLCAMTEST_ORBIT=1` watches your replayed body from behind instead of the killer's eyes.
* `COD4RW_THIRDPERSON=1`: start in third person. `COD4RW_CAST=<zone>`: the bots' characters come from that zone
  (e.g. `fullahead`, `mp_nuked`) besides the map's own. Background sims (`COD4RW_SIM`) leave bots in their team's
  models.
* `COD4RW_HUDTEST=1` (with `COD4RW_DUMMY`): 5 s in, the player fires at the friendly dummy (no hit marker); 5.6 s
  in, an enemy dummy wounds the player, fires and steps into the crosshair (red), and the player kills another with a
  headshot (the enemy leads by two kills); 6.1 s in, the player fires at that body (no hit marker). For screenshots of the HUD's hitmarker, damage
  arc, low health overlay, obituary and "+10"; the player's shots and hit confirms are logged.
* `COD4RW_GALLERY=<dir>`: render every character CoD4's character scripts define (multiplayer and campaign), twelve
  to a page with their model names, as PNGs in `<dir>`, then exit. `COD4RW_GALLERY_GAME=blackops` renders Black Ops'
  instead (from its install, through `crates/t5`). `COD4RW_GALLERY_ZONES=a,b` limits it to those zones and
  `COD4RW_GALLERY_POSE=<anim>` poses them with another animation.
* `COD4RW_SPAWN=x,y,z,yaw`: start the player at a fixed spot (CoD units and degrees), for comparable screenshots.
* `--spectate <bot name>` (or `COD4RW_SPECTATE`): start out following that bot; add `COD4RW_SPECTATE_SHOTS=<dir>` to
  save frames of its fights.
* `COD4RW_RECORD=<file.csv>`: like `--record`, to a given file: your view, inputs, position, stance and the enemy
  nearest your crosshair every frame, plus shots, hits and kills (`COD4RW_RECORD_WHO=<bot name>` records a bot, `all`
  every bot to `<file>-<name>.csv`). `python tools/fit_aim.py <file.csv>` looks at aim alone.
* `COD4RW_NAVDUMP=<file.csv>`: write the bots' navigation points.
* `COD4RW_UISHOT=<dir>`: screenshot the main menu, then each step in `COD4RW_UIMENUS` (comma separated: a menu to
  open, `menu@dvar=value` to also set dvars, `#Label` to click an item, `~pixels` to drag a gun preview), then start a
  match and screenshot each step in `COD4RW_UIGAME` (`#Label` clicks, `key:Digit2` presses a key, `hold:Tab` holds one (`hold:MouseRight`
  aims) through its screenshot, anything else waits). Splitscreen players' menus: `p2#Label` clicks in Player 2's,
  `p2:key:Enter` / `p2:key:Escape` / `p2:dir:down` are their controller's A, B / Menu and D-pad.
  Stats are not saved and the sound is muted in this mode; `COD4RW_STATSFILE=<file>` reads stats from a file instead.
* `COD4RW_SDBOMB=1`: in Search and Destroy and Sabotage, the player starts the first round carrying the bomb (with
  `COD4RW_SPAWN` at a target, to test planting). In Headquarters, `COD4RW_SPAWN` makes the first HQ the nearest.
* `COD4RW_BIPODSCAN=1`: 3 s into a match, log whether the player faces a ledge a bipod rests on, and up to twelve
  standing spots nearby that do, as `COD4RW_SPAWN` values.
* `COD4RW_WALK=<seconds>` (with `COD4RW_SPAWN`): Player 1 walks forward (`COD4RW_WALK_KEYS=KeyW,Space@0.05,KeyC@0.25`:
  held keys, `Key@t` tapped once at t; Space alone taps each second), logging position, speed and ground every
  frame, then exits; for movement bugs (stairs, gaps). `cargo run -p iw3 --example pathcheck|brushbox|planeat|trisat
  -- <map> ...` find the collision at a spot.
* `COD4RW_SPLITSCREEN=kbm,pad,...`: like `--splitscreen`. `COD4RW_SPLITTEST=<dir>` (with it): virtual controllers
  play Players 2 to 4 (Player 2 walks and turns, 3 fires and aims, 4 crouches, stands and throws a frag) while each
  player's state is logged and the window screenshot, then exit.
* `COD4RW_PADTEST=<dir>`: a virtual controller plays the steps in `COD4RW_PADSTEPS` (comma separated, one every
  0.7 s from 3 s in: a button to tap such as `a`, `lt`, `down`, `menu`; `+rt`/`-rt` to hold and let go; `ls:0;1` or
  `rs:0.5;0` to set a stick; `wait:<seconds>`; `shot:<name>` to save `<dir>/<name>.png`), then exits. Use with or
  without `--map`; `COD4RW_PAD=ps` makes it a PlayStation pad. Stats are not saved and the sound is muted.
* Supply drops in debug runs: `COD4RW_SUPPLYDROPS=<n>` starts with n to open, `COD4RW_SUPPLYTIME=<seconds>` earns one
  that often, `COD4RW_SUPPLYFILE=<file>` reads a collection (`supply.txt` format). Debug runs never save it.

Any other `COD4RW_*` variable (bar the `COD4RW_PAD*` ones) starts a match directly, skipping the menus.
* `IW3_TRACE=1`: log each top-level asset as it is parsed.
* `T5_TRACE=1` / `T5_TRACE_STRUCTS=1`: the same for Black Ops zones, and every struct the loader visits.

## Credits

The IW3 struct layouts were derived from the community's reverse-engineering work, especially
[OpenAssetTools](https://github.com/Laupetin/OpenAssetTools). This project writes its own code and only uses those
definitions as documentation.
