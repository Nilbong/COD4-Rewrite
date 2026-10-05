# Multiplayer foundation

An isolated Rust crate with working relay-only QUIC connections and reusable
netcode. It has its **own workspace, lockfile and build directory** so it can
be developed while map/graphics work continues. No existing game files have
been changed. The Bevy game does not use this crate yet.

## Architecture

```text
Player 1 / match host <-- outbound QUIC --> Trusted relay
Player 2              <-- outbound QUIC -->     |
Player 3              <-- outbound QUIC -->     |
```

The player host runs the authoritative game simulation. Other players submit
controls; they cannot submit position, damage, health or a claimed sender ID.
The relay assigns IDs from authenticated room membership and forwards inputs
to the host. Only that host can send authoritative snapshots to its players.
The same game authority can eventually run on a dedicated server.

All players make outbound UDP/QUIC connections to infrastructure, so normal
home networks do not require port forwarding. There is no hole punching,
direct-connect fallback, address exchange, or mesh of peer sockets. Some
networks block UDP; this version fails instead of leaking a direct address.

The relay knows connection IPs and can inspect traffic: TLS encrypts each
player-to-relay hop, not an additional end-to-end layer through the relay.
The relay is therefore trusted infrastructure. It never puts player addresses
in the wire protocol or prints them in the relay CLI.

## What is implemented

- TLS 1.3 QUIC using Quinn/rustls, strict trust of an explicitly supplied
  relay certificate, certificate-name validation and a dedicated ALPN.
  No certificate-verification bypass or application 0-RTT.
- Private unlisted rooms with a relay-generated, 256-bit invitation secret.
  Secrets are redacted from Debug output. Only the host receives the secret
  in its welcome; a joining player receives no other players' secrets.
- Up to 18 participants including the host; stable peer IDs are not reused
  while a room exists. Host disconnect closes the room and invalidates invites.
- Reliable ordered join/leave events on one control stream; replaceable
  input/state updates use QUIC datagrams so a lost update doesn't block newer
  updates. Wire versioning, packet size caps and checks for invalid fields,
  unsupported message types, duplicate pawn IDs, and trailing bytes.
- Input redundancy (last three commands), duplicate/replay filtering,
  bounded command queues, fixed-rate host input consumption, prediction
  reconciliation by replaying unacknowledged commands, remote interpolation
  including angle wrap, and no interpolation across deaths/teleports.
- Initial policy of 60 simulation ticks/s and 20 snapshots/s. Commands do not
  choose the simulation timestep. Held input expires after 100 ms without
  commands; discrete actions are not repeated on a missing packet.
- Per-connection packet/byte limits, QUIC address retry, admission rate cap,
  five-second handshake deadlines, 128 concurrent connections, eight per IP,
  32 rooms, small receive windows and bounded control queues. Slow control
  consumers are disconnected rather than accumulating unlimited messages.
- `hosting::choose_host`: pick a low-latency, stable candidate with enough
  measured upload and CPU capacity. This is an election policy, not a running
  matchmaking/measurement service or automatic host migration.
- A standalone relay executable, a real encrypted loopback movement demo,
  separate-process host/player demos, unit tests and loopback integration tests.

The snapshot format currently carries **up to 18 pawn states total**, including
bots. Each pawn includes position/velocity in Bevy metres, yaw/pitch in radians,
health, stance and life state. State packets fit within 1100 bytes without
application fragmentation. Full weapon state, projectiles, teams, scoring and
reliable gameplay events need a versioned protocol extension during integration.

## Run from the repository root

```powershell
cargo test --manifest-path crates/multiplayer/Cargo.toml --all-targets
cargo run --manifest-path crates/multiplayer/Cargo.toml --bin net-demo -- loopback
```

The loopback demo uses an in-memory development certificate, opens sockets
only on localhost, sends 120 controls through a real relay, returns 40 host
snapshots, and verifies prediction converges. It does not need game assets or
write a certificate to disk. It is a network test, not a playable CoD4 match.

To test different machines/processes, generate a development identity once:

```powershell
cargo run --manifest-path crates/multiplayer/Cargo.toml --bin cod4rw-relay -- identity crates/multiplayer/identity
cargo run --manifest-path crates/multiplayer/Cargo.toml --bin cod4rw-relay -- serve crates/multiplayer/identity
```

The relay binds `127.0.0.1:28970` by default. An explicit bind address is needed
for another machine, e.g. `0.0.0.0:28970`; any firewall rule belongs on the
**relay machine**, not the player host. No public deployment is made by these
commands. Relay operation continues until the process is stopped.

In another terminal, start a demonstration host:

```powershell
cargo run --manifest-path crates/multiplayer/Cargo.toml --bin net-demo -- host 127.0.0.1:28970 crates/multiplayer/identity/relay.der
```

It deliberately displays a room ID and invite. Keep the invite private. Another
terminal/machine can join using those values:

```powershell
cargo run --manifest-path crates/multiplayer/Cargo.toml --bin net-demo -- join 127.0.0.1:28970 crates/multiplayer/identity/relay.der <room-id> <invite>
```

For remote machines, replace localhost with the **relay address**, never the
host's home address. Send the public `relay.der` through a trusted channel.
Do not distribute `relay-key.der`. The key uses restrictive permissions on
Unix; on Windows use a directory with a private ACL for the relay service
account. Generated identity files are ignored by this crate's .gitignore.
CLI invites are convenient for development but can appear in shell history or
process arguments; a real launcher should hold them in memory instead.

## Game integration contract

1. Add the crate to the parent workspace and the game's dependencies when
   concurrent work permits; remove its standalone `[workspace]` at that point.
   Keep the old `cod4rw-net` discovery crate separate: its protocol advertises
   raw server addresses and is **not** a privacy-preserving online lobby.
   New relay matches must never advertise player hosts through that browser
   (`COD4RW_ADVERTISE` must remain off), local broadcasts or direct favourites.
2. Add a `MultiplayerPlugin` with an I/O task running `Session` and bounded
   channels into Bevy. Drain control/packet queues without blocking the render
   thread. Pick/map stable network IDs; never serialize Bevy Entity IDs.
3. Host runs all `movement::pmove`, weapons, hit detection, damage, bots,
   match modes, score and respawns at a fixed 60 Hz. Refactor the existing
   variable-frame Update movement and weapon timing to a shared fixed-step
   simulation function. Consume one `InputQueue::next_tick` per player each
   tick. The host's own input uses the same path locally. Do not accept speed
   scale, perks, damage multipliers or cooldowns from remote inputs.
4. Client converts `PlayerInput` into `InputCommand`. Predict only its own
   movement through the same collision/tuning/stance code; do not run the
   authoritative bot/combat/match systems. `Prediction<S>` must hold the full
   `Mover`/movement state, not just position. Apply host corrections then
   replay unacknowledged commands without replaying sounds/FX/XP side effects.
5. Host sends a recipient-specific acknowledged-input sequence in each
   snapshot, at 20 Hz. Clients discard stale snapshots before reconciliation.
   Add host-clock synchronization and use `Interpolation` for remote actors
   roughly 100 ms behind host time. Smooth local rendering separately from
   corrected simulation state. Freeze/reconnect or explicitly resynchronize
   on backlog overflow; do not silently discard reconciliation history.
6. Expand the protocol for class/equipment selection, shots, weapon state,
   explosives, scoring and round transitions. Validate any requested loadout
   against installed content and host rules; give important events stable IDs
   and reliable delivery. Never fetch an arbitrary path or executable sent by
   a host; select maps/content by validated IDs and negotiate content versions.
7. Add host-owned historical hitboxes and a bounded rewind window for shooting
   latency compensation, validating timestamps against observed latency.
   This version has no lag-compensated shooting. Test delay, jitter, packet loss,
   reconnects, joins during play, and progression attribution in actual matches.

## Security and hosting boundaries

Relaying hides players' home IPs **from other participants using this protocol**;
it is not anonymity from the relay, the ISP, or other applications. TLS protects
traffic in transit; room capabilities authorize entry, not a verified user
account. There is no account login, ban service, per-player revocable ticket,
invite rotation, anti-cheat or production matchmaking in this first version.

Host authority stops ordinary clients claiming damage/position, but a malicious
player host can still cheat. Competitive fairness eventually needs a trusted
dedicated host. No migration occurs when a host leaves: the room closes, rather
than accepting a new untrusted authority or revealing addresses.

Application rate limits do not stop a volumetric DDoS that fills the relay's
internet link. Public use needs a relay on protected hosting, provider-level
UDP DDoS mitigation, service authentication, monitoring, secret/key rotation
and a security review. Keep it a private prototype until those exist. Running
the relay on a player's PC would expose that player's IP and defeats the model.

Use measured RTT, jitter, loss, upload headroom and simulation cost for election.
Highest download speed alone is a poor criterion. Select the best candidate
before creating the authoritative room; the included policy needs a real lobby
and a measurement service before it can elect automatically.

Steam Datagram Relay is another potential infrastructure option if the project
has an eligible Steamworks integration. It also relays traffic and hides peer
IP addresses. The open-source GameNetworkingSockets library alone does not give
access to Valve's protected relay network. This crate needs no Steam app ID.

Sources: [Quinn certificate authentication](https://quinn-rs.github.io/quinn/quinn/certificate.html),
[QUIC transport limits](https://docs.rs/quinn/0.11.12/quinn/struct.TransportConfig.html),
[Steam Datagram Relay](https://partner.steamgames.com/doc/features/multiplayer/steamdatagramrelay).
