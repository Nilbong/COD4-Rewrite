//! Bot lab experiments (`tools/botlab.py`): behaviour changes under trial,
//! switched on by name with `COD4RW_BOTLAB=<name>[,<name>...]`, so a change
//! and the game without it are measured from the same build. One that
//! proves itself loses its switch and becomes how bots play.

use std::sync::OnceLock;

/// Is the experiment `name` switched on?
pub fn on(name: &str) -> bool {
    static ON: OnceLock<Vec<String>> = OnceLock::new();
    ON.get_or_init(|| {
        let list: Vec<String> = std::env::var("COD4RW_BOTLAB").unwrap_or_default().split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect();
        if !list.is_empty() {
            log::info!("bot lab experiments: {list:?}");
        }
        list
    })
    .iter()
    .any(|n| n == name)
}
