# Scenarios

Acceptance scenarios below use stable IDs (S1, S2, ...), assigned once and never renumbered or reused. The panel window is the default 760×760 unless a scenario says otherwise. Mockups are in `design/`.

## S1: The header and tab bar are always visible

- Given: the panel is open on any tab, with a plan of at least 5 stages
- When: the user scrolls the current view to its end
- Then: the header (FORGE, project switcher, phase badge, current step, `⋯`) and the tab bar Overview · Plan · Activity · Architecture · Features · Queue · Settings stay in place, and the selected tab is marked

## S2: Clicking a tab shows that view

- Given: the panel is open on Overview
- When: the user clicks each tab in turn
- Then: the matching view replaces the content area, the clicked tab becomes the selected tab, and no other view's content is shown at the same time

## S3: Tabs can be switched from the keyboard

- Given: the panel is open in normal mode on Overview
- When: the user presses `]`, then `[`, then `g` followed by each of `o`, `p`, `a`, `r`, `f`, `q`, `s`
- Then: `]` selects Plan, `[` returns to Overview, and each `g`-sequence selects Overview, Plan, Activity, Architecture, Features, Queue and Settings respectively, while `gg` keeps its existing meaning

## S4: The running Overview fits on one screen

- Given: a run is in progress on stage 3 of a 5-stage plan, with an active agent and retained output
- When: the user views Overview
- Then: the goal, a "now working" card (stage number and title, role, tool, model, elapsed time, latest output line), a one-line progress summary, one line per stage, and a Stop action are all visible without scrolling, and no stage shows routing or review policy lines

## S5: The idle Overview offers goal actions

- Given: no plan exists and the engine is idle
- When: the user views Overview
- Then: the goal field is shown with Create plan, Discuss first, Enhance with AI and Add to queue, and pressing `i` focuses the goal field

## S6: Overview actions follow the phase

- Given: the engine is in each of the phases idle, planning, plan_ready, awaiting_approval, running, blocked, failed and done in turn
- When: the user views Overview
- Then: only actions that are enabled in that phase are shown as buttons there, every other current action remains reachable from its own view or the `⋯` menu, and clicking a shown action calls the same API endpoint as before the redesign

## S7: A stage that needs attention is surfaced on Overview

- Given: a stage is blocked or failed
- When: the user views Overview
- Then: that stage is shown at the top of Overview with its status and reason, and clicking it opens its stage detail page

## S8: Settings holds every run and agent setting

- Given: the panel is open
- When: the user opens Settings
- Then: planner, architect, implementer, reviewer, automatic routing, architect review cadence, reviewer review cadence, push at end and auto-approve are shown as label/value rows, and clicking a row cycles its value through the same API calls the old buttons used

## S9: Settings holds models, limits and maintenance

- Given: the model catalogue and Claude quota are available, and the catalogue reports a policy error
- When: the user opens Settings
- Then: quota lines, catalogue metadata and the policy error are shown there, with Model settings & options, Refresh models (and Cancel refresh while refreshing), Refresh Claude limits, Update Forge and Change project, and none of these appear on Overview

## S10: The overflow menu reaches rare actions

- Given: the panel is open on any tab
- When: the user opens `⋯`
- Then: Update Forge, Discard plan, Refactor plan, View diff, Change project and Keyboard help are listed, disabled items reflect the same guards as today, and choosing one performs the same action as before the redesign

## S11: The project switcher lists sessions

- Given: two projects have sessions, one busy and one blocked
- When: the user opens the project switcher in the header
- Then: both projects are listed with the same status markers used today (● busy, ! needs attention, ✓ done, +N queued), choosing one selects that project, "Change project…" opens the project chooser, and the selected tab stays the same

## S12: Plan shows compact stage rows

- Given: a plan with committed, running and pending stages
- When: the user opens Plan
- Then: each stage takes one line with status icon, number and title, commit hash or status, and duration, and a one-line plan review strip summarizes the review verdict and round

## S13: Clicking a stage opens its detail page

- Given: Plan is open with stage 2 selected
- When: the user clicks stage 3, or selects it with `j` and presses Enter
- Then: a stage detail page opens showing a breadcrumb back to Plan, the stage status line, and sub-tabs Instructions, Acceptance, Review, Routing and Output, which together hold everything the inline expanded stage showed before the redesign

## S14: Stage detail navigation keeps position

- Given: the stage detail page of stage 3 is open after scrolling Plan
- When: the user presses `]` to move to stage 4, then Escape
- Then: `]` moves between stages rather than tabs while stage detail is open, stage 4's detail is shown, and Escape returns to Plan with stage 4 selected and the earlier scroll position intact

## S15: Plan editing, Q&A and feedback live in Plan

- Given: a plan is ready
- When: the user presses `e`, edits a stage and saves, then asks a question in Plan Q&A, then submits feedback with Improve with AI
- Then: each works as before the redesign and stays inside the Plan view

## S16: Activity uses the full view

- Given: an agent is producing output and history, git and review entries exist
- When: the user opens Activity
- Then: Live / History / Reports and the All / Runs / Git / Reviews / Errors / Reports filters fill the view height, and `Tab`, `h`/`l`, `Ctrl+d`/`Ctrl+u` and the digit filters work there as before

## S17: Architecture holds architecture and review status

- Given: an architecture context with guidance, risks and decisions, role usage totals, and a plan review in round 2
- When: the user opens Architecture
- Then: the architecture card, the role token totals and the plan review status with its fix commits are shown there and nowhere on Overview

## S18: Queue and Features are tabs

- Given: two queued goals and at least one feature spec
- When: the user opens Queue, then Features
- Then: Queue shows the list with Start queue, ↑, ↓ and ×, the Queue tab label shows the count, and Features shows the existing feature list with its actions and shortcuts

## S19: Pushed pages return to their tab

- Given: the user is on Plan
- When: the user opens the diff viewer, the discussion chat, model settings, the project chooser or keyboard help, and closes it with q, Escape or Back
- Then: the panel returns to Plan with its selection and scroll position intact

## S20: State survives polling and reopening

- Given: the user is on Activity reading older history
- When: several polling refreshes arrive, and later the panel is closed and reopened
- Then: the tab and reading position do not change during polling, and reopening shows Activity again

## S21: Every existing shortcut still works

- Given: the keyboard help list from before the redesign
- When: each shortcut is pressed in the view that shows its target
- Then: it performs the same action as before, keyboard help lists every old shortcut plus the new view keys, and the bottom hint line names the current view's keys

## S22: Views live in their own files

- Given: milestone M1 is complete
- When: the `quickshell/` directory is inspected
- Then: each tab view is a separate QML file, `Panel.qml` holds engine state, polling, API actions and navigation and is under 1,800 lines, and all `node --test tests/*.test.mjs`, `qmltestrunner -input tests/qml` and `cargo test` checks pass

## S23: Stage detail lives in its own file

- Given: milestone M2 is complete
- When: the `quickshell/` directory is inspected
- Then: the stage detail page is a separate QML file, `Panel.qml` stays under 1,800 lines, and all `node --test tests/*.test.mjs`, `qmltestrunner -input tests/qml` and `cargo test` checks pass
