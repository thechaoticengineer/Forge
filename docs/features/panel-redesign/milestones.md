# Milestones

**Status: M1 is implemented; M2 is planned.**

The redesign is split into two plans. M1 builds the navigation shell and moves every section into its view. M2 turns the plan into compact rows with a stage detail page. Each milestone becomes one Forge plan. Its first stage turns the milestone's scenarios into executable tests (QML tests under `tests/qml/` or `node --test` suites under `tests/`) that fail before implementation.

## M1: Navigation shell and views

Header, project switcher, tab bar, `⋯` menu and keyboard view switching. `Panel.qml` is split into per-view files. Overview, Activity, Architecture, Features, Queue and Settings are built. Plan temporarily hosts the existing stage list unchanged.

**Implemented.**

Covers: S1, S2, S3, S4, S5, S6, S8, S9, S10, S11, S16, S17, S18, S19, S20, S21, S22

Business tests: `tests/panel_redesign_m1.test.mjs`, `tests/qml/tst_panel_shell.qml`, `tests/qml/tst_panel_overview.qml`, `tests/qml/tst_panel_settings.qml` and `tests/qml/tst_panel_queue.qml`

### Visual verification (2026-09-18)

`quickshell/Panel.qml` was rendered offscreen at 760×760 in a standalone Quickshell harness under `/tmp` (not committed): a `shell.qml` instantiating the working-tree panel, with `Commons`, `Ui` and `services` symlinked to `/usr/share/omarchy/shell/*`, run with `QT_QPA_PLATFORM=offscreen quickshell -p <dir>`. `apiBase` pointed at a read-only local stub that forwarded GET requests to the live engine on port 8734 and refused every POST. Each tab was selected through `currentTab` and captured with `grabToImage` on the window's content item.

- Live engine state (this M1 plan: running, fixer active, 6 committed stages, empty queue): all seven tabs rendered. The running Overview's content is 343 px tall in a 628 px view, so it fits without scrolling. The now-working card, progress summary, all six stage rows and Stop sit inside the visible area.
- Running fixture (live state with 5 stages, stage 3 in progress, 2 queued goals, a second blocked project): Overview content is 303 px in 628 px. It matches `design/views.overview-running.png` row for row: a single-line goal, the now-working card (stage, role · tool · model · lines, elapsed time, latest line), `STAGES 2/5 committed · run · usage` with `Plan ›`, five compact rows (✓ hash and duration, ● `implementer`, · `pending`, each with ›), and then `Stop x`, `View diff d` and `Live output g a`. The header shows `Forge ▾ · 2 projects` and the tab bar shows `Queue 2`, as in the mockup.
- Idle fixtures: with a finished plan, Overview shows the goal field, `Start implementing r`, `Edit plan e` and `⋯ more`, the LAST RUN card with `Plan ›`, `Reports ›` and `View diff`, and the one-line quota summary. This is the layout of `design/views.overview-idle.png`. The mockup's `Create plan`/`Discuss`/`Enhance`/`Add to queue` set appears only for an idle state with no plan and a typed goal (S5, S6). With no plan and an empty goal, Overview shows only the goal field, because the `createPlan` guard needs a goal.
- Settings matches `design/views.settings.png`: AGENTS, REVIEW & RUN (nine label/value rows with ⇄), MODELS & LIMITS and MAINTENANCE. The text spec keeps the full quota, tier, provider and metadata lines in Settings, so this view is taller than the mockup's one-line summary. At 760×760 its content is 652 px in a 628 px view, and MAINTENANCE (`Update Forge`, `Change project`) is reached by scrolling the view.
- Plan hosts the unchanged `PlanEditorView`, with feedback and Q&A above it. Activity fills the view with Live/History. Architecture shows the card, role token totals and plan review status. Features lists the specs with their actions. Queue lists goals with ↑/↓/× and `Start queue`, or its empty-state line. The header and tab bar stay fixed, and the hint line changes per tab in every render.
- Differences from the mockups that were not changed: colours follow the active Omarchy theme, so the running badge and hashes use the theme's accent and urgent colours rather than the mockup's palette, and the card surfaces are close to the background. Theming is out of scope for M1. No layout defects needed fixing.

## M2: Compact plan and stage detail

One-line stage rows and the plan review strip. A stage detail page with sub-tabs and previous/next navigation. Plan editing, Q&A and feedback stay inside Plan. The compact rows reuse the Overview stage row from M1.

Covers: S7, S12, S13, S14, S15, S23
