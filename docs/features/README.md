# Feature specs

**Status: M1 (format and discovery) and M4 (pen.dev integration) are implemented.** The Forge engine discovers `docs/features/<slug>/` folders, validates required files and scenario structure, and exposes them via a read-only `GET /api/features` endpoint. Agents edit pen.dev `.pen` files headlessly via the shell with the bundled skill, and the engine exports PNGs after each editing turn before review and commit. Milestones M2 (spec phase), M3 (feature to plans), and M5 (panel viewer) remain planned. Runtime state storage (`.forge/features/<slug>.json`) and the existing goal, discussion, planning, execution and queue workflows remain unchanged.

## Purpose

Features are specified in the repository under `docs/features/<slug>/` before they are planned, so that goal, scope, acceptance scenarios, decisions and milestone split are reviewable and versioned. The `<slug>` is a short lowercase, hyphen-separated folder name. Folders whose names start with `_` (such as `_template`) are not features.

## Folder format

Each feature specification occupies a directory under `docs/features/<slug>/` with the following structure:

```
docs/features/<slug>/
  README.md           goal, scope, out of scope, behavior, open questions
  design/
    panel-list.pen    pen.dev mockup
    panel-list.png    exported PNG matching the .pen file
    workflow.mmd      Mermaid diagram (or fenced ```mermaid in .md)
  scenarios.md        acceptance scenarios with stable IDs (S1, S2, ...)
  decisions.md        agreed decisions
  milestones.md       split into successive plans (M1, M2, ...) with scenario IDs
```

**README.md** contains the feature's goal, scope and out-of-scope boundaries, intended behavior, and any open questions. It is the entry point for reviewers. Its first `# ` heading (outside fenced code blocks) is the feature's title; a README without one is titled by its slug. The engine validates that README.md exists and is readable as UTF-8.

**design/** holds UI mockups as `.pen` files (pen.dev format), each with an exported PNG next to it using the same base name (e.g. `panel-list.pen` and `panel-list.png`; a design with several top-level frames exports one `panel-list.<frame-slug>.png` per frame). The engine exports PNGs headlessly after each agent editing turn; agents must not hand-edit PNGs. Diagrams are stored as Mermaid files (`.mmd`) or fenced ` ```mermaid ` blocks in Markdown.

**scenarios.md** lists acceptance scenarios in Given/When/Then format, each with a stable ID (`S1`, `S2`, ...). IDs are never renumbered or reused once assigned, so that milestones, plans and executable tests can reliably refer to them. The engine validates that scenario IDs follow the form `## S<number>:` (or `## S<number>` without colon), where `<number>` is one or more ASCII digits. Duplicate or malformed scenario IDs are reported as validation errors. Example:

```
## S1 User opens the feature

Given: the panel is open and focused
When: the user presses Ctrl+O
Then: the feature dialog appears with focus on the first input
```

**decisions.md** records agreed decisions (what was decided and why), in any format suitable for your project.

**milestones.md** splits the feature into successive plans, each labelled M1, M2, etc., and lists the scenario IDs that each milestone covers. The engine validates that each `Covers:` line is followed by either `none yet` or a comma-separated list of scenario IDs that are defined in scenarios.md. Invalid or unknown scenario IDs are reported as validation errors. For example:

```
## M1 Discover and validate

Covers: S1, S2, S3
```

A milestone whose scenarios are not written yet uses `Covers: none yet`.

Empty templates of these files are available in `docs/features/_template/` to copy as a starting point.

## Intended flow

The feature specification flow proceeds in order:

1. **Draft** — the spec folder is created and written with README, design sketches, scenarios and initial decisions.
2. **Architect spec review** — the architect reviews the proposed goal, scope, scenarios and decisions for consistency and feasibility.
3. **Scenario approval** — scenarios are reviewed and approved as acceptance criteria.
4. **Milestones** — the feature is split into successive milestones, each covering a subset of the scenarios.
5. **One Forge plan per milestone** — for each milestone:
   - The first stage of the plan turns that milestone's scenarios into executable tests that fail before implementation.
   - Later stages implement the feature until those tests pass.
6. **Business tests stay green** — once a scenario has an executable business test, that test must keep passing after every later plan, for every feature. See [Business tests](#business-tests).

## Business tests

The executable tests generated from scenarios are the project's business tests. Together with the feature folders they form a living business specification: the documentation says what the product must do, and the business tests prove that it still does.

- After every plan, all business tests of all features pass, not only the tests of the milestone being implemented.
- A business test is changed or removed only after its scenario is deliberately changed or removed in the feature spec, and that spec change is approved.
- Agents never change, skip, weaken or delete a business test to make a plan pass. If implementation conflicts with an approved scenario, the agent escalates to the architect, who either adjusts the plan so the scenario still holds or proposes a scenario change for approval.
- Each business test is traceable to its scenario ID, so a failing test points back to the behavior it protects.

## Runtime state

Approvals, plan links and progress are runtime state, stored in `.forge/features/<slug>.json` inside the project, not in the repository. The committed spec folder under `docs/features/<slug>/` holds only documentation; the JSON file outside the repository captures which scenarios are approved, which milestone is being implemented, and which plan stages cover it. This file is not yet produced; it will be created starting with milestone M2, which introduces approvals committed to runtime state.

## See also

The specification of this feature-spec workflow itself is documented in [`docs/features/feature-specs/`](feature-specs/README.md).
