//! Prints where each Call of Duty install was found (`cargo run -p gamepaths --example status`).
fn main() {
    println!("Steam libraries: {:?}", gamepaths::steam_libraries());
    for g in gamepaths::Game::ALL {
        println!("{:<33} {:?}", g.title(), gamepaths::find_with_source(g));
    }
}
