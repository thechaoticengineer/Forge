# ADR-0006: Isolate Task Implementation in Git Worktrees

- Status: Accepted
- Date: 2026-08-30
- Decision owners: Project maintainers

## Context

Planning is complete and read-only. The next slice lets an agent change code, which is the first point where the orchestrator writes to a developer's repository. The product promises that a task runs in isolation, that pre-existing user work survives, and that the exact worktree an agent used can be opened for inspection.

Git offers linked worktrees, which give each task its own checkout and its own branch while sharing one object database. That fits the product better than cloning, stashing, or serializing work through the primary checkout, but it introduces durable filesystem state outside SQLite. A worktree can outlive the engine that created it, and `git worktree` itself has destructive options that must never be reachable from an automated path.

The repository the user selected is their working copy. It may be dirty, may already contain branches with similar names, may be a linked worktree itself, and may be operated on in another terminal while a run is active. The engine must decide what it refuses before it decides what it creates.

## Decision

The Rust engine will create one linked Git worktree and one task branch per implementation task, recorded durably before the Git command runs.

### Location and naming

Worktrees will live in the engine's own state directory, not inside the user's repository:

```text
<state root>/worktrees/<run id>/<task position>-<task slug>
```

The user's repository directory therefore gains no orchestrator files, repository discovery cannot rediscover a task worktree as a project, and removing the state directory removes only orchestrator-owned checkouts. The state root already exists as an owner-only directory and keeps that mode.

Task branches will use a reserved prefix:

```text
orchestrator/<run id>/<task position>-<task slug>
```

The slug is derived from the task title, lowercased, restricted to ASCII alphanumerics and hyphens, bounded in length, and never empty. The prefix is the engine's namespace; a name outside it is user territory.

### Base revision

Every task worktree for a run starts from the run's recorded `base_revision`, not from the repository's current `HEAD`. One recorded base per run keeps later diffs, verification evidence, and review comparable, and prevents an unrelated commit made during a long run from silently changing what a task was built on. The engine refuses to create a worktree when the recorded base revision is no longer present in the repository.

### Refusals

The engine will refuse, before running `git worktree add`, when:

- the repository is not an inspectable, non-bare Git worktree with a committed `HEAD`;
- the recorded base revision cannot be resolved in the repository;
- the task branch name already exists as any reference;
- the intended worktree directory already exists, is a symbolic link, or cannot be atomically reserved;
- the repository already has a linked worktree registered at that path;
- the durable record for that task already names a different branch or path.

The engine will never pass `--force` to `git worktree add`, `git worktree remove`, `git checkout`, or `git branch -D`, and will never create a task branch outside the reserved prefix.

A failed creation rolls back only what the engine itself reserved. `git worktree add` creates the task branch before it can fail, so rollback removes the reserved directory, prunes the incomplete registration, and deletes the reserved branch through a compare-and-delete that only succeeds while that branch still points exactly at the base revision. A branch that has moved is left alone. Any part of the rollback that fails is reported for manual inspection rather than escalated to a forced removal.

Uncommitted and untracked changes in the user's primary worktree do not block worktree creation, because a linked worktree does not read or modify them. That condition is recorded and surfaced so the user knows the agent is working from the committed base revision and cannot see their in-progress edits.

### Cleanup and recovery

A recorded worktree has an explicit lifecycle state: reserved, ready, missing, or retired. Cleanup is bounded by these rules:

- the engine removes only worktrees it created and recorded;
- removal requires a clean worktree with no commits that exist nowhere else, and is otherwise refused and reported;
- removal is an explicit user action, never an automatic consequence of a run ending, failing, or being cancelled;
- task branches are never deleted by the engine in this phase, apart from the compare-and-delete rollback of a branch a failed creation had just reserved;
- a worktree whose directory has disappeared is marked missing and its Git registration is pruned; the record is kept as history.

On startup the engine reconciles recorded worktrees against the filesystem and `git worktree list` and marks divergence instead of repairing it silently. A worktree reserved but never confirmed ready is treated the way an interrupted planner attempt already is: marked failed, with an audit event, so the user can retry rather than see permanently pending state.

This ADR does not decide implementer process supervision, activity streaming, verification commands, diff presentation, commit creation, or parallel execution of multiple tasks.

## Consequences

### Positive

- An agent cannot reach the user's working copy, index, or current branch.
- The user's repository directory stays free of orchestrator state.
- One recorded base revision per run makes diffs, verification, and review comparable.
- Reserved naming makes orchestrator-created branches and directories unambiguous.
- Destructive Git options are absent from the automated path rather than merely unused.
- Interrupted creation and vanished worktrees become explicit durable state.

### Negative

- Worktrees consume disk outside the repository, and an abandoned run leaves a checkout the user must retire deliberately.
- An agent cannot see uncommitted user work, which will occasionally surprise a user who expected it as context.
- Sharing one object database means repository-level Git operations run by the user in another terminal can still affect a task worktree.
- Reconciling filesystem state with durable records adds startup work and a new class of divergence to present.
- A branch created for a task outlives its worktree until the user removes it.

### Follow-up

- Define the implementer process contract and supervision before an agent is allowed to run inside a worktree.
- Present worktree location, base revision, and dirty-primary-worktree condition in the panel.
- Add an explicit retire action with its own confirmation once runs can complete.
- Decide how commits created in a task branch reach the user's branches in the final approval phase.
- Revisit disk accounting and retention if abandoned worktrees accumulate in practice.

## Alternatives Considered

### Work directly in the user's checkout

This needs no new filesystem state, but it forces stashing or refusing dirty repositories, exposes the user's branch and index to an agent, and makes concurrent human work unsafe. It contradicts the product's isolation and preservation promises.

### Clone the repository for each task

A clone is strongly isolated but duplicates the object database per task, is slow on large repositories, and separates commits from the origin repository so that returning results requires a fetch or push the user did not approve.

### Place worktrees inside the repository

`<repository>/.orchestrator/worktrees/...` keeps everything together and survives state-directory loss, but writes orchestrator directories into the user's project, risks appearing in their tooling and ignore rules, and makes discovery and cleanup boundaries harder to state.

### Branch from current `HEAD` at task start

Using live `HEAD` follows the user's latest work, but two tasks in one run could then start from different revisions, making the final diff and verification evidence incomparable and the run harder to explain.

### Automatic cleanup when a run ends

Removing worktrees automatically keeps disk usage low, but a failed or abandoned run is exactly when its checkout is most valuable for diagnosis, and automatic removal is precisely the silent deletion the product forbids.

## References

- [ADR-0001](0001-separate-engine-from-shell-frontend.md)
- [ADR-0003](0003-store-state-in-sqlite.md)
- [ADR-0004](0004-run-planners-as-constrained-cli-processes.md)
- [Human control and safety](../../README.md#human-control-and-safety)
- [Core workflow](../../README.md#core-workflow)
