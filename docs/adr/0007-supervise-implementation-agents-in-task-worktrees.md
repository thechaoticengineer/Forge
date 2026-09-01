# ADR-0007: Supervise Implementation Agents in Task Worktrees

- Status: Accepted
- Date: 2026-09-01
- Decision owners: Project maintainers

## Context

The planning workflow is read-only, and ADR-0006 creates an isolated worktree for an approved task. The next workflow boundary grants an agent permission to modify that worktree. The engine must remain responsible for process lifetime and durable state, use the developer's existing authenticated Codex CLI or Claude Code CLI installation, and keep the invocation recoverable when a client or the engine disappears.

An implementation process is not verification. A successful exit proves only that the CLI invocation completed; it does not establish that the task is correct, that checks pass, or that its changes are ready to commit. The later verification, independent review, and final approval boundaries must remain explicit.

## Decision

The user will explicitly select Codex or Claude for one task from the approved current plan. The task must already have a recorded worktree in the `ready` state. The Rust engine will record an implementation attempt before launching the selected CLI, move the run to `running`, and supervise at most one implementation attempt per run.

Both adapters will:

- use the existing authenticated CLI installation and its configured model rather than a direct model API or engine-selected model;
- use the recorded task worktree as the only working directory granted for writes;
- send the prompt over standard input so repository details do not appear in the process list;
- run non-interactively without a dangerous permission-bypass option;
- place the child in its own process group, capture bounded standard output and standard error, enforce a timeout, and terminate the group when supervision ends abnormally;
- receive the run goal, approved task, acceptance criteria, dependencies, base revision, branch, and explicit safety boundaries;
- be instructed not to commit, merge, push, rewrite history, or modify another checkout.

Codex will run ephemerally with its `workspace-write` sandbox rooted at the task worktree. Claude will run in safe, restricted mode with only its built-in read, search, edit, and write tools enabled; shell execution is deliberately excluded until deterministic command execution is owned by the engine.

SQLite will retain the selected agent, exact prompt, attempt status, bounded final and diagnostic output, exit code, error, and timestamps. A zero exit status marks the invocation `completed` and returns the run to `waiting_for_user`; it does not mark the task or run complete. A nonzero exit, timeout, launch failure, or output-limit failure marks the attempt and run `failed` while preserving the worktree for inspection or retry. Startup recovery marks an attempt left `running` as failed rather than replaying it.

This decision does not define live structured activity streaming, pause or redirection, deterministic verification commands, independent review, worktree retirement, diff approval, commits, or task-branch integration.

## Consequences

### Positive

- A write-capable agent is confined to an engine-owned checkout selected by an explicit user action.
- Shell or CLI disconnection does not make the frontend the process owner.
- Attempts and failures survive restarts with enough evidence to diagnose and retry them.
- Provider authentication and model selection stay with the installed CLIs.
- Successful agent prose cannot bypass verification, review, or human approval.

### Negative

- Bounded output is stored in SQLite until the artifact pipeline is implemented.
- Claude cannot run exploratory shell commands in this slice.
- Process groups improve normal cancellation and timeout behavior but cannot guarantee cleanup after an uncatchable engine or machine failure.
- The request remains open until the implementation process exits; richer asynchronous controls require a later protocol extension.

### Follow-up

- Stream normalized activity while keeping raw output available as evidence.
- Add explicit cancel, redirect, and additional-context requests.
- Move large immutable output into the artifact store with checksums.
- Define deterministic verification before treating implementation output as successful work.
- Add independent review by the agent that did not implement the task.

## Alternatives Considered

### Run agents in the user's selected checkout

This would expose the user's current branch, index, and uncommitted work. It violates the isolation boundary already accepted in ADR-0006.

### Let the QML or CLI client own the process

Closing a terminal, hiding the panel, or reloading `omarchy-shell` could then terminate work or lose its outcome, contradicting the shared-engine architecture.

### Grant unrestricted or bypassed permissions

This would simplify tool compatibility but would allow the agent to escape the intended worktree and cross consequential boundaries without user approval.

### Treat exit status zero as task completion

The CLI exit code establishes only process success. Correctness requires deterministic checks, independent review, and final human approval.

## References

- [ADR-0001](0001-separate-engine-from-shell-frontend.md)
- [ADR-0003](0003-store-state-in-sqlite.md)
- [ADR-0004](0004-run-planners-as-constrained-cli-processes.md)
- [ADR-0006](0006-isolate-task-implementation-in-git-worktrees.md)
- [Core workflow](../../README.md#core-workflow)
