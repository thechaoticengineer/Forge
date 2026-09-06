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
  temp_file=""
  cleanup_plugin_install() {
    if [[ -n "$temp_file" ]]; then
      rm -f -- "$temp_file"
    fi
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

  plugin_new=true
  plugin_changed=true
  manifest_changed=false
  if [[ -e "$plugin_dir" || -L "$plugin_dir" ]]; then
    plugin_new=false
    if diff -rq "$staging_dir/new" "$plugin_dir" >/dev/null; then
      plugin_changed=false
    else
      diff_status=$?
      # A comparison error must not be mistaken for changed files.
      if (( diff_status != 1 )); then
        exit "$diff_status"
      fi
    fi
    if ! cmp -s "$staging_dir/new/manifest.json" "$plugin_dir/manifest.json"; then
      manifest_changed=true
    fi
  fi

  if [[ "$plugin_changed" == false ]]; then
    # Keep the installed tree in place so even a swap cannot trigger a reload.
    echo "==> Plugin unchanged; not restarting shell"
    exit 0
  fi

  if [[ "$plugin_new" == true || "$manifest_changed" == true ]]; then
    # Discovery and manifest changes need a restart, so replace the whole tree.
    if [[ -e "$plugin_dir" || -L "$plugin_dir" ]]; then
      mv -T "$plugin_dir" "$staging_dir/old"
    fi
    mv -T "$staging_dir/new" "$plugin_dir"

    # Give any watcher-triggered QML reload time to settle before requesting
    # the restart needed for plugin discovery or manifest changes.
    sleep 2
    echo "==> Restarting shell (new plugin or manifest changed)"
    omarchy restart shell
  else
    echo "==> Updating plugin files in place for shell hot-reload; not restarting shell"
    # Remove obsolete entries and incompatible types, children before parents.
    while IFS= read -r -d '' installed; do
      staged="$staging_dir/new/${installed#"$plugin_dir/"}"
      if [[ ! -e "$staged" && ! -L "$staged" ]] ||
        { [[ -d "$installed" && ! -L "$installed" ]] &&
          [[ ! -d "$staged" || -L "$staged" ]]; } ||
        { [[ -d "$staged" && ! -L "$staged" ]] &&
          [[ ! -d "$installed" || -L "$installed" ]]; }; then
        rm -rf -- "$installed"
      fi
    done < <(find "$plugin_dir/" -mindepth 1 -depth -print0)

    # Preserve directories and unchanged files. Atomic saves in each target's
    # directory notify the shell's file watchers, unlike a whole-tree swap.
    while IFS= read -r -d '' staged; do
      installed="$plugin_dir/${staged#"$staging_dir/new/"}"
      if [[ -d "$staged" && ! -L "$staged" ]]; then
        mkdir -p -- "$installed"
        continue
      fi
      if [[ -L "$staged" && -L "$installed" ]]; then
        [[ "$(readlink -- "$staged")" == "$(readlink -- "$installed")" ]] && continue
      elif [[ ! -L "$staged" && ! -L "$installed" ]] && cmp -s "$staged" "$installed"; then
        continue
      fi
      mkdir -p -- "${installed%/*}"
      temp_file="$(mktemp "${installed%/*}/.${installed##*/}.new.XXXXXX")"
      cp -a -- "$staged" "$temp_file"
      mv -fT -- "$temp_file" "$installed"
      temp_file=""
    done < <(find "$staging_dir/new" -mindepth 1 -print0)
    echo "==> Plugin updated with per-file atomic replacements"
  fi
)

echo "==> Installing systemd user service"
mkdir -p "$unit_dir"
cp "$repo/systemd/forge-engine.service" "$unit_dir/forge-engine.service"
# Stop first: takes over from the old transient unit (enable refuses while
# one is loaded) and guarantees a restart onto the new binary.
systemctl --user stop forge-engine.service 2>/dev/null || true
systemctl --user daemon-reload
systemctl --user enable --now forge-engine.service

echo "==> Done"
systemctl --user --no-pager --lines=0 status forge-engine.service || true
