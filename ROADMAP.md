# Roadmap

> **Working title:** “Omarchy AI Build Orchestrator” remains descriptive and temporary.

The [README](README.md) defines the product vision and safety boundaries. This roadmap records implementation order and current progress. Checked items exist in the repository today; unchecked items are planned and must not be described as implemented.

No phase below is a promise of a release date. Safety, recovery, and a coherent vertical workflow take priority over feature count.

## Current Position

The foundation and planning workflow are implemented. A developer can open the Omarchy panel, browse repositories below configured local project roots, discover accessible GitHub repositories through an authenticated `gh` CLI, explicitly clone a missing repository, preserve a goal, ask Codex CLI or Claude Code CLI for a constrained structured plan, revise that plan, and approve or reject it.

Each task of an approved plan is actionable in the Omarchy panel with its dependency declaration, worktree, branch, latest implementer attempt, failures, and next action visible in context. The panel can confirm creation of the recorded isolated worktree and launch a user-selected Codex or Claude implementer through the existing engine requests. Because task branches currently share the run's original base, dependent tasks are explicitly blocked in both the panel and engine until prerequisite task results can be composed without dropping changes; root tasks remain executable. The engine refuses conflicting worktree operations, supervises the bounded process, and preserves its outcome. The Omarchy Overview shows bounded durable activity and lets the user pause, resume, cancel, redirect, or add context while retaining partial work and linked attempt history. Opening and retiring exact worktrees and richer prompt history remain open.

The CLI and panel can run a bounded completion pipeline: detected Rust and Omarchy checks are persisted, their evidence is sent to a fresh independent reviewer, and failed gates can launch a fresh implementer correction. Passing gates prepare an exact final tree, complete patch, changed-file summary, and proposed one-task commit. A separate user approval revalidates that tree before creating the local isolated-worktree commit; rejection is durable and preserves the worktree. After commit creation, another confirmed action can fast-forward a selected checked-out clean local branch, with exact-head comparison and durable outcome. Configurable project checks, full raw-output drill-down, semantic multi-commit splitting, and divergent merge flows remain open.

This is a complete single-task vertical slice, not yet a whole-project operating loop. A run still plans against one recorded repository base, dependent tasks cannot inherit integrated prerequisite work, and the engine does not select and continue through a durable backlog. Forge can safely complete one result; it cannot yet own a broad engineering campaign and keep working until every reachable problem is resolved.

## Immediate Validation — First Self-Hosted Run

The UI-first implementation slice is present. Before campaign orchestration builds on it, the complete path must be exercised as one uninterrupted self-hosted run against this repository without substituting terminal commands for panel decisions.

- [x] Present every approved task with its implementation readiness, dependency state, worktree, branch, assigned agent, latest attempt, and next available action.
- [x] Create a task worktree from the panel through the existing engine request, with a confirmation that explains the isolated branch and committed-base behavior.
- [x] Let the user choose Codex CLI or Claude Code CLI and start the implementation from the panel through the existing supervised engine request.
- [x] Keep worktree creation, agent selection, implementation launch, failure recovery, and retry usable with keyboard-only navigation.
- [x] Show actionable worktree and launch failures in the task context without losing the approved plan or creating duplicate side effects.
- [ ] Exercise the self-hosting path against this repository: approve a plan, create its worktree, launch its implementer, inspect activity and gates, and approve or reject the resulting local commit entirely through the panel.

The managed engine is installed and running for the current development environment, and the installed plugin has been refreshed and rescanned successfully. The remaining operational proof is to exercise the complete self-hosting path through the live panel. Task integration remains separate from final approval and never pushes. Composing multiple dependent task branches remains open because all task worktrees currently start from the run's shared base.

## Next Major Milestone — Durable Whole-Project Campaigns

The next milestone is not another isolated feature task. Forge should accept a broad project objective, turn repository evidence and an editable backlog into ordered work, and continue through that work across many independently reviewed changes. A **campaign** is the durable owner of that objective, backlog, evolving integration base, policies, decisions, and history.

The first campaign implementation should be deliberately sequential. It should prove correct dependency composition and recovery before parallel execution is considered. Human approval remains required for consequential Git operations, architecture or product decisions, policy changes, pushes, and deployment.

The milestone is complete when Forge can use the repository's full roadmap as a campaign, finish and integrate multiple dependent tasks onto an evolving local base, re-evaluate the remaining work after each integration, survive restarts, and continue until the campaign is complete or has an explicit evidence-backed blocker.

### Delivery order

1. **Campaign state and backlog:** persist a broad objective, ordered work items, dependencies, priorities, acceptance criteria, policies, and lifecycle independently from a single agent plan attempt.
2. **Evolving integration base:** dispatch each new task from the campaign's current accepted base, advance that base only after explicit integration, and invalidate or re-plan stale work safely.
3. **Dependency composition:** make an integrated prerequisite available to dependent tasks without cherry-picking, silently merging divergent work, or dropping sibling changes.
4. **Durable coordinator:** deterministically select the next unblocked item, run the existing implementation/verification/review/approval pipeline, and continue under visible user-controlled policy.
5. **Campaign interface:** show the whole backlog, progress, current work, blockers, decisions, evidence, and next action in the Omarchy panel.
6. **Self-hosting proof:** run a real Forge campaign through multiple dependent improvements, including restart recovery and a human-decision stop, entirely through the managed engine and panel.

### Non-goals for the first campaign milestone

- Parallel implementation of sibling tasks.
- Automatic push, pull-request creation, deployment, release, or destructive cleanup.
- Automatic acceptance of architecture, security, product-scope, or conflicting-review decisions.
- Rebase, history rewriting, force-push, or silent conflict resolution.
- Treating agent claims, elapsed time, or token usage as proof that work is complete.

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
- [ ] Add a dedicated inventory for all recorded task worktrees, branches, and lifecycle states across current and previous runs.
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

## Phase 7 — Campaign State and Backlog

- [ ] Record a campaign separately from a single run, with repository identity, broad objective, current integration base, status, and timestamps.
- [ ] Persist an editable backlog with stable work-item identities, acceptance criteria, dependencies, priority, and source or rationale.
- [ ] Represent queued, ready, running, waiting-for-user, blocked, integrated, superseded, failed, and completed states explicitly.
- [ ] Preserve campaign revisions and the decision that added, changed, reordered, split, superseded, or removed each work item.
- [ ] Import an approved multi-task plan into a campaign without treating that first plan as permanently correct.
- [ ] Reconcile roadmap, repository, review, and verification evidence into proposed backlog changes that require visible approval.
- [ ] Add campaign creation, selection, pause, resume, stop, and archive operations to the shared engine protocol and CLI.
- [ ] Show current and previous campaigns without losing existing run history.

## Phase 8 — Evolving Base and Dependency Composition

- [ ] Record the exact accepted campaign base and update it only after a separately approved integration succeeds.
- [ ] Create each task worktree from the campaign base current at dispatch time rather than the campaign's original base.
- [ ] Mark planned or running work stale when its assumed base no longer matches the accepted campaign base.
- [ ] Re-plan or recreate stale, not-yet-approved work without rewriting user history or discarding useful evidence.
- [ ] Unblock a dependent item only when every prerequisite result is present in its dispatch base.
- [ ] Compose sequential task results through exact fast-forward integration before considering divergent merge support.
- [ ] Detect sibling overlap and require re-planning or explicit human resolution instead of guessing conflict intent.
- [ ] Retire integrated task worktrees and branches through a separate confirmed, recoverable cleanup action.
- [ ] Preserve the rule that integration never implies push or deployment.

## Phase 9 — Durable Campaign Coordinator

- [ ] Deterministically choose the next ready work item by approved dependency and priority order.
- [ ] Reuse the existing supervised implementation, deterministic verification, independent review, final inspection, and integration pipeline for every item.
- [ ] Expose the minimum campaign execution policy and **Continue**, **Pause**, and **Stop** controls through the shared protocol, CLI, and panel before enabling coordinator continuation.
- [ ] Continue to the next ready item only under the campaign execution policy explicitly selected by the user.
- [ ] Define stop boundaries for human judgment, failed or contradictory review, repeated correction failure, stale plans, unavailable agents, and unsafe Git state.
- [ ] Distinguish retryable infrastructure failure, work-item failure, campaign blocker, and campaign completion.
- [ ] Re-evaluate readiness and proposed backlog changes after every accepted integration.
- [ ] Recover a campaign after engine or shell restart without replaying consequential side effects or losing which action owned the transition.
- [ ] Support a user redirect that changes campaign priority or scope while preserving the previous plan and partial evidence.
- [ ] Explain why the coordinator selected, skipped, blocked, retried, superseded, or stopped on each work item.

## Phase 10 — Campaign Interface and Self-Hosting Proof

- [ ] Add a campaign dashboard with objective, accepted base, overall progress, active item, queued work, blockers, and waiting decisions.
- [ ] Let the user inspect and edit backlog ordering, dependencies, acceptance criteria, and priority with keyboard-first controls.
- [ ] Present the next proposed coordinator action before it launches work or changes Git state.
- [ ] Provide explicit **Continue campaign**, **Pause**, **Redirect**, **Stop**, and **Resolve blocker** controls.
- [ ] Link every campaign item to its worktree, attempts, verification evidence, reviews, commit, integration, and decision history.
- [ ] Show stale or superseded work without presenting it as part of the accepted base.
- [ ] Notify the user only for failures, blockers, completed campaign work, or decisions that actually need attention.
- [ ] Create a Forge campaign from the complete remaining roadmap rather than a demonstration-only goal.
- [ ] Complete and integrate multiple dependent Forge tasks through the live panel against an evolving `main`.
- [ ] Restart the engine and reload the Omarchy plugin during that campaign, then continue from durable state.
- [ ] Demonstrate a safe stop at a real human-decision boundary and a later explicit continuation.
- [ ] Finish with every campaign item completed, superseded with rationale, or blocked with concrete evidence.

## First Vertical Milestone Checklist

The completed first vertical milestone established each part of the single-task engine workflow with real Omarchy visibility and decision controls. The remaining validation is exercising those parts as one uninterrupted self-hosted run against this repository.

- [x] Install or enable the validated Quickshell plugin.
- [x] Summon the orchestrator panel from Omarchy.
- [x] Open a local Git repository or clone one from GitHub through the panel.
- [x] Describe a bounded engineering goal.
- [x] Inspect, revise, approve, or reject a generated plan.
- [x] Watch Codex CLI or Claude Code CLI implement the plan in an isolated worktree.
- [x] See deterministic build and test status update in the panel.
- [x] Receive independent review from the other agent.
- [x] Inspect the final diff and proposed semantic commit.
- [x] Approve or reject the final result using only the keyboard.
- [x] Reload the planning UI or restart the engine without losing the draft or plan.
- [x] See current engine and attention state in the bar widget.

## After the Campaign Milestone

Only after sequential campaign execution and recovery are dependable should the project consider parallel task execution, smarter agent routing, usage-limit awareness, pull-request workflows, automated low-risk decisions, richer historical learning, additional Omarchy surfaces, or remote workers. Configurable verification policy and security or prompt-injection hardening may move earlier when campaign work exposes a concrete safety need.
