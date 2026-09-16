# Milestones

**Status: planned; the engine does not implement any of this yet.**

The feature-spec workflow is split into successive milestones (M1-M5). Each milestone becomes one Forge plan: the plan's first stage turns that milestone's scenarios into executable tests that fail before implementation, and later stages implement the feature until those tests pass.

## M1: Format and discovery

The `docs/features/<slug>/` folder format; feature discovery and validation in the engine; a read-only API exposing discovered features; a panel list of features with an "open in nvim" action.

Scenarios: S1, S2, S3, S4, S5, S6, S7, S8, S9

## M2: Spec phase

A co-authoring agent whose writes are restricted to a single feature folder; an architect spec review of the draft; approvals committed to runtime state.

Scenarios: to be written before M2 is planned.

## M3: Feature to plans

Splitting a feature into milestones; supplying feature context to the planner, architect and reviewer; a failing-tests-first first stage in each milestone's plan; scenario coverage required during plan review.

Scenarios: to be written before M3 is planned.

## M4: pen.dev integration

Agents drive `pen interactive` headlessly through the shell, using the pen.dev CLI's bundled skill; a PNG is auto-exported next to every changed `.pen` file; the desktop app's MCP server is used only for manual design work, not by agents.

Scenarios: to be written before M4 is planned.

## M5: Panel viewer

Rendered markdown for a feature's documentation; scenarios shown together with their test results; status indicators and approval buttons.

Scenarios: to be written before M5 is planned.
