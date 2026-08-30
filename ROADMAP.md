# Roadmap

> **Working title:** “Omarchy AI Build Orchestrator” remains descriptive and temporary.

The [README](README.md) defines the product vision and safety boundaries. This roadmap records implementation order and current progress. Checked items exist in the repository today; unchecked items are planned and must not be described as implemented.

No phase below is a promise of a release date. Safety, recovery, and a coherent vertical workflow take priority over feature count.

## Current Position

The foundation and planning workflow are implemented. A developer can open the Omarchy panel, browse repositories below configured local project roots, discover accessible GitHub repositories through an authenticated `gh` CLI, explicitly clone a missing repository, preserve a goal, ask Codex CLI or Claude Code CLI for a constrained structured plan, revise that plan, and approve or reject it. The next major slice is isolated implementation in a protected Git worktree.

The **Changes**, **Verification**, and **Review** panel sections currently communicate product direction but do not yet run those stages.

## Phase 0 — Foundation

- [x] Establish the Rust workspace, orchestration engine, and CLI.
- [x] Separate the Rust engine from the Quickshell frontend.
- [x] Use versioned JSON messages over a protected local Unix socket.
- [x] Persist authoritative run state and audit history in SQLite.
- [x] Inspect local Git repositories without modifying them.
- [x] Provide validated Omarchy `bar-widget` and `panel` plugin entry points.
- [x] Reconnect the bar, panel, and CLI to the same engine state.
- [x] Record lasting architecture choices with ADRs.

## Phase 1 — Repository and Planning Workflow

- [x] Create a durable draft from an absolute repository path and natural-language goal.
- [x] Complete repository directory paths through the Rust engine when `Tab` is pressed.
- [x] Show multiple path candidates without enumerating the filesystem in QML.
- [x] Navigate panel focus with arrow keys and Vim-style `h`/`j`/`k`/`l` equivalents.
- [x] Use `Enter` to enter or activate a focus area and `Esc` to cancel or close.
- [x] Run Codex CLI or Claude Code CLI as a constrained read-only planner.
- [x] Validate and persist structured tasks, acceptance criteria, and dependencies.
- [x] Edit tasks, safely reorder them, and preserve plan revisions.
- [x] Approve or reject the plan from either the CLI or Omarchy panel.
- [x] Restore the active draft and plan after engine or frontend restart.
- [x] Browse bounded local project roots and remote-only GitHub repositories from the panel.
- [x] Explicitly clone a selected GitHub repository into a collision-safe local destination.

Planned usability follow-ups:

- [x] Add a richer keyboard-driven repository chooser with shared search and manual-path fallback.
- [x] Keep the repository chooser usable in constrained-height and narrow panel layouts.
- [ ] Add current and previous run selection.
- [ ] Add the searchable command palette and contextual action discovery.
- [ ] Extend cursor-level keyboard focus to every actionable control introduced by later phases.

## Phase 2 — Isolated Implementation

This is the next major implementation phase.

- [x] Define worktree and task-branch lifecycle rules, including recovery and cleanup boundaries.
- [ ] Refuse to overwrite or absorb existing user changes.
- [ ] Create and record an isolated Git worktree for each implementation task.
- [ ] Let the user assign Codex CLI or Claude Code CLI as implementer.
- [ ] Run the implementer under explicit process supervision owned by the Rust engine.
- [ ] Stream structured activity and progress to the Omarchy panel.
- [ ] Let the user pause, cancel, redirect, or add context without losing run history.
- [ ] Preserve prompts, responses, changed files, failed approaches, and retries.
- [ ] Open the exact task worktree from the interface.

## Phase 3 — Deterministic Verification

- [ ] Define project verification commands and policy without asking an agent to infer success.
- [ ] Run builds, tests, formatting, linting, and analyzers in the task worktree.
- [ ] Capture exact commands, working directories, durations, output, and exit codes.
- [ ] Persist verification attempts and distinguish pass, fail, cancellation, and infrastructure errors.
- [ ] Present concise results in the **Verification** section with raw output available on demand.
- [ ] Return actionable deterministic failures to the implementing agent only under user-visible policy.

## Phase 4 — Independent Review and Correction

- [ ] Assign review to the agent that did not implement the change.
- [ ] Give the reviewer the approved plan, acceptance criteria, diff, and verification evidence.
- [ ] Store findings with severity, evidence, status, and disposition.
- [ ] Present findings in the **Review** section instead of hiding them in agent transcripts.
- [ ] Return specific accepted findings to the implementer for correction.
- [ ] Repeat verification and independent review after corrections.
- [ ] Stop for human judgment on architecture, security, product intent, or conflicting agent opinions.

## Phase 5 — Final Inspection and Approval

- [ ] Present the complete diff and changed-file summary.
- [ ] Present verification evidence and unresolved review findings together.
- [ ] Propose meaningful Conventional Commit boundaries and messages.
- [ ] Show proposed commits before creating them.
- [ ] Support keyboard-only final approval or rejection.
- [ ] Create only user-approved commits without amending, rebasing, squashing, merging, or pushing implicitly.
- [ ] Preserve the final outcome and rejected result in durable history.

## Phase 6 — Omarchy Integration and Recovery Hardening

- [ ] Define installation and lifecycle management for the Rust engine alongside the shell plugin.
- [ ] Add a conflict-checked Omarchy key binding for summoning or hiding the panel.
- [ ] Send Omarchy notifications for blocked, failed, completed, and waiting-for-user events.
- [ ] Verify live theme changes, light and dark themes, display scaling, and varied monitor sizes.
- [ ] Verify plugin reload and `omarchy-shell` restart during active implementation and verification.
- [ ] Add end-to-end recovery tests across engine, shell, and terminal restarts.
- [ ] Document supported installation, update, removal, state backup, and recovery procedures.

## First Milestone Checklist

The first milestone is complete only when the whole workflow works through the real Omarchy UI:

- [x] Install or enable the validated Quickshell plugin.
- [x] Summon the orchestrator panel from Omarchy.
- [x] Open a local Git repository or clone one from GitHub through the panel.
- [x] Describe a small engineering goal.
- [x] Inspect, revise, approve, or reject a generated plan.
- [ ] Watch Codex CLI or Claude Code CLI implement the plan in an isolated worktree.
- [ ] See deterministic build and test status update in the panel.
- [ ] Receive independent review from the other agent.
- [ ] Inspect the final diff and proposed semantic commits.
- [ ] Approve or reject the final result using only the keyboard.
- [x] Reload the planning UI or restart the engine without losing the draft or plan.
- [x] See current engine and attention state in the bar widget.

## After the First Milestone

Only after the vertical slice is dependable should the project consider parallel task execution, smarter agent routing, usage-limit awareness, pull-request workflows, configurable policies, security and prompt-injection checks, automated low-risk decisions, richer historical learning, additional Omarchy surfaces, or remote workers.
