#!/bin/sh
# Build the UI + static aarch64 binary and install on an explicit SSH host.
# Stream files over SSH so no sftp-server is required.
set -eu
[ "$#" -eq 1 ] || { echo "usage: $0 <router-ssh-alias>" >&2; exit 2; }
HOST=$1
cd "$(dirname "$0")/.."
(cd web && npm run build)
cargo zigbuild --release --target aarch64-unknown-linux-musl
BIN=target/aarch64-unknown-linux-musl/release/openwrtfi
SSH="ssh -o BatchMode=yes $HOST"
$SSH 'cat > /tmp/openwrtfi.new && chmod +x /tmp/openwrtfi.new && mv /tmp/openwrtfi.new /usr/bin/openwrtfi' < "$BIN"
$SSH 'cat > /etc/init.d/openwrtfi && chmod +x /etc/init.d/openwrtfi' < packaging/openwrt/files/openwrtfi.init
$SSH '[ -f /etc/openwrtfi.json ] || cat > /etc/openwrtfi.json' < packaging/openwrt/files/openwrtfi.json
$SSH '/etc/init.d/openwrtfi enable; /etc/init.d/openwrtfi restart' </dev/null
echo "deployed $(du -h "$BIN" | cut -f1) → http://$HOST:8088"
