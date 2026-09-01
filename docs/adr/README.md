# Architecture Decision Records

Architecture Decision Records preserve the context and reasoning behind significant technical choices. They explain why a decision was made, which alternatives were considered, and what consequences contributors should expect.

ADRs supplement the product vision in the root README. They do not turn tentative ideas into completed functionality.

## When to Write an ADR

Write an ADR when a decision:

- establishes or changes a major component boundary;
- selects a protocol, persistence model, dependency, or integration strategy;
- creates a security, reliability, compatibility, or operational constraint;
- rejects a plausible alternative that future contributors may reconsider;
- is difficult or costly to reverse.

Routine implementation details, formatting choices, and easily reversible experiments usually do not need an ADR.

## Process

1. Copy `0000-template.md` to the next available four-digit number.
2. Use a short kebab-case filename, such as `0002-select-local-ipc-protocol.md`.
3. Start with status `Proposed` while the decision is under discussion.
4. Record the context, decision, consequences, and credible alternatives.
5. Change the status to `Accepted` when the decision is approved.
6. Do not rewrite the history of an accepted ADR. Create a new ADR that supersedes it when the decision changes.

Keep ADRs concise enough to review, but include enough reasoning that a future contributor can understand the tradeoff without reconstructing old conversations.

## Statuses

- `Proposed` — under discussion and not yet binding;
- `Accepted` — current architectural direction;
- `Deprecated` — retained for history but no longer recommended;
- `Superseded by ADR-NNNN` — replaced by a newer decision;
- `Rejected` — considered and explicitly not adopted.

## Index

| ADR | Status | Decision |
| --- | --- | --- |
| [ADR-0001](0001-separate-engine-from-shell-frontend.md) | Accepted | Separate the Rust orchestration engine from the Omarchy shell frontend |
| [ADR-0002](0002-use-versioned-json-over-a-unix-socket.md) | Accepted | Exchange versioned JSON messages over a private Unix-domain socket |
| [ADR-0003](0003-store-state-in-sqlite.md) | Accepted | Keep authoritative state in SQLite and large artifacts in files |
| [ADR-0004](0004-run-planners-as-constrained-cli-processes.md) | Accepted | Run Codex and Claude planners as bounded, read-only CLI subprocesses |
| [ADR-0005](0005-browse-local-and-github-repositories.md) | Accepted | Discover local projects and clone GitHub repositories through the Rust engine |
| [ADR-0006](0006-isolate-task-implementation-in-git-worktrees.md) | Accepted | Give each implementation task an isolated Git worktree and reserved task branch |
| [ADR-0007](0007-supervise-implementation-agents-in-task-worktrees.md) | Accepted | Run explicitly assigned implementation agents under engine supervision in task worktrees |
