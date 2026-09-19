# Milestones

**Status: M1, M2, M3 and M4 are implemented; M5 is planned.**

The feature-spec workflow is split into successive milestones (M1-M5). M1's scenarios (S1-S9), M2's scenarios (S10-S19), M3's scenarios (S27-S35) and M4's scenarios (S20-S26) have executable business tests. Each milestone becomes one Forge plan: the plan's first stage turns that milestone's scenarios into executable tests that fail before implementation, and later stages implement the feature until those tests pass.

## M1: Format and discovery

The `docs/features/<slug>/` folder format; feature discovery and validation in the engine; a read-only API exposing discovered features; a panel list of features with an "open in nvim" action.

Status: implemented
Covers: S1, S2, S3, S4, S5, S6, S7, S8, S9

Business tests: src/feature_spec_tests.rs (S1-S6, S9) and tests/panel_features.test.mjs (S7-S8)

## M2: Spec phase

Creating a feature from the template; a co-authoring agent whose writes are restricted to a single feature folder; an architect spec review of the draft; spec and scenario approvals, with the spec committed on approval and approvals recorded in runtime state.

Status: implemented
Covers: S10, S11, S12, S13, S14, S15, S16, S17, S18, S19

Business tests: src/feature_spec_m2_tests.rs (S10-S19) and tests/panel_feature_spec.test.mjs (panel parts of S10-S18)

## M3: Feature to plans

Planning an approved milestone from the panel; supplying compact feature context to the planner, architect and reviewers; a failing-tests-first first stage in each milestone's plan; scenario coverage required during plan review; `Business tests:` lines as the registry of business tests, which every plan keeps passing; business test changes surfaced to reviewers; a completed plan marks its milestone implemented.

Status: implemented
Covers: S27, S28, S29, S30, S31, S32, S33, S34, S35

Business tests: src/feature_spec_m3_tests.rs (S27-S35) and tests/panel_feature_plan.test.mjs (panel parts of S27-S28)

## M4: pen.dev integration

Agents drive `pen interactive` headlessly through the shell, using the pen.dev CLI's bundled skill; a PNG is auto-exported next to every changed `.pen` file; the desktop app's MCP server is used only for manual design work, not by agents. Missing pen CLI or authentication blocks the run; export failures are returned to the fixer like failing checks.

Status: implemented
Covers: S20, S21, S22, S23, S24, S25, S26

Business tests: src/pen_dev_tests.rs (S20-S26)

## M5: Panel viewer

Not yet implemented. Rendered markdown for a feature's documentation; scenarios shown together with their test results; status indicators and approval buttons.

Status: planned
Covers: none yet
