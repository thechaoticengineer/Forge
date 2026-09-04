# ADR-0016: Simplify the Single-Task Workflow

- Status: Proposed; contracts specified 2026-09-04 and awaiting a decision
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

If accepted, this supersedes ADR-0006, the worktree-specific parts of
ADR-0007, and the separate integration interaction in ADR-0013. ADR-0012 also
needs revision where it requires a separate commit-approval step.

## Contracts

These were the open questions that kept the decision proposed. They are
specified here so the proposal can be accepted or rejected on its merits
rather than on its gaps.

### 1. Checkout ownership

Forge takes exclusive, durable ownership of the selected checkout for the
lifetime of one task. Ownership records the repository path, the owning run
and task, the branch the user was on (the **origin branch**), that branch's
head at acquisition, and the temporary branch name.

Acquisition requires all of: a non-bare Git worktree; `git status --porcelain`
empty, including untracked files; no in-progress rebase, merge, cherry-pick,
revert, or bisect; `HEAD` attached to a branch; and no existing Forge
ownership of the same repository. A failed precondition is reported as a
refusal naming the condition, never worked around.

Ownership is recorded before the checkout is touched and settled after, so an
interrupted acquisition is always visible as a reservation rather than as
silent partial state. At most one task owns a repository at a time.

### 2. External interference

The user keeps a normal checkout and may edit it. Before every consequential
step — verification, review evidence capture, final inspection, and merge —
Forge re-reads the repository and requires that `HEAD` is still the temporary
branch, that the temporary branch head matches what Forge last recorded, and
that the origin branch head is unchanged since acquisition.

Any mismatch stops the task in an `interference detected` state that names
what changed. Forge never resets, force-updates, checks out over, or discards
to recover from interference. The user chooses to re-inspect or to abandon.

### 3. Rejection preserves work

Rejection never deletes anything. Forge commits any dirty state on the
temporary branch, renames that branch to a durable `forge/rejected/<run>/<task>`
name, returns the checkout to the origin branch, and records the branch name
in run history. Forge does not delete the branch. Removing it is a separate,
explicitly confirmed action, consistent with the worktree-retirement rule this
ADR otherwise removes.

### 4. Merge and push are ordered, not atomic

Combining the two creates a partial-success case that must be reported
honestly rather than hidden.

Forge merges first: fast-forward only, onto the origin branch. A branch that
cannot fast-forward stops the task; Forge does not rebase, squash, or resolve
conflicts. Forge pushes second, only the origin branch, only to its configured
upstream, never with `--force`.

Three terminal outcomes are recorded and distinguished in the interface:

- **merged and pushed** — local and remote both advanced;
- **merged, not pushed** — the local branch advanced and the remote did not,
  with the exact push error and a retry-push action;
- **not merged** — nothing changed anywhere.

Forge does not undo a successful local merge to make the pair look atomic.
Reversing a completed merge is itself a history rewrite, which this project
does not perform without explicit permission. The interface states plainly
that local and remote differ until the push succeeds or the user acts.

### 5. Recovery

Every consequential transition is reserved before its Git operation and
settled after it, so a crash leaves a reserved record rather than an unknown
state. On startup Forge inspects the real repository for each reserved record
and classifies it: still on the temporary branch means ownership resumes; back
on the origin branch with the temporary branch present means ownership was
released and the task needs a decision; a missing temporary branch is recorded
as a loss and never recreated.

Forge never replays a consequential Git operation during recovery.

### 6. Where the automatic path still stops

Routine transitions need no confirmation. Forge stops for the user on
unresolved review findings, architecture, security or product judgment,
detected interference, a non-fast-forward merge, a push failure, and repeated
correction failure within the existing bounded limit.

### 7. Relationship to verification policy

ADR-0017 reads a project's verification policy as committed at the task's base
revision. Without a linked worktree that base revision is the origin branch
head recorded at acquisition, so the rule survives this change unaltered: the
policy that gates a task is still the one committed before the task started.

### 8. Multi-task sequencing

Removing worktrees does not remove the need for an accepted base. The origin
branch head at acquisition is that base, and a completed merge advances it, so
a dependent task acquires the checkout at a revision that already contains its
prerequisites. Sequential dependency composition is preserved; only concurrent
sibling implementation is given up.

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

- Replace worktree and integration controls with one next-action-oriented
  workflow.
- Add end-to-end tests for the automatic verification, review, correction,
  final inspection, merge, push, and cleanup sequence, including the
  merged-but-not-pushed and interference cases.
- Decide whether rejected branches are ever pruned automatically after an
  explicit retention period, or only by confirmed user action.
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
- [ADR-0017](0017-read-project-verification-policy-from-the-task-base-revision.md)
- [Roadmap](../../ROADMAP.md)
