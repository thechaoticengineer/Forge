# Roadmap

> **Working title:** “Omarchy AI Build Orchestrator” remains descriptive and temporary.

The [README](README.md) defines the product vision and safety boundaries. This roadmap records implementation order and current progress. Checked items exist in the repository today; unchecked items are planned and must not be described as implemented.

No phase below is a promise of a release date. Safety, recovery, and a coherent vertical workflow take priority over feature count.

## Current Position

The foundation and planning workflow are implemented. A developer can open the Omarchy panel, browse repositories below configured local project roots, discover accessible GitHub repositories through an authenticated `gh` CLI, explicitly clone a missing repository, preserve a goal, ask Codex CLI or Claude Code CLI for a constrained structured plan, revise that plan, and approve or reject it.

Each task of an approved plan is actionable in the Omarchy panel with its dependency declaration, worktree, branch, latest implementer attempt, failures, and next action visible in context. The panel can confirm creation of the recorded isolated worktree and launch a user-selected Codex or Claude implementer through the existing engine requests. Because task branches currently share the run's original base, dependent tasks are explicitly blocked in both the panel and engine until prerequisite task results can be composed without dropping changes; root tasks remain executable. The engine refuses conflicting worktree operations, supervises the bounded process, and preserves its outcome. The Omarchy Overview shows bounded durable activity and lets the user pause, resume, cancel, redirect, or add context while retaining partial work and linked attempt history. Opening and retiring exact worktrees and richer prompt history remain open.

The CLI and panel can run a bounded completion pipeline: detected Rust and Omarchy checks are persisted, their evidence is sent to a fresh independent reviewer, and failed gates can launch a fresh implementer correction. Passing gates prepare an exact final tree, complete patch, changed-file summary, and proposed one-task commit. A separate user approval revalidates that tree before creating the local isolated-worktree commit; rejection is durable and preserves the worktree. After commit creation, another confirmed action can fast-forward a selected checked-out clean local branch, with exact-head comparison and durable outcome. Configurable project checks, full raw-output drill-down, semantic multi-commit splitting, and divergent merge flows remain open.

## Next Delivery Slice — UI-First Self-Hosting

The immediate priority is to let a developer use the Omarchy panel to start the implementation workflow, not merely observe and control a process launched from the CLI. After the engine is available, an approved plan should be executable through final local task-commit approval without requiring a terminal command.

- [x] Present every approved task with its implementation readiness, dependency state, worktree, branch, assigned agent, latest attempt, and next available action.
- [x] Create a task worktree from the panel through the existing engine request, with a confirmation that explains the isolated branch and committed-base behavior.
- [x] Let the user choose Codex CLI or Claude Code CLI and start the implementation from the panel through the existing supervised engine request.
- [x] Keep worktree creation, agent selection, implementation launch, failure recovery, and retry usable with keyboard-only navigation.
- [x] Show actionable worktree and launch failures in the task context without losing the approved plan or creating duplicate side effects.
- [ ] Exercise the self-hosting path against this repository: approve a plan, create its worktree, launch its implementer, inspect activity and gates, and approve or reject the resulting local commit entirely through the panel.

The managed engine is installed and running for the current development environment, and the installed plugin has been refreshed and rescanned successfully. The remaining operational proof is to exercise the complete self-hosting path through the live panel. Task integration remains separate from final approval and never pushes. Composing multiple dependent task branches remains open because all task worktrees currently start from the run's shared base.

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

Planned usability follow-ups after the UI-first self-hosting slice:

- [x] Add a richer keyboard-driven repository chooser with shared search and manual-path fallback.
- [x] Keep the repository chooser usable in constrained-height and narrow panel layouts.
- [ ] Add current and previous run selection.
- [ ] Add the searchable command palette and contextual action discovery.
- [ ] Extend cursor-level keyboard focus to every actionable control introduced by later phases.

## Phase 2 — Isolated Implementation

This phase is partly implemented. A task can be given an isolated worktree and a user-selected implementing agent can run inside it under engine supervision. Recent categorized activity is durable and visible in the Omarchy panel, where the user can pause or resume the supervised process group, cancel it, or start a linked continuation with a redirect or additional context without deleting partial work.

- [x] Define worktree and task-branch lifecycle rules, including recovery and cleanup boundaries.
- [x] Refuse to overwrite or absorb existing user changes.
- [x] Create and record an isolated Git worktree for each implementation task.
- [x] Let the user assign Codex CLI or Claude Code CLI as implementer.
- [x] Run the implementer under explicit process supervision owned by the Rust engine.
- [x] Stream bounded categorized activity and progress to the Omarchy panel.
- [x] Let the user explicitly cancel an implementation without losing run history or partial work.
- [x] Let the user pause, redirect, or add context without losing run history.
- [ ] Preserve prompts, responses, changed files, failed approaches, and retries.
- [ ] Open the exact task worktree from the interface.
- [ ] Show recorded task worktrees, their branches, and their status in the panel.
- [ ] Retire a task worktree and its branch through an explicit confirmed action.

## Phase 3 — Deterministic Verification

- [ ] Define project verification commands and policy without asking an agent to infer success.
- [x] Run detected builds, tests, formatting, linting, and analyzers in the task worktree.
- [x] Capture exact commands, working directories, durations, output, and exit codes.
- [x] Persist verification attempts and distinguish pass, fail, and infrastructure errors. Cancellation remains planned.
- [x] Present concise results in the **Verification** section. Raw-output drill-down remains planned.
- [x] Return actionable deterministic failures to the implementing agent under an explicit bounded finish policy.

## Phase 4 — Independent Review and Correction

- [x] Assign review to the agent that did not implement the change, with an explicit fresh-session fallback policy.
- [x] Give the reviewer the approved plan, acceptance criteria, diff, and verification evidence.
- [ ] Store findings with severity, evidence, status, and disposition. Verdicts, severity, and evidence are implemented; disposition remains.
- [x] Present findings in the **Review** section instead of hiding them in agent transcripts.
- [x] Return requested findings to the implementer for correction.
- [x] Repeat verification and independent review after corrections.
- [ ] Stop for human judgment on architecture, security, product intent, or conflicting agent opinions.

## Phase 5 — Final Inspection and Approval

- [x] Present the complete diff and changed-file summary.
- [x] Present verification evidence and unresolved review findings together.
- [x] Propose a meaningful one-task Conventional Commit boundary and message. Semantic multi-commit splitting remains planned.
- [x] Show proposed commits before creating them.
- [x] Support keyboard-only final approval or rejection.
- [x] Create only explicitly requested local commits without amending, rebasing, squashing, merging, or pushing implicitly.
- [x] Preserve the final outcome and rejected result in durable history.
- [x] Define a separate explicit, conflict-checked action for integrating an approved task commit into a selected local branch.

## Phase 6 — Omarchy Integration and Recovery Hardening

- [x] Define installation and lifecycle management for the Rust engine alongside the shell plugin.
- [x] Shut the engine down cleanly on `SIGTERM` as well as `SIGINT`.
- [ ] Add a conflict-checked Omarchy key binding for summoning or hiding the panel.
- [ ] Send Omarchy notifications for blocked, failed, completed, and waiting-for-user events.
- [ ] Verify live theme changes, light and dark themes, display scaling, and varied monitor sizes.
- [ ] Verify plugin reload and `omarchy-shell` restart during active implementation and verification.
- [ ] Add end-to-end recovery tests across engine, shell, and terminal restarts.
- [ ] Document supported installation, update, removal, state backup, and recovery procedures.

## First Milestone Checklist

The completed first milestone established the full engine workflow with real Omarchy visibility and decision controls. The panel now starts task worktrees and implementers as well; the remaining proof for the UI-first self-hosting slice is exercising that complete path against this repository.

- [x] Install or enable the validated Quickshell plugin.
- [x] Summon the orchestrator panel from Omarchy.
- [x] Open a local Git repository or clone one from GitHub through the panel.
- [x] Describe a small engineering goal.
- [x] Inspect, revise, approve, or reject a generated plan.
- [x] Watch Codex CLI or Claude Code CLI implement the plan in an isolated worktree.
- [x] See deterministic build and test status update in the panel.
- [x] Receive independent review from the other agent.
- [x] Inspect the final diff and proposed semantic commit.
- [x] Approve or reject the final result using only the keyboard.
- [x] Reload the planning UI or restart the engine without losing the draft or plan.
- [x] See current engine and attention state in the bar widget.

## After the First Milestone

Only after the UI-first self-hosting slice and its engine lifecycle are dependable should the project consider parallel task execution, smarter agent routing, usage-limit awareness, pull-request workflows, configurable policies, security and prompt-injection checks, automated low-risk decisions, richer historical learning, additional Omarchy surfaces, or remote workers.
