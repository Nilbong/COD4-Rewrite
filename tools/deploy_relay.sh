#!/usr/bin/env bash
# Builds the online relay (crates/multiplayer, `cod4rw-relay`) on the relay
# server and (re)starts it as the `cod4rw-relay` systemd service.
#
#   tools/deploy_relay.sh [root@host]        (default root@167.233.105.90)
#
# Only the relay's own source and the workspace's Cargo.lock are sent; the
# build happens on the server (so its CPU type doesn't matter). The first run
# also creates the relay's identity on the server (/var/lib/cod4rw-relay/id:
# relay.der, the public certificate the game trusts, and relay-key.der, the
# private key, which never leaves the server) and copies relay.der back to
# data/relay/relay.der here.
set -euo pipefail
HOST="${1:-root@167.233.105.90}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

mkdir -p "$STAGE/crates"
cp -r "$ROOT/crates/multiplayer" "$STAGE/crates/"
cp "$ROOT/Cargo.lock" "$STAGE/"
cat > "$STAGE/Cargo.toml" <<'TOML'
[workspace]
resolver = "3"
members = ["crates/multiplayer"]

[workspace.package]
edition = "2024"
version = "0.1.0"
license = "MIT OR Apache-2.0"

[profile.release]
codegen-units = 1
lto = "thin"
TOML
tar -C "$STAGE" -czf "$STAGE/relay-src.tgz" Cargo.toml Cargo.lock crates
scp -q "$STAGE/relay-src.tgz" "$HOST:/tmp/relay-src.tgz"

ssh "$HOST" 'bash -s' <<'REMOTE'
set -euo pipefail
rm -rf /opt/cod4rw-relay-src && mkdir -p /opt/cod4rw-relay-src
tar -C /opt/cod4rw-relay-src -xzf /tmp/relay-src.tgz
cd /opt/cod4rw-relay-src
/root/.cargo/bin/cargo build --release --bin cod4rw-relay 2>&1 | tail -3
install -m 0755 target/release/cod4rw-relay /usr/local/bin/cod4rw-relay

# The identity, made once and kept.
if [ ! -f /var/lib/cod4rw-relay/id/relay.der ]; then
    sudo -u relay /usr/local/bin/cod4rw-relay identity /var/lib/cod4rw-relay/id
fi

cat > /etc/systemd/system/cod4rw-relay.service <<'UNIT'
[Unit]
Description=cod4rw online relay
After=network-online.target
Wants=network-online.target

[Service]
User=relay
Group=relay
ExecStart=/usr/local/bin/cod4rw-relay serve /var/lib/cod4rw-relay/id 0.0.0.0:28970
Restart=always
RestartSec=2
# Hardening: the relay only needs its identity and the network.
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadOnlyPaths=/var/lib/cod4rw-relay
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectControlGroups=true
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
LockPersonality=true
MemoryDenyWriteExecute=true
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable --now cod4rw-relay >/dev/null 2>&1
systemctl restart cod4rw-relay
sleep 1
systemctl --no-pager --lines=3 status cod4rw-relay | sed -n '1,3p;/Relay listening/p'
REMOTE

mkdir -p "$ROOT/data/relay"
scp -q "$HOST:/var/lib/cod4rw-relay/id/relay.der" "$ROOT/data/relay/relay.der"
echo "relay.der copied to data/relay/relay.der"
