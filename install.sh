#!/usr/bin/env bash
# Build Forge and install the engine binary, systemd user service, and
# Omarchy plugin files from this working tree. Idempotent; safe to run
# while the engine is running (the binary is replaced by atomic rename).
set -euo pipefail

repo="$(cd "$(dirname "$0")" && pwd)"
engine_dir="$HOME/.local/share/forge"
plugin_dir="$HOME/.config/omarchy/plugins/dev.omarchy-ai-build-orchestrator"
unit_dir="$HOME/.config/systemd/user"

echo "==> Building engine"
cargo build --release --manifest-path "$repo/Cargo.toml"

echo "==> Validating plugin"
omarchy plugin validate "$repo"

echo "==> Installing engine binary"
mkdir -p "$engine_dir"
cp "$repo/target/release/forge" "$engine_dir/forge-engine.new"
mv -f "$engine_dir/forge-engine.new" "$engine_dir/forge-engine"

echo "==> Installing plugin files"
mkdir -p "$plugin_dir"
# The plugin copy is managed by this script, not git.
rm -rf "$plugin_dir/.git"
rsync -a --delete "$repo/quickshell/" "$plugin_dir/quickshell/"
rsync -a "$repo/manifest.json" "$plugin_dir/manifest.json"

echo "==> Installing systemd user service"
mkdir -p "$unit_dir"
cp "$repo/systemd/forge-engine.service" "$unit_dir/forge-engine.service"
# Stop first: takes over from the old transient unit (enable refuses while
# one is loaded) and guarantees a restart onto the new binary.
systemctl --user stop forge-engine.service 2>/dev/null || true
systemctl --user daemon-reload
systemctl --user enable --now forge-engine.service

echo "==> Restarting shell"
omarchy restart shell

echo "==> Done"
systemctl --user --no-pager --lines=0 status forge-engine.service || true
