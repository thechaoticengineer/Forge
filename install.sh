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
(
  mkdir -p "${plugin_dir%/*}"
  # Keep temporary trees on the same filesystem, hidden from plugin discovery.
  # The wrapper has no manifest; only the completed new tree becomes a plugin.
  staging_dir="$(mktemp -d "${plugin_dir%/*}/.forge.staging.XXXXXX")"
  cleanup_plugin_install() {
    if [[ -e "$staging_dir/old" || -L "$staging_dir/old" ]] &&
      [[ ! -e "$plugin_dir" && ! -L "$plugin_dir" ]]; then
      if ! mv -T "$staging_dir/old" "$plugin_dir"; then
        echo "Failed to restore plugin; previous copy retained at $staging_dir/old" >&2
        return 1
      fi
    fi
    rm -rf "$staging_dir"
  }
  trap cleanup_plugin_install EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM

  mkdir "$staging_dir/new"
  # Copy only plugin assets, so a stray .git in the installed copy is discarded.
  rsync -a "$repo/quickshell/" "$staging_dir/new/quickshell/"
  rsync -a "$repo/manifest.json" "$staging_dir/new/manifest.json"

  # Renames expose a complete snapshot, with a brief gap on re-install.
  if [[ -e "$plugin_dir" || -L "$plugin_dir" ]]; then
    mv -T "$plugin_dir" "$staging_dir/old"
  fi
  mv -T "$staging_dir/new" "$plugin_dir"
)

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
