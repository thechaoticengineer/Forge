# Milestones

**Status: M1 and M2 are implemented.**

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

**Implemented.**

Business tests: `tests/panel_redesign_m2.test.mjs`, `tests/qml/tst_panel_plan.qml`, `tests/qml/tst_panel_stage_detail.qml`, `tests/qml/tst_panel_overview_attention.qml` and `tests/qml/tst_panel_settings_compact.qml`

The inline stage expansion moved into `quickshell/StageDetailPage.qml`, so the source-inspection tests that sliced it (`tests/panel_review_details.test.mjs`, `tests/panel_review.test.mjs`, `tests/panel_routing.test.mjs`, `tests/panel_components.test.mjs` and the `tests/run_stage_details.py` fixture) were retargeted to the new files without weakening their assertions.

### Visual verification (2026-09-18)

`quickshell/Panel.qml` was rendered offscreen at 760×760 with the same standalone harness as M1 (under `/tmp`, not committed): a `shell.qml` instantiating the working-tree panel, `Commons`, `Ui` and `services` symlinked to `/usr/share/omarchy/shell/*`, `QT_QPA_PLATFORM=offscreen quickshell -p <dir>` and `grabToImage` on the window's content item. `apiBase` pointed at a local stub that forwarded GET and HEAD requests to the live engine on port 8734 and answered every POST, PUT, DELETE and PATCH with 403, so no engine state changed. Stage detail was opened with `openStageDetail(i)` and each sub-tab was selected through `stageDetailTab`.

Real engine state (this M2 plan, running; stages 1-4 committed, stage 5 in progress): Plan, and the stage detail page of a committed stage (stage 4) and of the in-progress stage (stage 5), each on all five sub-tabs. Fixture state, used only for display states the live engine did not have (a copy of the live state served by the stub): a blocked stage and a pending stage, an approved plan review (`round 2`, architect and independent approving), and a three-window Claude/Fable quota. The live state had no blocked or failed stage, no plan review and no quota, so the Overview attention line, the plan review strip and the Settings quota lines were verified on the fixture only. A finished-plan state from M1 was also rendered for the idle Plan actions.

- Plan: five or six rows, each exactly 24 px tall on one line: status icon, number and title, commit hash (or `blocked`, `pending`, or the current activity) and duration, then `›`. No routing, model or review policy text appears on the list. The summary line reads `N stages · M committed · review: per plan`, followed by the phase actions (`Start implementing`, `Edit plan`, `Q&A`). With a plan review the one-line strip reads `Plan review · approved · round 2 of 4 · architect ✓ independent ✓` with `details ›`. The feedback field and the Plan Q&A section follow. After opening stage 5 and closing the page, Plan returned with row 5 selected.
- Stage detail: the breadcrumb `‹ Plan / 4. title`, `‹ 3 · 5 ›`, the status line (icon, status, hash, elapsed time, routing summary) and the sub-tab bar all fit on their lines at 760 px. Instructions shows the instructions, the commit message and the View diff, Live output and Copy buttons (reached by scrolling the page's own content area for long instructions). Acceptance, Review (policy line, gate outcomes, rationale), Routing (model status lines, planner and architect rationale, the routing details toggle) and Output (activity, usage, Live output) each showed their content. The header and tab bar stay visible above the page, and the hint line reads `h/l sub-tab · [ ] prev/next stage · q/Esc back to Plan`.
- Overview with a blocked stage: `! stage 5 · title · blocked — fix rounds exhausted ›` appears as one line directly under the goal, above the now-working card and the stage rows.
- Settings: each quota window takes exactly one line (`Claude · 5h 65% remaining · resets Sat 02:17`, none truncated) and the view's content is 600 px in a 628 px view, so MAINTENANCE (`Update Forge`, `Change project`) is visible without scrolling. With **▸ details** expanded the content grows to 647 px, and scrolling to the end brings `Update Forge` and `Change project` fully into view.
- Layout defects fixed: (1) the stage stepper showed `‹ 4    6 ›` without the mockup's `·` between the numbers, so a separator was added to `StageDetailPage.qml`; (2) the placeholders of the feedback and Plan Q&A fields were not vertically centred and their descenders were clipped (this was already the case in the M1 render), so `PlanView.qml` now centres them.
- Differences from the mockups that were not changed: colours follow the active Omarchy theme, so the selected row shows a filled highlight rather than the mockup's outline and the selected sub-tab and the review strip use a surface fill that is close to the background (theming is out of scope). The mockup's Plan summary has a `Plan review ›` button, but the strip below the rows does that job. Plan shows a `Q&A` button in the summary row and also keeps the collapsible **Plan Q&A** section with the question field under the feedback field, because Q&A and its shortcuts were kept as before (S15). The status line elides the routing summary with `…` when it is long; the full text is on the Routing sub-tab. Long unbroken instruction text wraps anywhere, as the shared prose control did before M2.
