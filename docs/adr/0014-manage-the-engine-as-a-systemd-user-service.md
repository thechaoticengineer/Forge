# ADR-0014: Manage the Engine as a systemd User Service

- Status: Accepted
- Date: 2026-09-02
- Decision owners: Project maintainers

## Context

The Quickshell plugin and CLI reconnect to an independently running engine, but
requiring a terminal to launch that engine breaks the intended Omarchy
workflow. The engine owns long-running work and must outlive shell reloads while
remaining scoped to the authenticated desktop user. Omarchy's plugin installer
only manages plugin source and deliberately runs no install hooks.

## Decision

The Rust CLI will manage a systemd user service named
`omarchy-ai-build-orchestrator.service`. Installation writes an owner-only unit
below `$XDG_CONFIG_HOME/systemd/user`, or `~/.config/systemd/user`, then reloads
the user manager and enables and starts the service for `default.target`.

The generated unit records absolute paths for the engine, Codex, Claude Code,
and GitHub CLI executables. It restarts the engine on failure but not after a
clean stop. The engine handles both `SIGINT` and `SIGTERM`, removes its owned
socket during orderly shutdown, and keeps durable state in the existing
user-scoped state directory.

Service installation, status, and removal remain explicit CLI actions. They do
not modify `/usr/share/omarchy`, hide executable installation inside the shell
plugin, or delete engine state, task worktrees, or task branches. Plugin
installation and engine lifecycle therefore remain independently reversible.

## Consequences

- Opening the panel no longer depends on a manually launched terminal process
  after the user installs the service.
- Shell restarts and plugin reloads do not own engine lifetime.
- Binary updates require installing new binaries and restarting the service;
  the unit keeps stable absolute executable identities.
- Linux systems without a systemd user manager need a future alternative
  activation design; the initial Omarchy target uses systemd.
- Removing the service intentionally preserves durable workshop state.

## Alternatives Considered

### Start the engine from QML

This couples work lifetime to the unsandboxed shell frontend and allows plugin
reloads to interrupt orchestration.

### Add an Omarchy plugin install hook

The installed Omarchy contract intentionally clones and validates plugin code
without executing install hooks or privileged commands.

### Use systemd socket activation immediately

Socket activation could reduce idle lifetime, but adds descriptor passing and
activation-specific recovery before the persistent process has shown a need
for it.

## References

- [ADR-0001](0001-separate-engine-from-shell-frontend.md)
- [ADR-0002](0002-use-versioned-json-over-a-unix-socket.md)
- `$OMARCHY_PATH/shell/plugins/README.md`
