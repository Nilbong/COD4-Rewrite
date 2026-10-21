//! Everyone plays the host's map editor layout ([`crate::map_editor`]):
//! the same placed objects, lights, spawns and hidden map models.
//!
//! Just before telling friends to load the match (and a friend joining
//! one under way), the host sends its layout for the map, compressed, in
//! parts that fit the relay's messages ([`LobbyMsg::Layout`]). A friend's
//! game puts the parts back together and plays that layout instead of its
//! own (`layout::set_override`) before the map loads; none from the host
//! means none at all. Leaving online play puts the player's own back.

use crate::map_editor::layout;
use crate::online::lobby::{LobbyMsg, MAX_LAYOUT_PART, MAX_LAYOUT_PARTS};
use std::io::{Read, Write};
use std::sync::Mutex;

/// The most a layout may come to unpacked (it's text: a few KB).
const MAX_TEXT: u64 = 1 << 20;

/// The host's layout for `map`, as messages to send before `Start`.
pub fn messages(map: &str) -> Vec<LobbyMsg> {
    let none = || {
        bevy::log::info!("online: no layout for {map} to send: friends play it as it comes");
        vec![LobbyMsg::Layout { map: map.to_owned(), part: 0, parts: 0, data: Vec::new() }]
    };
    let Some(text) = layout::text_for(map).filter(|t| !t.trim().is_empty()) else { return none() };
    let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
    let packed = match z.write_all(text.as_bytes()).and_then(|_| z.finish()) {
        Ok(p) => p,
        Err(e) => {
            bevy::log::warn!("online: couldn't pack the layout: {e}");
            return none();
        }
    };
    let chunks: Vec<&[u8]> = packed.chunks(MAX_LAYOUT_PART).collect();
    if chunks.len() > MAX_LAYOUT_PARTS as usize {
        bevy::log::warn!("online: the {map} layout is too big to send ({} bytes packed): friends play the map without it", packed.len());
        return none();
    }
    bevy::log::info!(
        "online: sending the {map} layout: {} objects, {} bytes ({} packed, {} parts), fingerprint {:016x}",
        layout::parse(&text).len(),
        text.len(),
        packed.len(),
        chunks.len(),
        layout::hash(&layout::parse(&text))
    );
    let parts = chunks.len() as u8;
    chunks.into_iter().enumerate().map(|(i, c)| LobbyMsg::Layout { map: map.to_owned(), part: i as u8, parts, data: c.to_vec() }).collect()
}

/// The parts arriving: the map, and each part so far.
static INCOMING: Mutex<Option<(String, Vec<Option<Vec<u8>>>)>> = Mutex::new(None);

/// A part from the host. Once all of a layout's parts are here, that's the
/// layout this game plays the map with.
pub fn receive(map: String, part: u8, parts: u8, data: Vec<u8>) {
    if parts == 0 {
        bevy::log::info!("online: the host has no layout for {map}: playing it as it comes");
        layout::set_override(Some((map, String::new())));
        return;
    }
    let Ok(mut incoming) = INCOMING.lock() else { return };
    // A new layout (another map, another count): start over.
    if incoming.as_ref().is_none_or(|(m, p)| *m != map || p.len() != parts as usize) {
        *incoming = Some((map.clone(), vec![None; parts as usize]));
    }
    let Some((_, have)) = incoming.as_mut() else { return };
    if let Some(slot) = have.get_mut(part as usize) {
        *slot = Some(data);
    }
    if have.iter().any(Option::is_none) {
        return;
    }
    let packed: Vec<u8> = have.iter().flatten().flatten().copied().collect();
    *incoming = None;
    let mut text = String::new();
    match flate2::read::DeflateDecoder::new(&packed[..]).take(MAX_TEXT).read_to_string(&mut text) {
        Ok(_) => {
            bevy::log::info!(
                "online: playing the host's {map} layout: {} objects, fingerprint {:016x}",
                layout::parse(&text).len(),
                layout::hash(&layout::parse(&text))
            );
            layout::set_override(Some((map, text)));
        }
        Err(e) => {
            // (Better the bare map than half a layout.)
            bevy::log::warn!("online: the host's {map} layout didn't unpack ({e}): playing it as it comes");
            layout::set_override(Some((map, String::new())));
        }
    }
}

/// Out of online play: the player's own layouts again.
pub fn clear() {
    layout::set_override(None);
    if let Ok(mut i) = INCOMING.lock() {
        *i = None;
    }
}
