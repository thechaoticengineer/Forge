# ADR-0016: Simplify the Single-Task Workflow

- Status: Proposed
- Date: 2026-09-02
- Decision owners: Project maintainers

## Context

The first self-hosted task proved that Forge can plan, implement, verify,
review, commit, and integrate a change. It also showed that the current
workflow exposes too much internal Git machinery. The user has to create and
later retire a linked worktree, approve several technical transitions, and
explicitly integrate a task branch even though those steps are not the work
they came to Forge to perform.

For the initial single-task product, that isolation model creates more friction
than value. The interface should present the user's decisions and keep routine
implementation, verification, review, correction, and Git bookkeeping behind
one coherent progression.

## Decision

The proposed initial workflow is:

1. The user selects a project and describes a task.
2. Forge generates a plan.
3. The user accepts the plan and chooses Codex or Claude as implementer.
4. Forge automatically implements the accepted plan, runs deterministic
   checks, requests an independent review, applies bounded corrections when
   appropriate, and verifies the result again.
5. Forge stops for the user only when a real product, architecture, security,
   destructive-action, or unresolved-review decision is required.
6. The completed result offers **Review changes** and **Merge & push**. Review
   exposes the diff, checks, and independent findings. Merge and push is one
   explicit consequential action with a clear target and outcome.

The single-task path will not create a linked Git worktree. At most one task
may modify a project at a time, and Forge must start from a clean checkout. A
temporary implementation branch may remain as an internal mechanism, but it
must not become a user-managed workflow step. Forge owns its creation,
checkout, integration, push outcome reporting, and cleanup.

Routine stages after implementer selection do not require separate clicks.
Failures remain visible and retryable, automatic correction is bounded, and
the implementing agent is never the sole reviewer of its own work.

This ADR remains proposed until the exact branch-switching, rejection,
rollback, push-failure, and recovery contracts are specified. If accepted, it
will supersede ADR-0006, the worktree-specific parts of ADR-0007, and the
separate integration interaction in ADR-0013. ADR-0012 will also need revision
or supersession where it requires a separate commit-approval step.

## Consequences

### Positive

- The primary experience becomes **Plan → Accept and choose agent → Automatic
  work → Review changes / Merge & push**.
- Worktrees, task branches, integration targets, and cleanup disappear from
  the normal interface.
- Verification, independent review, and bounded correction remain safety
  gates without becoming repeated user confirmations.
- A successful task ends with an obvious useful outcome rather than more Git
  administration.

### Negative

- The user's checkout cannot safely be used concurrently while Forge is
  implementing a task.
- Forge must refuse dirty repositories and detect external branch or working
  tree changes during a run.
- Switching a temporary branch in the selected checkout is more intrusive
  than a linked worktree.
- Combining local integration and push creates a partial-success case when the
  local branch advances but the remote push fails.
- Removing linked worktrees postpones parallel task implementation.

### Follow-up

- Specify ownership and recovery rules for the selected checkout and any
  hidden temporary branch.
- Define rejection behavior without silently deleting useful changes.
- Define the exact atomicity and UI for merge success followed by push failure.
- Replace worktree and integration controls with one next-action-oriented
  workflow.
- Add end-to-end tests for the automatic verification, review, correction,
  final inspection, integration, push, and cleanup sequence.
- Supersede the affected accepted ADRs when this proposal is accepted.

## Alternatives Considered

### Keep linked worktrees but hide them

This preserves stronger isolation and allows concurrent human work, but it
retains lifecycle and recovery complexity that is not currently earning its
cost. It remains a possible later implementation when parallel work becomes a
real product need.

### Modify the current branch directly

This is the smallest implementation, but it removes the clean boundary needed
for review, rejection, and final integration. A hidden temporary branch keeps
that boundary without exposing it as a user task.

### Keep every existing confirmation

This maximizes explicitness but makes routine safe progress feel like Git
administration. Consequential decisions should remain explicit; internal
pipeline transitions should not.

## References

- [ADR-0006](0006-isolate-task-implementation-in-git-worktrees.md)
- [ADR-0007](0007-supervise-implementation-agents-in-task-worktrees.md)
- [ADR-0010](0010-run-independent-reviews-in-fresh-agent-sessions.md)
- [ADR-0012](0012-separate-final-inspection-from-commit-approval.md)
- [ADR-0013](0013-fast-forward-approved-task-commits.md)
- [Roadmap](../../ROADMAP.md)
