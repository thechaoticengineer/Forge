# Feature specs

**Status: M1 (format and discovery), M2 (spec phase), M3 (feature to plans) and M4 (pen.dev integration) are implemented; M5 (panel viewer) is planned.** The Forge engine discovers `docs/features/<slug>/` folders, validates required files and scenario structure, and exposes them via a read-only `GET /api/features` endpoint. It also lets a feature be created from the template, co-authored by a read-only agent whose proposed writes it validates and applies, reviewed by the persistent architect, and approved (spec, then scenarios) with each approval bound to a Git commit and a content hash. Agents edit pen.dev `.pen` files headlessly via the shell with the bundled skill, and the engine exports PNGs after each editing turn before review and commit. M3 plans an approved milestone as a Forge plan with compact feature context and registers business tests that every plan keeps passing. The existing goal, discussion, planning, execution and queue workflows remain unchanged.

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

Status: implemented
Covers: S1, S2, S3
```

A milestone whose scenarios are not written yet uses `Covers: none yet`.

Each milestone may carry one `Status:` line, either `Status: implemented` or `Status: planned`. A milestone without one is planned. The engine reports any other value, a second `Status:` line in the same milestone, or a `Status:` line before the first milestone as a validation error. The plan that implements a milestone sets its line to `implemented`. From these lines `GET /api/features` derives each feature's `progress`: `implemented` when every milestone is implemented, `in progress` when some are, and `planned` otherwise. It also returns a `milestones` list of `{id, title, status}`. Progress is independent of the spec approval status: marking a milestone implemented edits the folder, which moves `spec_status` back to `draft` as any other change does. The panel's Features tab hides implemented features until you choose to show them, and labels partly implemented ones `N/M implemented`.

Empty templates of these files are available in `docs/features/_template/` to copy as a starting point.

## Intended flow

The feature specification flow proceeds in order:

1. **Draft** — the spec folder is created (from the panel's "New feature" action, which copies `docs/features/_template/`) and written with README, design sketches, scenarios and initial decisions, edited by hand or through the panel's read-only co-authoring chat.
2. **Architect spec review** — requested from the panel, the architect reviews the proposed goal, scope, scenarios and decisions for consistency and feasibility and returns a structured verdict.
3. **Scenario approval** — once the architect has approved the spec, the panel's approval actions record the spec approval and then the scenario approval, each bound to a Git commit and a content hash of the folder.
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

### Business test registry

A milestone registers its business test files on one `Business tests:` line in `milestones.md`, normally set by the final stage of the plan that implements it. Discovery parses these lines read-only, and reviewers of every plan are given the registered files of all features and must run them.

```
Business tests: src/a.rs (S1-S6, S9) and tests/b.test.mjs (S7-S8)
Business tests: `src/a.rs`, `tests/b.test.mjs` and `qml/c.qml`
```

Grammar:

- At most one `Business tests:` line per milestone, after the milestone's `## ` heading.
- The value is a list of entries separated by commas and by the standalone word `and`. Separators inside parentheses do not split, so a note may contain commas or `and`.
- An entry is a repository-relative path, optionally wrapped in one pair of backticks, optionally followed by one parenthesized note (free text, ignored, for example the scenario IDs it covers). The path has no whitespace, is not absolute and has no `..` component. Paths are resolved against the project root, not the feature folder.
- Both forms above are accepted: paths with notes joined by `and`, and backticked paths separated by commas and `and`.

A feature is invalid, with a reason naming the entry, when a milestone has a second `Business tests:` line (`repeated Business tests line in <M>`) or one before the first milestone, when an entry is not a path (`malformed Business tests entry in <M>: <entry>`), or when a path is not an existing regular file (`business test file not found in <M>: <path>`). `GET /api/features` reports each milestone's registered files as `business_tests`.

## Planning a milestone

A milestone of a feature whose status is `scenarios approved` is planned with the **Plan milestone** action of the panel's Features tab, which calls `POST /api/features/plan` (see [the API in the top-level README](../../README.md#feature-specs)). The action is offered on planned milestones that cover scenarios and is enabled only for an approved feature. The engine refuses, with a reason shown inline and without starting an agent or changing any state, when the feature is invalid or not approved for its current content, the milestone is unknown, implemented or `Covers: none yet`, it covers a scenario that is not approved, or the engine is busy with a plan or the queue. Otherwise it builds the goal itself and starts planning like any goal plan; on success the panel switches to the Plan tab. Execution is not started and nothing is added to the queue.

- **Compact feature reference.** The planner, the architect and the reviewers of a milestone plan receive the feature slug and folder, the milestone ID and title, the covered scenario IDs and the rules below, never the contents of the feature's files, so their prompts do not grow with the spec. They read the files in the repository when they need them. The plan records `feature: {slug, milestone, title, scenario_ids}`.
- **First stage.** It turns every covered scenario into an executable test named after its scenario ID that fails before implementation, and names every covered scenario ID in its instructions or acceptance. A plan whose first stage misses an ID goes back to the planner through the shared response-correction budget.
- **Final stage.** It sets the milestone's `Status: implemented` and its `Business tests:` line to the test files that cover its scenarios, so that both edits sit inside the reviewed commit range.
- **Plan review.** Plan review adds one criterion per covered scenario ID, `an executable test traceable to <ID> exists and passes`, so an unverified scenario prevents approval. Every plan review, milestone or not, also gets the registered business test files and the list of those that the diff modified or deleted (see [Business tests](#business-tests)); reviewers reject such a change unless it only adds tests or implements an approved scenario change. The engine does not block these diffs. Implementers that find a conflict with an approved scenario escalate it to the architect as an architectural context gap.
- **Completion.** After the plan review approves, the engine makes no repository change: it only records the plan as `completed` with its commit range in the feature's runtime state.
- **Other plans.** Goal, discussion, refactor and queue plans get no feature context and write no feature state; only the business test rules apply to them.

## Runtime state

Approvals, plan links and progress are runtime state, stored in `.forge/features/<slug>.json` inside the project, not in the repository. The committed spec folder under `docs/features/<slug>/` holds only documentation; the JSON file outside the repository captures which scenarios are approved, which milestone is being implemented, and which plan stages cover it.

This file is produced starting with milestone M2. A missing file means the defaults `{"version":1, "slug", "reviews":[], "approvals":[], "chat":[], "architect_session":null, "plans":[]}`; a file written before M3 has no `plans` and is read as `[]`. `reviews` holds append-only architect spec review records (verdict, issues, questions, content hash, provider/model/session); `approvals` holds append-only spec and scenario approval records, each bound to a Git commit and a content hash of the folder; `chat` holds the co-authoring transcript; `architect_session` remembers the feature's own architect session when no plan session was resumed; `plans` holds one link per planned milestone (milestone, goal, started time, status `planning`, `planned`, `failed` or `completed`, and once completed the `commit_range` `{base, head}` of the reviewed work). A feature's status (draft, spec approved, or scenarios approved) is derived from this state and the folder's current content hash on every read, never stored: changing any file in the folder after an approval returns the status to draft while the earlier reviews and approvals remain as history. See [the feature-spec API in the top-level README](../../README.md#feature-specs) for the exact schema and the endpoints that read and write it.

## See also

The specification of this feature-spec workflow itself is documented in [`docs/features/feature-specs/`](feature-specs/README.md).
