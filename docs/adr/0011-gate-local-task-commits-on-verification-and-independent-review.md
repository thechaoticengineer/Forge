# ADR-0011: Gate Local Task Commits on Verification and Independent Review

- Status: Accepted; commit authorization superseded by ADR-0012
- Date: 2026-09-01
- Decision owners: Project maintainers

## Context

A completed implementation is only an agent process result. Forge needs
authoritative command evidence and an independent review before it can safely
turn task-worktree changes into a local commit. Failures should be actionable
without requiring the developer to relay output between fresh sessions.

## Decision

An explicit `finish_task` request runs a bounded engine-owned pipeline in the
existing task worktree. The engine detects supported deterministic checks from
repository markers. Rust workspaces receive format, test, and Clippy checks;
Omarchy plugins receive the installed `omarchy plugin validate` contract. Exact
argv, status, output, duration, and exit code are stored in SQLite.

A failed verification or `changes_requested` verdict may start a fresh
implementer continuation with bounded evidence. The pipeline repeats
verification and independent review, up to the request's correction limit.
Every corrected implementation is reviewed independently; an unfavorable
verdict never causes reviewer fallback or verdict shopping.

The engine may create one local Conventional Commit only when the same
implementation attempt has a passing verification and an approved independent
review. The explicit finish request authorizes only this commit in Forge's
reserved task worktree. The engine validates the recorded branch and base,
never amends, merges, pushes, deploys, retires a worktree, or modifies the
developer's checkout.

## Consequences

- Command results and review findings become durable gate evidence.
- Routine failures can return to a fresh implementation session automatically.
- Unsupported projects stop with an infrastructure result instead of asking an
  agent to infer success.
- The initial detection policy is intentionally fixed and narrow; project-level
  configurable commands remain future work.
- A generated commit is local and isolated. Integrating or publishing it still
  requires a separate explicit action.

## Alternatives Considered

### Trust the implementing agent's success statement

Agent statements are useful context but cannot replace command exit status.

### Let review replace deterministic checks

A reasoning model cannot reliably establish build, test, format, or analyzer
success without running the authoritative commands.

### Commit before review and repair with later commits

This makes failed gates durable Git history prematurely and complicates the
simple isolated-task lifecycle.

## References

- [ADR-0006](0006-isolate-task-implementation-in-git-worktrees.md)
- [ADR-0007](0007-supervise-implementation-agents-in-task-worktrees.md)
- [ADR-0010](0010-run-independent-reviews-in-fresh-agent-sessions.md)
