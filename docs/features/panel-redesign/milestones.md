# Milestones

The redesign is split into two plans. M1 builds the navigation shell and moves every section into its view. M2 turns the plan into compact rows with a stage detail page. Each milestone becomes one Forge plan. Its first stage turns the milestone's scenarios into executable tests (QML tests under `tests/qml/` or `node --test` suites under `tests/`) that fail before implementation.

## M1: Navigation shell and views

Header, project switcher, tab bar, `⋯` menu and keyboard view switching. `Panel.qml` is split into per-view files. Overview, Activity, Architecture, Features, Queue and Settings are built. Plan temporarily hosts the existing stage list unchanged.

Covers: S1, S2, S3, S4, S5, S6, S8, S9, S10, S11, S16, S17, S18, S19, S20, S21, S22

## M2: Compact plan and stage detail

One-line stage rows and the plan review strip. A stage detail page with sub-tabs and previous/next navigation. Plan editing, Q&A and feedback stay inside Plan. The compact rows reuse the Overview stage row from M1.

Covers: S7, S12, S13, S14, S15, S23
