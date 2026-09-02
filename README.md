# Omarchy AI Build Orchestrator

> **Working title:** “Omarchy AI Build Orchestrator” is descriptive and temporary. A final product name has not been chosen.
>
> **Project status:** The first vertical workflow is implemented. The Omarchy panel can create an approved task's isolated worktree, launch a user-selected Codex or Claude implementer, show durable activity, and carry the task through deterministic checks, independent review, correction, and exact final-tree inspection. The CLI and panel show the complete patch, changed files, gate evidence, and proposed one-task Conventional Commit before a separate keyboard-accessible approval or rejection. Approval refuses a worktree that changed after inspection and creates only a local isolated-worktree commit. A second confirmed action can fast-forward a checked-out clean local branch to that commit, and the CLI can install the engine as a managed user service. Configurable project verification, semantic multi-commit splitting, divergent merges, push, and deployment remain planned.
>
> **Implementation progress:** See [ROADMAP.md](ROADMAP.md) for the ordered delivery plan and first-milestone checklist.

## Vision

Omarchy AI Build Orchestrator is intended to become a local **software workshop** for building software with Codex CLI and Claude Code CLI.

The developer brings an idea, defect, or engineering goal into the workshop. The orchestrator turns that intent into explicit work: a plan that can be edited, tasks that can be isolated, changes that can be verified, an implementation that can be independently reviewed, and a history that explains what happened. The product is not merely an AI chat, a CLI wrapper, a task runner, or a better viewer for terminal logs. It is the environment in which the whole software-building process is coordinated.

The core outcome is:

> Turn an idea or engineering task into a traceable, tested, independently reviewed software change while keeping the developer in control.

Over time, the workshop should help build and improve itself using the same planning, isolation, verification, review, and approval process it applies to other projects. Each reliable improvement should make the next project faster to build without weakening human control.

## Problem

Codex CLI and Claude Code CLI are individually capable, but using both well still demands substantial manual coordination. A developer currently has to move context between terminal sessions, decide who plans and who implements, create and clean up worktrees, run checks, interpret failures, arrange an independent review, send findings back for correction, and remember why a decision was made.

That coordination is easy to lose in terminal scrollback and conversational history. An agent may report success even though the authoritative command failed. One agent may review its own assumptions. Existing user changes may be put at risk. A useful rejected approach may disappear even though it explains the final design.

The orchestrator should give this work an explicit, durable structure. Agent statements remain useful evidence, but exit codes, command output, diffs, repository state, review findings, and human decisions determine whether a change is ready.

## Product Experience

Using the product should feel like entering a focused workshop, not opening another chat client.

The default view should show the state of the work and the next decision. It should immediately answer:

- What is running now, and which agent owns it?
- What is complete, queued, blocked, failed, or waiting for me?
- What failed, and what evidence explains the failure?
- Which files changed?
- Did compilation, tests, formatting, linting, and analysis pass?
- What did the independent reviewer find?
- What will be committed if I approve the result?

The user should be able to move from the high-level run summary into the plan, a task, an agent activity stream, a command result, a changed file, a review finding, or the exact worktree. Raw Codex and Claude Code output should remain available for diagnosis and audit, but it should not dominate the primary interface.

The intended rhythm is deliberate: describe a goal, shape the plan, observe implementation, respond when judgment is needed, inspect the evidence, and approve or reject the result.

## Technology Direction

The initial technology direction has two cooperating product parts:

- **Rust orchestration engine and CLI:** orchestration, process management, durable state, Git and worktree operations, Codex and Claude Code adapters, deterministic checks, review loops, and the command-line interface.
- **Omarchy Quickshell/QML plugin:** the primary graphical interface, including a small bar widget and a richer panel integrated into the existing Omarchy shell.

An optional Rust-based TUI may be added later. It would be another interface to the same engine, not a separate product and not the primary Omarchy experience. The product is not designed as TUI-only.

The primary frontend will not be GTK4, Electron, a browser dashboard, or a generic standalone cross-platform GUI. It should be native to the current Omarchy Quickshell plugin architecture and run inside the existing long-lived `omarchy-shell` process.

The split is intentional:

```text
Omarchy Quickshell/QML plugin
            ↕
      local IPC boundary
            ↕
     Rust orchestration engine
            ↕
 Codex CLI, Claude Code CLI, Git,
 builds, tests, analyzers and state
```

The QML plugin should remain a responsive presentation and interaction layer. It must not directly own long-running agents, builds, tests, analyzers, Git operations, or durable project state. Those responsibilities belong to the Rust engine, where they can survive a shell reload and be tested independently.

## Codex and Claude Code Collaboration

The first version will support exactly two agent integrations:

- Codex CLI;
- Claude Code CLI.

Both adapters should use the developer's existing authenticated command-line installations. The initial scope does not include direct model APIs, local models, OpenCode, or additional providers.

Codex and Claude Code should be treated as collaborating but independent workers. Their roles are task-dependent:

- Codex can implement while Claude Code reviews.
- Claude Code can implement while Codex reviews.
- One agent can prepare a plan while the other challenges its assumptions.
- Concrete review findings can be returned to the implementing agent for correction.
- The user can redirect either agent or add context without restarting the entire run.

An agent must never be the sole reviewer of its own implementation. Independent review does not replace deterministic verification or human approval; it adds a second reasoning path that can catch incorrect assumptions, incomplete work, unsafe changes, and maintainability problems.

The first workflow should keep routing simple and visible: the user or a small deterministic policy chooses the planner, implementer, and reviewer. Smarter routing is a later capability. It may eventually consider task type, agent availability, retained context, previous results, and available usage limits, but none of that is required for the first complete workflow.

The implemented planning slice keeps that choice explicit. The engine launches the selected authenticated CLI non-interactively with read-only planning permissions, sends its prompt over standard input, requires a shared structured plan schema, and validates the result before storing it. It does not select a model or use a direct model API. Implementation agents receive writes only inside an existing task worktree and remain engine-supervised. Pause, cancellation, redirects, and added context preserve durable evidence and partial work. The explicit finish operation runs detected deterministic checks, supplies their evidence to a fresh independent reviewer, can launch a bounded fresh implementer correction, and then prepares an exact final inspection record. A separate approval revalidates the inspected tree before creating one local task-worktree commit; rejection preserves the result without committing it. Neither action merges or pushes. See [ADR-0010](docs/adr/0010-run-independent-reviews-in-fresh-agent-sessions.md), [ADR-0011](docs/adr/0011-gate-local-task-commits-on-verification-and-independent-review.md), and [ADR-0012](docs/adr/0012-separate-final-inspection-from-commit-approval.md).

## Omarchy and Quickshell Integration

### Compatibility baseline

This vision has been checked against the locally installed Omarchy `4.0.2-1` shell documentation and source, including:

- `$OMARCHY_PATH/shell/README.md`;
- `$OMARCHY_PATH/shell/plugins/README.md`;
- `$OMARCHY_PATH/shell/plugins/bar/README.md`;
- the installed plugin registry, shell loader, shared QML components, theme singletons, and notification service.

Omarchy currently runs one long-lived Quickshell process named `omarchy-shell`. The bar, widgets, panels, menus, overlays, notifications, and desktop services are hosted inside that process as plugins. The orchestrator frontend should follow that architecture instead of launching a second standalone Quickshell application.

### Third-party shell plugin

The frontend should be an installable third-party Omarchy shell plugin:

- written in QML for Quickshell;
- declared by a schema-version `1` `manifest.json`;
- installed under `~/.config/omarchy/plugins/<plugin-id>/`;
- distributed as a Git repository that can be installed with `omarchy plugin add <git-url>`;
- enabled, disabled, updated, and removed through the supported `omarchy plugin` workflow;
- reloadable by saving files in the user plugin directory, with `omarchy-shell shell rescanPlugins` available to force discovery;
- integrated through the documented manifest, loader, bar-widget, panel, and `omarchy-shell` IPC contracts;
- implemented without modifying or copying built-in files under `$OMARCHY_PATH`.

The working plugin ID is `dev.omarchy-ai-build-orchestrator`. It is temporary but valid for the installed registry: it is namespaced, contains no path components, and does not use the reserved `omarchy.*` namespace.

The installed API documents both `bar-widget` and `panel` as supported plugin kinds, and the registry accepts multiple kinds in one manifest. The planned manifest should therefore expose:

- a `bar-widget` entry point for compact, always-available run status;
- a `panel` entry point for the full workshop interface.

The panel can be summoned, hidden, or toggled through the current shell IPC contract. During development, the documented toggle path will have this shape:

```text
omarchy-shell shell toggle dev.omarchy-ai-build-orchestrator '{}'
```

A Hyprland/Omarchy binding should provide a fast toggle, but no specific global shortcut is chosen here. The implementation must inspect the user's effective bindings and current Omarchy defaults before selecting or suggesting one.

The bar widget should make meaningful states visible at a glance: active, blocked, failed, completed, and waiting for the user. Important events that need attention should use the desktop notification path already handled by Omarchy's notification service. Notifications should draw the user back to the relevant run or decision rather than duplicating the whole interface.

Removal must leave the base installation unchanged. `omarchy plugin remove dev.omarchy-ai-build-orchestrator` should remove the plugin checkout and its shell registration without touching built-in Omarchy source. Engine state and binaries require their own explicit lifecycle and must never be hidden inside modifications to `$OMARCHY_PATH`.

### Thin and defensive QML

Third-party plugins are unsandboxed code inside the long-lived shell process. That makes frontend restraint a safety requirement:

- QML should render state, collect input, and send structured requests across the local boundary.
- Expensive parsing, agent processes, builds, analyzers, and Git work must stay in Rust.
- Malformed or partial engine data must produce a contained UI error, not destabilize `omarchy-shell`.
- A plugin reload or QML failure must not cancel an agent, corrupt project state, or erase history.
- The UI must reconnect, load a current snapshot, and resume live updates after either side restarts.

### Omarchy-native visual design

The frontend should visually and behaviorally belong to Omarchy. It should use the installed shell's shared color and structural style data rather than ship one hardcoded palette. In the current shell, the shared `Color` and `Style` QML singletons expose live theme surface colors, typography, spacing, state treatments, bar dimensions, corner radius, and outer gaps; the installed `qs.Ui` components provide reusable panel and control conventions.

The plugin should:

- follow live theme changes supported by the shell;
- use current Omarchy font and icon conventions;
- reuse documented or installed common QML components where they fit;
- follow Omarchy spacing, borders, corner radii, focus states, shadows, and restrained surfaces;
- remain readable across light and dark themes and unusual foreground/background combinations;
- reserve status colors for meaningful state rather than decoration;
- keep motion subtle, short, and connected to state changes;
- adapt to horizontal and vertical bars, multiple monitor sizes, and display scaling;
- feel natural beside Hyprland's tiled workspaces rather than imitate a conventional floating desktop app.

The project should not fork built-in QML merely to copy its appearance. Compatibility with shared components must be validated against the installed Omarchy version as the shell evolves.

## Core Workflow

The intended end-to-end workflow is:

1. The user opens a local Git repository or explicitly clones a selected GitHub repository.
2. The user describes a goal in natural language.
3. An agent analyzes the repository and proposes a plan.
4. The plan is shown as explicit tasks with acceptance criteria.
5. The user approves, edits, reorders, or rejects the plan.
6. Implementation is assigned to Codex or Claude Code.
7. Each task runs in an isolated Git worktree.
8. The orchestrator streams agent activity and structured progress to the UI.
9. The Rust engine runs deterministic build, test, formatting, linting, and static-analysis commands.
10. The other agent performs an independent review.
11. Review findings return to the implementing agent when corrections are required.
12. The system stops and asks the user when product, architecture, security, destructive operations, or conflicting agent opinions require human judgment.
13. The user inspects the final diff, verification results, review findings, proposed commits, and run history.
14. The user approves or rejects the result.

Compilation, tests, formatting, linters, analyzers, and Git commands are deterministic operations. The engine should record the exact invocation, working directory, relevant environment, output, duration, and exit code. It should not ask an AI agent to guess whether a command succeeded when the operating system already provides an authoritative result.

## UI and Keyboard Interaction

### Main panel

The main panel should be a first-class product interface, not a transcript viewer. It should provide:

- project and repository selection;
- current and previous runs;
- the approved plan, acceptance criteria, and task dependency graph;
- live Codex and Claude Code activity with clear ownership;
- running, queued, blocked, failed, completed, and waiting states;
- changed files and diff inspection;
- build, test, formatting, lint, and analyzer results;
- independent review findings and correction loops;
- proposed semantic commits;
- human decisions, interventions, and unresolved questions;
- an input for additional context or agent redirection;
- a searchable command palette;
- contextual keyboard actions.

The default screen should summarize progress and prioritize actionable information. Detailed prompts, responses, stdout, stderr, and internal events remain one level deeper for users who need to investigate.

The implemented panel is task-centered. Its persistent queue shows every proposed or approved task, while the selected task workspace keeps acceptance criteria, dependencies, worktree state, implementer activity, failures, next action, changes, verification, independent review, and final decisions in one context. Before approval the queue is an editable proposal; after approval it is the actionable backlog. A complete patch opens as contextual final inspection owned by the selected task instead of making changes, verification, and review peer top-level destinations. Because every task branch currently starts from the run's shared base revision, tasks with dependencies are visibly blocked until task results can be composed without dropping prerequisite changes; root tasks remain executable. Raw verification-output drill-down and semantic multi-commit splitting remain planned. See [ADR-0015](docs/adr/0015-center-the-panel-on-tasks.md).

### Keyboard-first operation

The complete workflow must be usable without a mouse. Mouse interaction may be supported, but no important operation should require it.

Vim-style navigation should be used where it is natural and consistent with the current Omarchy panel conventions:

- `j` / `k` move through items;
- `h` / `l` move between adjacent areas or related views;
- `Enter` opens, activates, or confirms the focused action;
- `Esc` returns, cancels a transient mode, or closes the panel;
- `/` opens search or filtering;
- `:` opens the command palette.

Plan approval, diff inspection, verification results, retries, agent instructions, and final approval should have discoverable shortcuts. The user should not need to memorize them: the panel should show context-sensitive actions, and every important command should be searchable in the palette.

Destructive or consequential actions must not rely on single ambiguous keystrokes. Their confirmation should state the target, effect, and recovery implications.

The implemented panel provides two explicit focus areas: the task queue and the selected task workspace. `Right` or `l` enters the workspace, while `Left` or `h` returns to the queue. `Up`/`Down` and `j`/`k` move through tasks or scroll contextual inspection, and `Enter` opens or activates the focused task action. Text fields retain normal character input.

The repository chooser presents two searchable areas: local Git worktrees discovered below configured project roots and accessible GitHub repositories that are not already local. The initial project root is `~/Projects`; repeated `--projects-root /absolute/path` engine options can replace it. Local scanning is bounded and GitHub discovery uses the developer's existing authenticated `gh` CLI. `h`/`l` or Left/Right switches areas, `j`/`k` or Up/Down selects a repository, `/` focuses shared search, and `Enter` opens the local repository or explicitly clones the selected GitHub repository.

The persistent task rail keeps the selected project, original task, and current workflow stage visible. Before work starts, those stages distinguish a task that is **To plan**, a generated proposal that is **Planned**, and an approved task that is **Ready to run**. The same rail exposes project choice and switching without requiring a hidden setup view.

Clones use `<projects-root>/<repository>` by default and fall back to `<projects-root>/<owner>/<repository>` on a name collision. The engine refuses to overwrite either destination. Manual absolute-path entry remains available with `p`; there, `Tab` requests directory completion from the Rust engine. All discovery and clone operations cross the structured local IPC boundary; QML does not enumerate the filesystem or launch Git and GitHub processes inside `omarchy-shell`. See [ADR-0005](docs/adr/0005-browse-local-and-github-repositories.md).

## Architecture Boundaries

All interfaces must share one orchestration model and one durable state. Business logic belongs in the Rust engine, not in QML, CLI presentation code, or a future TUI.

Significant architectural choices and their reasoning are preserved as [Architecture Decision Records](docs/adr/). ADRs complement this product vision by recording the context, alternatives, and consequences behind decisions as the implementation evolves.

| Boundary | Planned responsibility |
| --- | --- |
| Rust orchestration core | Run lifecycle, task graph, state transitions, assignment, review/correction loops, policy, and approvals |
| Codex CLI adapter | Launch and supervise the authenticated Codex CLI, translate events, preserve prompts and results, and support cancellation/redirection |
| Claude Code CLI adapter | Launch and supervise the authenticated Claude Code CLI under the same adapter contract |
| Git and worktree management | Inspect repositories, protect user changes, create isolated worktrees, calculate diffs, and prepare proposed commits |
| Process execution | Run deterministic commands with controlled working directories, captured output, exit codes, timeouts, and cancellation |
| Persistent run state | Store goals, plans, tasks, events, evidence, decisions, recovery points, and outcomes |
| Local IPC boundary | Exchange structured commands, snapshots, events, acknowledgements, health, and reconnection state |
| Quickshell/QML plugin | Present the native Omarchy panel and bar widget; collect keyboard and pointer input; reconnect after reload |
| Rust CLI | Expose the same projects, runs, actions, evidence, and approvals for shell use and automation |
| Optional Rust TUI | Offer a terminal-native view over the same engine without duplicating workflow logic |

The initial IPC transport is defined by [ADR-0002](docs/adr/0002-use-versioned-json-over-a-unix-socket.md): versioned, newline-delimited JSON over an owner-only Unix-domain socket below `$XDG_RUNTIME_DIR`. The engine sends an authoritative snapshot when a client connects, and clients reconnect and replace their local view after interruption. The evolving message contract must remain:

- explicit and local;
- structured and versionable;
- observable enough to diagnose failures;
- permissioned appropriately for local state-changing actions;
- resilient to partial messages, backpressure, disconnects, and duplicate delivery;
- able to recover when the engine or frontend restarts independently.

The engine should continue safely if the panel closes or the plugin reloads. The frontend should reconnect, request a current snapshot, reconcile any missed events, and resume live updates without pretending that a new run started.

## Human Control and Safety

The developer remains the final authority. The product should make human intervention efficient, well-informed, and visible rather than treat it as a failure of automation.

The user must be able to:

- inspect and edit plans before execution;
- see which agent performed every action;
- open the exact worktree used by an agent;
- inspect commands, output, exit codes, and timing;
- pause, retry, redirect, or cancel work;
- provide additional context without restarting the run;
- reject an agent's plan or implementation approach;
- inspect proposed commits before they are created;
- approve or reject the final result.

The engine should prefer isolated, reversible operations and preserve pre-existing repository state. It must not silently:

- overwrite uncommitted or untracked user work;
- delete worktrees, branches, files, or run history that may still be needed;
- force-push or otherwise alter a remote;
- amend, squash, rebase, reset, or rewrite user history;
- merge branches;
- deploy software;
- execute destructive commands;
- resolve important disagreements between agents;
- cross an approval boundary because an agent claims the action is safe.

The system should stop for human judgment when intent is unclear, risk is material, permissions expand, product behavior is subjective, architecture has lasting consequences, a security concern is unresolved, or the two agents reach conflicting conclusions that evidence cannot settle.

## State and Traceability

Every run should preserve enough durable information to reconstruct both the result and the path taken:

- the original goal;
- repository identity, starting revision, branch, and working-tree condition;
- proposed, edited, rejected, and approved plans;
- tasks, acceptance criteria, and dependencies;
- agent assignments and role changes;
- important prompts, responses, summaries, and structured events;
- commands, working directories, output, durations, and exit codes;
- build, test, formatting, linting, and analysis results;
- changed files and diffs at meaningful checkpoints;
- review findings, severity, disposition, and evidence;
- corrections, retries, failed approaches, and abandoned paths;
- human instructions, approvals, rejections, and decisions;
- proposed and created commits;
- the final outcome and any unresolved follow-up.

Run state belongs to the Rust engine and its durable storage, not to terminal scrollback or QML object lifetime. Restarting `omarchy-shell`, hot-reloading the plugin, hiding the panel, closing a terminal, or reconnecting the CLI must not erase or silently reset a run.

The implemented storage foundation uses SQLite through the Rust engine. By default, its database is stored at `$XDG_STATE_HOME/omarchy-ai-build-orchestrator/state.db`, falling back to `~/.local/state/omarchy-ai-build-orchestrator/state.db`. The current schema preserves projects, draft runs, planner attempts and failure evidence, versioned plans, tasks, acceptance criteria, dependencies, human plan decisions, task worktree records, supervised implementation attempts, bounded output and recent categorized activity, append-only audit events, and artifact metadata. It is intentionally only a slice of the broader history described above. See [ADR-0003](docs/adr/0003-store-state-in-sqlite.md) for the storage decision and safety boundaries.

Each implementation task of an approved plan can be given its own Git worktree and its own branch under the reserved `orchestrator/` prefix. Worktrees are created below the engine's state directory rather than inside the user's repository, always start from the run's recorded base revision, and are recorded durably before Git runs so that an interrupted engine leaves a retryable record. The engine refuses rather than forces: an existing branch, an occupied destination, a missing base revision, or an already-registered path stops the operation, and a failed attempt removes only what it reserved. Uncommitted work in the user's checkout is never read, moved, or absorbed; it is recorded so the interface can show that the agent works from the committed base revision instead. See [ADR-0006](docs/adr/0006-isolate-task-implementation-in-git-worktrees.md) for the lifecycle, cleanup, and recovery boundaries.

After recovery, the user should be able to understand what completed, what may have been interrupted, which operations are safe to retry, and what still requires attention. Recovery should favor explicit state reconciliation over replaying side effects blindly.

Traceability is also a product feature. A run history should explain why a plan changed, why a finding was accepted or rejected, why a command was retried, and which evidence supported final approval.

## Engine Installation and Lifecycle

Install both Rust binaries, then let the CLI create and start the user-scoped
systemd service:

```text
cargo install --locked --path crates/orchestrator-engine
cargo install --locked --path crates/orchestrator-cli
build-orchestrator engine install
```

The generated unit stores absolute paths for the engine and the authenticated
Codex, Claude Code, and GitHub CLI installations, starts at the user's default
target, and restarts only after failure. Reinstall the service after changing
those executable locations or project roots. Rebuild or reinstall the binaries
and run `systemctl --user restart omarchy-ai-build-orchestrator.service` after
updating their code.

```text
build-orchestrator engine status
build-orchestrator engine uninstall
```

Uninstalling the service stops it and removes only its user unit. It preserves
the SQLite state, artifacts, task worktrees, task branches, and the independently
managed Omarchy plugin. See [ADR-0014](docs/adr/0014-manage-the-engine-as-a-systemd-user-service.md).

## Initial Scope

The first version should be deliberately narrow while still delivering the complete product shape:

- local Omarchy systems and local Git repositories;
- the Rust engine, Rust CLI, and real Quickshell/QML frontend;
- Codex CLI and Claude Code CLI using existing authenticated installations;
- one visible plan followed by user approval;
- one implementing agent and the other agent as independent reviewer;
- isolated Git worktrees for tasks;
- project-defined deterministic verification commands;
- one correction loop that can be repeated under user control;
- durable local state, history, recovery, and reconnect behavior;
- final diff, evidence, review, and proposed commit inspection.

Parallel scheduling, predictive routing, remote execution, and automatic low-risk decisions are not prerequisites for this version. The goal is one dependable vertical workflow, not a large framework.

## Git and Commit Conventions

The project uses **Conventional Commits** to create semantic, readable Git history:

```text
<type>(optional-scope): <short imperative description>
```

Default types:

- `feat` — new functionality;
- `fix` — bug fix;
- `docs` — documentation;
- `refactor` — restructuring without changing behavior;
- `test` — tests;
- `perf` — performance improvements;
- `build` — build system or dependency changes;
- `ci` — CI/CD changes;
- `chore` — maintenance.

Examples relevant to this project:

```text
feat(core): add agent task scheduler
feat(quickshell): add active run panel
feat(bar): show pending user decision
fix(worktree): preserve uncommitted user changes
fix(ipc): reconnect after shell reload
docs: describe the Omarchy plugin architecture
refactor(git): isolate worktree creation
test(review): cover failed review loop
```

Commit rules:

- keep one logical change per commit;
- use a short imperative description;
- use a scope when it adds useful context;
- explain why in the body when the reason is not obvious;
- commit only after relevant verification succeeds;
- separate refactoring from functionality where practical;
- mark breaking changes explicitly;
- show proposed commits to the user before creating them;
- never amend, squash, rebase, force-push, or rewrite user history without explicit permission.

The orchestrator should eventually help agents divide work into meaningful semantic commits as changes are developed instead of producing one large commit at the end. Proposed boundaries and messages remain subject to user review.

## First Milestone

The first milestone is a complete vertical slice that includes both the Rust engine and a real Omarchy UI. It is successful when the user can:

1. Install or enable the Quickshell plugin on Omarchy.
2. Summon the orchestrator from the Omarchy environment.
3. Open a local Git repository or clone one from GitHub through the panel.
4. Describe a small software change.
5. Inspect and approve the proposed plan.
6. Watch Codex or Claude Code implement it in an isolated worktree.
7. See build and test status update through the Omarchy UI.
8. Receive an independent review from the other agent.
9. Inspect the final diff and proposed semantic commit.
10. Approve or reject the result using only the keyboard.
11. Reload the plugin or restart the shell without losing the run.
12. Use the bar widget to see whether work is active or waiting for attention.

CLI commands may expose the same workflow, but CLI completion alone does not satisfy this milestone. The Quickshell panel and bar widget are part of the first usable product slice.

The current completion state and the ordered work needed to close this milestone are tracked in [ROADMAP.md](ROADMAP.md).

## Non-Goals

The first version is not:

- a cloud SaaS platform;
- a generic cross-platform desktop application;
- an IDE replacement;
- a general frontend for every AI model;
- a fully autonomous software company;
- a system that merges or deploys important changes automatically;
- a large distributed multi-agent framework;
- a collection of unrelated agent chats;
- a modification or fork of Omarchy itself.

It also does not need direct provider APIs, local model hosting, arbitrary agent plugins, automatic pull-request operation, or a standalone primary GUI.

## Future Direction

After the first workflow is dependable, later capabilities may include:

- parallel task execution in controlled isolated worktrees;
- smarter routing between Codex and Claude Code;
- agent availability, usage, and limit awareness;
- pull-request creation and monitoring;
- security, dependency, permission, and prompt-injection checks;
- configurable project policies and quality gates;
- automated decisions for explicitly defined low-risk review findings;
- richer project history and learning from previous runs;
- additional Omarchy widgets, panels, and notification actions;
- remote workers while preserving the local Omarchy control surface and approval model.

These are directions, not initial requirements. Each should be evaluated by whether it makes the workshop more reliable, understandable, and effective without sacrificing local ownership, independent review, deterministic evidence, or human control.
