# ADR-0013: Fast-Forward Approved Task Commits into Local Branches

- Status: Accepted
- Date: 2026-09-02
- Decision owners: Project maintainers

## Context

ADR-0012 ends final approval with one inspected commit on an isolated task
branch. Using that result still requires a developer to move another local
branch. Integration is consequential: the selected branch may be checked out,
dirty, advanced since the run began, or divergent from the approved commit.
Final task approval cannot imply permission to make that second change.

## Decision

Forge will expose integration as a separate explicit request after the task
commit reaches `created`. The request names an existing local target branch.
Before any Git write, the engine resolves the exact target head and approved
commit, proves that their histories permit a fast-forward or that the commit is
already contained, and records the user's integration intent in SQLite.

The Git operation rechecks the exact recorded target head. The branch must be
checked out in a clean worktree so Forge can keep its reference, index, and
files coherent. Forge prepares an exact compare-and-swap reference transaction
before updating the index and files. Git's prepared transaction holds the
target and checked-out `HEAD` locks, rejecting a concurrent branch switch or
target movement. Forge rechecks unique worktree ownership under those locks,
then updates the worktree and commits the transaction.
An unowned branch is refused rather than leaving an untracked temporary
worktree across a crash. Symbolic local branch refs are not accepted as targets.
A branch force-checked-out in multiple worktrees is refused because one checkout
would necessarily become stale. A changed, dirty, missing, or divergent target
is refused. Commits that change submodule pointers are also refused until Forge
has a design that keeps initialized submodule worktrees coherent. Forge does
not create a merge commit, cherry-pick, resolve conflicts, push, deploy, delete
the task branch, or retire its worktree.

Integration intent and outcome are durable. A reservation left by an engine
interruption becomes failed with an unknown-outcome warning and is never
replayed automatically.

## Consequences

- A user can bring an approved task into a normal local branch without leaving
  Forge while retaining a distinct confirmation boundary.
- Checked-out files update only when their worktree is clean.
- A prepared exact reference transaction prevents the checked-out branch from
  being advanced after concurrent movement or checkout changes.
- Integrating an unowned branch requires the user to check it out in a normal
  or linked worktree first.
- Divergent work requires a manual or future explicitly designed merge flow.
- Tasks that all start from the same run base are not automatically composed;
  dependent-task branch design remains separate work.

## Alternatives Considered

### Merge automatically during task approval

This combines two permissions, can touch the primary checkout unexpectedly,
and makes the inspected task result ambiguous.

### Cherry-pick the task commit

Cherry-pick synthesizes a new commit and may enter conflict resolution, so the
result is no longer the exact independently reviewed commit.

### Always update the branch ref directly

Moving a checked-out ref behind Git's worktree and index would leave the user's
checkout incoherent.

## References

- [ADR-0006](0006-isolate-task-implementation-in-git-worktrees.md)
- [ADR-0012](0012-separate-final-inspection-from-commit-approval.md)
- [Human control and safety](../../README.md#human-control-and-safety)
