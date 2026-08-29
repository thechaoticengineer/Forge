# Contributing

Thank you for helping shape Omarchy AI Build Orchestrator. The project is at an early foundation stage, so contributions should strengthen the architecture, constraints, and first vertical slice rather than broaden the scope prematurely.

The project name is temporary. Use the descriptive working title from the README until a permanent name is chosen.

## Before You Start

Read these files before making changes:

- `README.md` for the product vision, architecture, initial scope, and milestone;
- `AGENTS.md` for repository instructions that also apply to coding agents;
- `docs/adr/` for accepted architecture decisions and the ADR process;
- the current installed Omarchy documentation when changing or documenting shell integration.

For substantial product, architecture, security, persistence, or workflow changes, discuss the approach before implementation and create or update an ADR when the choice has lasting consequences. Small corrections and focused documentation improvements can proceed directly.

## Project Principles

Contributions should preserve these foundations:

- The product is a software workshop, not an AI chat wrapper or transcript viewer.
- The Rust engine owns orchestration, processes, Git operations, and durable state.
- The Quickshell/QML plugin is the primary Omarchy frontend and remains thin and defensive.
- Codex CLI and Claude Code CLI are the only initial agent integrations.
- Agent implementation and independent review are separate roles.
- Deterministic commands decide whether builds, tests, formatting, linting, and analysis succeeded.
- Human approval remains required at consequential boundaries.
- The CLI, Quickshell frontend, and any future TUI use the same engine and workflow model.

## Omarchy Changes

Base Omarchy integration claims on the locally installed version, not memory or assumptions. Start with:

```text
$OMARCHY_PATH/shell/README.md
$OMARCHY_PATH/shell/plugins/README.md
$OMARCHY_PATH/shell/plugins/bar/README.md
```

Inspect the installed shell source when the documentation does not establish a contract. Do not modify built-in files under `$OMARCHY_PATH`, copy built-in plugins to reproduce their appearance, use the reserved `omarchy.*` namespace, or invent unsupported plugin kinds.

The QML frontend must use the supported third-party plugin architecture and shared Omarchy theme and component conventions. Agent runs, builds, analyzers, Git operations, and persistent state belong in the Rust engine, outside the long-lived `omarchy-shell` process.

## Development Workflow

1. Start from a clean understanding of the repository and preserve unrelated user changes.
2. Keep the change focused on one clear outcome.
3. Add or update tests when behavior changes.
4. Run the relevant deterministic checks and record any checks that cannot be run.
5. Review the final diff for scope, safety, and accidental generated files.
6. Request independent review for agent-produced implementation work.

The Rust workspace is pinned through `rust-toolchain.toml`. Run the relevant checks from the repository root:

```text
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
omarchy plugin validate .
```

The `build-orchestrator status` and `build-orchestrator ping` commands connect to a running `orchestrator-engine` through the same local protocol used by the QML frontend. These commands currently expose only the foundation health and state snapshot; they do not yet implement the product workflow.

Documentation changes should also be checked for Markdown structure, broken internal references, trailing whitespace, and consistency with the README.

## Commits

Use Conventional Commits:

```text
<type>(optional-scope): <short imperative description>
```

Keep each commit to one logical change and keep the subject short. Use a body only when the reason is not obvious. Do not add AI or tool attribution, generated-by text, or co-author trailers solely because an automated tool helped create the change.

Examples:

```text
docs: add contribution guidelines
feat(core): add agent task scheduler
fix(ipc): reconnect after shell reload
test(review): cover failed review loop
```

Do not amend, squash, rebase, force-push, or rewrite another contributor's history without explicit permission.

## Pull Requests

A pull request should explain:

- the problem or goal;
- the chosen approach and important tradeoffs;
- how the change was verified;
- any known limitations, follow-up work, or human decisions still required.

Keep unrelated refactoring separate where practical. Include screenshots or recordings for meaningful Quickshell UI changes, and describe how keyboard-only operation, theme changes, multiple display sizes, and plugin reloads were checked.

## Safety and Security

Never include credentials, tokens, private prompts, private repository data, or local run history in a contribution. Avoid destructive Git behavior and preserve uncommitted user work. Security concerns that could put users or repositories at risk should be reported privately to the maintainer rather than demonstrated against a live system.

## License

By contributing, you agree that your contribution is provided under the repository's MIT License.
