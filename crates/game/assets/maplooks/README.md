# Map looks

Saved by the in-game Map Look Editor (F9): `<map>.txt` for a map's normal
look, `<map>.showcase.txt` for its showcase look. Each line is a live
tuning knob, `name = value`, applied over `tuning.txt` while that map is
played. Embedded into the game at build time (`build.rs`), so they ship.
