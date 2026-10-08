//! The lighting re-bake on its own (`src/bake`), for working on it without
//! building the game: rebake <map>. The game runs the same with
//! `COD4RW_BAKE=<map>`. `rebake --convert` rewrites version-1 caches in the
//! current format (keeping the old ones); `rebake --showcase <map>` bakes
//! its time-of-day keyframes.
#[path = "../src/bake/mod.rs"]
#[allow(dead_code)]
mod bake;

fn main() -> std::process::ExitCode {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "mp_crash".into());
    if arg == "--convert" {
        for (map, before, after) in bake::convert_v1() {
            println!("{map}: {before:.1} MB -> {after:.1} MB");
        }
        return std::process::ExitCode::SUCCESS;
    }
    if arg == "--showcase" {
        // Every time-of-day keyframe (`bake::tod`), logging as `bake::cli`.
        let map = std::env::args().nth(2).unwrap_or_else(|| "mp_cargoship".into());
        return bake::cli_with(&map, |map, opts| bake::showcase(map, opts));
    }
    bake::cli(&arg)
}
