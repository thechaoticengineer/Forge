# ADR-0001: Separate the Engine from the Shell Frontend

- Status: Accepted
- Date: 2026-08-29
- Decision owners: Project maintainers

## Context

The product needs a native Omarchy interface and must also supervise long-running Codex CLI and Claude Code CLI processes, isolated Git worktrees, builds, tests, analyzers, reviews, approvals, and durable run history.

Omarchy hosts third-party QML plugins as unsandboxed code inside one long-lived `omarchy-shell` Quickshell process. The plugin may be hidden, hot-reloaded during development, fail independently, or disappear when the shell restarts. None of those frontend lifecycle events should cancel active work, corrupt repository state, or erase orchestration history.

The same workflow must eventually be available through the Omarchy frontend, a Rust CLI, and an optional TUI without duplicating business logic or allowing the interfaces to disagree about run state.

## Decision

The product will consist of two independently restartable parts:

1. A Rust orchestration engine that owns agent execution, process supervision, Git and worktree operations, deterministic verification, state transitions, approvals, and durable history.
2. A Quickshell/QML third-party Omarchy plugin that owns presentation and interaction through a bar widget and a richer panel.

The two parts will communicate through an explicit local IPC boundary. The boundary must be structured, observable, versionable, and resilient to either side restarting. The frontend must be able to reconnect, load an authoritative state snapshot, reconcile missed events, and resume updates.

This ADR does not select the IPC protocol, message encoding, persistence technology, daemon activation model, or process ownership strategy. Each requires evidence and, where the choice has lasting consequences, a separate ADR.

## Consequences

### Positive

- Active runs and durable history can survive plugin reloads and shell restarts.
- The QML plugin remains small enough to reduce risk to the shared `omarchy-shell` process.
- Git, process, persistence, and orchestration logic can be tested without a graphical shell.
- The Quickshell frontend, CLI, and optional TUI can share one authoritative workflow and state model.
- Frontend failures can be isolated from agent and repository operations.

### Negative

- The project must design, version, test, and diagnose an IPC boundary.
- Reconnection, snapshot synchronization, backpressure, cancellation, and duplicate delivery become explicit engineering concerns.
- Installation and lifecycle management involve both a shell plugin and an engine process.
- End-to-end testing requires coverage across the process boundary.

### Follow-up

- Define engine lifecycle and activation requirements.
- Define durable state and recovery semantics.
- Evaluate local IPC options and record the selected protocol in a separate ADR.
- Define compatibility and migration rules for IPC messages and stored state.
- Create an end-to-end restart test covering engine restart, plugin reload, and `omarchy-shell` restart.

## Alternatives Considered

### Run orchestration directly in QML

This would remove the IPC boundary but place agent processes, Git operations, checks, and state inside the unsandboxed long-lived shell. A QML error or plugin reload could then interrupt work or lose state, and non-graphical interfaces would need to duplicate the workflow.

### Make a standalone TUI or desktop application primary

This would simplify frontend process isolation but would not deliver the required native Omarchy bar and panel experience. It would also weaken integration with the existing `omarchy-shell` plugin architecture.

### Give each interface its own orchestration implementation

This would avoid a shared engine API at first, but the interfaces would develop different state machines, safety behavior, and recovery semantics. The duplication would make verification and traceability less reliable.

## References

- [Product vision and architecture boundaries](../../README.md#architecture-boundaries)
- `$OMARCHY_PATH/shell/README.md`
- `$OMARCHY_PATH/shell/plugins/README.md`
