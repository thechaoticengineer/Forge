# ADR-0012: Separate Final Inspection from Commit Approval

- Status: Accepted
- Date: 2026-09-01
- Decision owners: Project maintainers
- Supersedes: The `finish_task` commit-authorization portion of ADR-0011

## Context

ADR-0011 allowed one explicit finish request to verify, review, correct, and
commit a task. That guarantees technical gates, but the final corrected tree
may differ from the tree the user saw before the gates ran. The first complete
workflow requires the user to inspect the exact final diff and proposed commit
before authorizing the Git side effect.

## Decision

`finish_task` prepares a durable task-commit proposal and never creates the
commit. After verification passes and an independent reviewer approves the same
implementation attempt, the engine captures the exact proposed Git tree, full
binary patch, changed-file summary, Conventional Commit message, and evidence
references. The proposal becomes the authoritative final-inspection record.

The user must then explicitly approve or reject that proposal. Approval
re-captures the worktree through a temporary Git index and compares its tree
object ID with the inspected tree. A mismatch marks the proposal stale and
creates no commit. Git staging validates the same tree again immediately before
commit creation. Rejection records the decision while preserving the worktree,
branch, changes, proposal, and evidence.

The first milestone keeps one task worktree mapped to one proposed local
Conventional Commit. Semantic multi-commit splitting remains a later feature.
Approval never merges, pushes, deploys, retires the worktree, or changes the
developer's primary checkout.

## Consequences

- The user approves the exact tree that Git will commit, not an earlier agent
  result.
- Changes made after inspection invalidate the proposal instead of silently
  entering the approved commit.
- Final approval and rejection survive engine and shell restarts as durable
  state.
- Capturing an exact tree uses a temporary index. Git may write unreachable
  blob and tree objects to the shared object database, but it changes no index,
  reference, branch, worktree file, or user checkout before approval.
- `finish_task` changes from a commit operation into a preparation operation;
  protocol version 2 clients use separate approval and rejection requests
  afterward.

## Alternatives Considered

### Treat finish as approval

The gate correction loop can change the final tree, so a pre-finish inspection
would not authorize the exact result.

### Recompute the diff only after approval

This gives the user no stable artifact to inspect and makes the approval
ambiguous.

### Copy the worktree before approval

A second checkout would provide isolation but duplicate durable filesystem
state and complicate recovery without improving the tree-identity guarantee.

## References

- [ADR-0006](0006-isolate-task-implementation-in-git-worktrees.md)
- [ADR-0010](0010-run-independent-reviews-in-fresh-agent-sessions.md)
- [ADR-0011](0011-gate-local-task-commits-on-verification-and-independent-review.md)
