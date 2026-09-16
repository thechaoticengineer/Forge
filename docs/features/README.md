# Feature specs

**Status: this workflow is planned and not yet implemented.** The Forge engine does not currently discover, validate, read from or act on the `docs/features/` folder. Nothing in this folder changes how goals, discussions, plans or the queue work today. This folder holds documentation only.

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

**README.md** contains the feature's goal, scope and out-of-scope boundaries, intended behavior, and any open questions. It is the entry point for reviewers.

**design/** holds UI mockups as `.pen` files (pen.dev format), each with an exported PNG next to it using the same base name (e.g. `panel-list.pen` and `panel-list.png`), and diagrams as Mermaid files (`.mmd`) or fenced ` ```mermaid ` blocks in Markdown.

**scenarios.md** lists acceptance scenarios in Given/When/Then format, each with a stable ID (`S1`, `S2`, ...). IDs are never renumbered or reused once assigned, so that milestones, plans and executable tests can reliably refer to them. Example:

```
## S1 User opens the feature

Given: the panel is open and focused
When: the user presses Ctrl+O
Then: the feature dialog appears with focus on the first input
```

**decisions.md** records agreed decisions (what was decided and why), in any format suitable for your project.

**milestones.md** splits the feature into successive plans, each labelled M1, M2, etc., and lists the scenario IDs that each milestone covers. For example:

```
## M1 Discover and validate

Covers: S1, S2, S3
```

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

## Runtime state

Approvals, plan links and progress are runtime state, stored in `.forge/features/<slug>.json` inside the project, not in the repository. The committed spec folder under `docs/features/<slug>/` holds only documentation; the JSON file outside the repository captures which scenarios are approved, which milestone is being implemented, and which plan stages cover it. This file is not produced by the current Forge engine.

## See also

The specification of this feature-spec workflow itself is documented in `docs/features/feature-specs/` (to be added).
