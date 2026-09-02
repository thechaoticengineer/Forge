# ADR-0015: Center the Panel on Tasks

- Status: Accepted
- Date: 2026-09-02
- Decision owners: Project maintainers

## Context

The first vertical workflow exposed **Overview**, **Plan**, **Changes**,
**Verification**, and **Review** as peer destinations in the Omarchy panel.
Those destinations follow the implementation pipeline, but a developer must
move between several of them to understand one task. The split also makes the
overview a catch-all and leaves evidence without a clear owner once a run has
multiple tasks.

Campaigns will add a broader objective, an evolving backlog, and more history,
but the unit a developer starts, redirects, inspects, approves, and integrates
will remain a task. The interface needs that stable center before campaign
navigation adds another level.

## Decision

The Omarchy panel will use tasks as its primary navigation and inspection
unit. A persistent task queue will show readiness, activity, blockers, and the
next available action. Selecting a task will open one task workspace containing
its goal, acceptance criteria, dependencies, worktree, implementation activity,
changes, deterministic verification, independent review, decisions, and
history.

Changes, verification, and review are evidence owned by a task. They will not
be peer top-level destinations. Large artifacts such as a complete patch may
open in a contextual inspection mode, but the selected task remains visible
and owns every action and artifact shown there.

Draft creation and plan generation remain setup states for producing tasks.
Before approval, the task queue is an editable proposal. After approval, it is
the actionable backlog. A future campaign is context around that backlog, not
a replacement for task-centered interaction.

The QML frontend may derive presentation labels and groupings defensively from
the authoritative snapshot. Lifecycle rules, readiness, Git actions, process
control, verification, review, and durable state remain owned by the Rust
engine.

## Consequences

- A developer can understand and act on one task without reconstructing it
  across pipeline-oriented screens.
- The default panel answers which task needs attention and what can happen
  next.
- Plan tasks can evolve into a campaign backlog without another global
  navigation redesign.
- Task evidence can grow large, so the workspace needs concise summaries and
  contextual drill-down rather than showing every artifact at once.
- Run selection and campaign context remain necessary, but become secondary
  navigation around the task workspace.

## Alternatives Considered

### Keep pipeline stages as peer sections

This is straightforward to implement, but fragments one task's state and
scales poorly when several tasks have changes, checks, and reviews at once.

### Make the run or campaign dashboard primary

A dashboard helps with broad progress, but it does not provide a stable place
for implementation controls, evidence, and human decisions. It would still
need a task workspace and risks recreating an overview catch-all.

### Use separate windows for each artifact

Separate surfaces provide room for large diffs and logs, but weaken keyboard
continuity and task context. Contextual drill-down inside the panel can be
added before considering additional surfaces.

## References

- [ADR-0001](0001-separate-engine-from-shell-frontend.md)
- [ADR-0010](0010-run-independent-reviews-in-fresh-agent-sessions.md)
- [Product UI direction](../../README.md#ui-and-keyboard-interaction)
