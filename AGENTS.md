# Repository Instructions

These instructions apply to the entire repository.

## Start Here

- Read `README.md` and `CONTRIBUTING.md` before changing the project.
- Treat the README as a future-facing product vision. Do not describe planned functionality as implemented.
- The product name is temporary. Do not invent or promote a final name.
- Do not scaffold application code unless the task explicitly requests implementation.

## Architecture

- Use Rust for orchestration, process management, durable state, Git integration, agent adapters, and CLI work.
- Use Quickshell/QML for the primary Omarchy frontend. Do not make a TUI, browser dashboard, Electron app, GTK app, or generic cross-platform GUI the primary interface.
- Keep QML thin and defensive. Agent processes, builds, tests, analyzers, Git operations, and durable state belong in the Rust engine.
- Keep an explicit local IPC boundary, but do not choose a protocol without a dedicated design decision.
- Ensure the Quickshell frontend, Rust CLI, and any optional TUI share the same engine and business logic.
- Support only existing authenticated Codex CLI and Claude Code CLI installations in the initial scope.
- Never let the implementing agent be the sole reviewer of its own work.

## Omarchy

- Before making Omarchy integration claims or changes, inspect the current installed documentation and source, especially:
  - `$OMARCHY_PATH/shell/README.md`
  - `$OMARCHY_PATH/shell/plugins/README.md`
  - `$OMARCHY_PATH/shell/plugins/bar/README.md`
- Do not guess how Omarchy works or rely on stale external examples when the installed contract is available.
- Do not modify built-in files under `$OMARCHY_PATH`.
- Use the supported third-party Quickshell plugin system, shared theme data, and reusable components.
- Keep third-party plugin IDs namespaced and outside the reserved `omarchy.*` namespace.
- Check effective Omarchy/Hyprland bindings before proposing a global shortcut.

## Safety and Verification

- Preserve unrelated and uncommitted user changes.
- Prefer isolated, reversible operations. Do not rewrite history, force-push, merge, deploy, or run destructive commands without explicit permission.
- Use deterministic command results and exit codes for builds, tests, formatting, linting, analyzers, and Git checks.
- Add or update tests with behavioral changes and run the checks relevant to the modified area.
- If a check cannot be run, state that clearly instead of implying success.
- Review the final diff for scope and accidental files before committing.

## Commits

- You may commit a completed logical change unless the user says not to commit.
- Use a short Conventional Commit subject: `<type>(optional-scope): <short imperative description>`.
- Keep one logical change per commit and avoid a body unless it adds necessary context.
- Do not add AI or tool attribution, generated-by text, or co-author trailers solely because an automated tool contributed.
- Commit only files relevant to the task.
- Never amend, squash, rebase, force-push, or rewrite user history without explicit permission.
