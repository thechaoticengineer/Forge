# Scenarios

**Status: M1 scenarios (S1-S9), M2 scenarios (S10-S19) and M4 scenarios (S20-S26) have executable business tests and are covered by the implementation. M3 scenarios (S27-S35) are planned.**

Acceptance scenarios below use stable IDs (S1, S2, ...), assigned once and never renumbered or reused. This file currently covers milestones M1 (format and discovery), M2 (spec phase), M3 (feature to plans) and M4 (pen.dev integration); scenarios for M5 are to be written before that milestone is planned (see `milestones.md`). The M1 scenarios are verified by business tests in src/feature_spec_tests.rs (S1-S6, S9) and tests/panel_features.test.mjs (S7-S8). The M2 scenarios are verified by business tests in src/feature_spec_m2_tests.rs (S10-S19) and tests/panel_feature_spec.test.mjs (panel parts of S10-S18). The M4 scenarios are verified by business tests in src/pen_dev_tests.rs (S20-S26).

## S1: A valid feature folder is discovered

- Given: `docs/features/<slug>/` exists with all required files (`README.md`, `scenarios.md`, `decisions.md`, `milestones.md`)
- When: the engine scans `docs/features/` for feature folders
- Then: the folder is discovered and reported as a valid feature

## S2: Folders starting with an underscore are ignored

- Given: `docs/features/_template/` (or any other folder whose name starts with `_`) exists alongside valid feature folders
- When: the engine scans `docs/features/` for feature folders
- Then: the underscore-prefixed folder is not reported as a feature

## S3: A folder missing a required file is reported invalid

- Given: a feature folder under `docs/features/<slug>/` is missing one of its required files
- When: the engine scans `docs/features/` for feature folders
- Then: the folder is reported as invalid, with a reason that identifies the missing file

## S4: A scenarios.md with duplicate or malformed IDs is reported invalid

- Given: a feature's `scenarios.md` contains two scenarios sharing the same ID, or an ID that does not follow the stable `S<number>` form
- When: the engine validates that feature's folder
- Then: the feature is reported as invalid, with a reason that identifies the offending scenario ID

## S5: A milestones.md referencing an unknown scenario ID is reported invalid

- Given: a feature's `milestones.md` lists a scenario ID that is not defined in that feature's `scenarios.md`
- When: the engine validates that feature's folder
- Then: the feature is reported as invalid, with a reason that identifies the unknown scenario ID

## S6: The read-only API lists features without modifying files

- Given: `docs/features/` contains a mix of valid and invalid feature folders
- When: a client requests the list of features from the engine's read-only API
- Then: the response identifies each feature by slug, title (the first `# ` heading of its `README.md`, or the slug when there is none) and validation status, and no file under `docs/features/` is created, modified or removed as a result

## S7: The panel lists discovered features with their status

- Given: the engine has discovered one or more feature folders
- When: the user opens the panel's feature list
- Then: each discovered feature is shown with an indication of its validation status

## S8: Opening a feature folder in nvim from the panel

- Given: the panel's feature list is showing a discovered feature
- When: the user chooses the "open in nvim" action for that feature
- Then: a terminal window opens nvim on that feature's folder (`nvim .` in `docs/features/<slug>/`), showing the folder's files for browsing

## S9: The existing goal/discussion/queue flow is unaffected without feature specs

- Given: `docs/features/` is absent from the repository, or present but empty
- When: the user drives the existing goal, optional discussion, planning, staged execution and queue flow
- Then: that flow behaves exactly as it does today, with no change caused by the absence or emptiness of `docs/features/`

## S10: A new feature is created from the template

- Given: no folder `docs/features/<slug>/` exists for a valid new slug
- When: the user creates a feature with that slug and a title from the panel
- Then: the folder is created from `docs/features/_template/` with the title as the README heading, the feature appears in the list with status `draft`, and nothing is committed yet

## S11: The co-authoring agent edits only its feature folder

- Given: a draft feature is open for co-authoring
- When: the user sends a message asking for changes to the spec
- Then: the agent replies and its proposed file changes are applied only to files inside `docs/features/<slug>/`

## S12: Co-authoring writes outside the feature folder are rejected

- Given: the co-authoring agent proposes a change to a path outside `docs/features/<slug>/` (including `..` or symlink escapes)
- When: the engine validates the agent's response
- Then: no file is written, and the rejection is returned to the agent through the shared response-correction budget

## S13: The architect reviews the spec

- Given: a draft feature that passes M1 validation
- When: the user requests an architect spec review
- Then: the persistent architect returns a structured verdict (approved, issues, questions) about consistency, feasibility and conflicts with the existing architecture, and the verdict is stored in the feature's runtime state and shown in the panel

## S14: An invalid feature cannot be sent to spec review

- Given: a feature that fails M1 validation
- When: the user requests an architect spec review
- Then: the request is refused with the validation reasons, and no agent is started

## S15: Approving the spec commits the feature folder

- Given: a feature whose latest architect spec review approved the current content
- When: the user approves the spec
- Then: the engine commits only `docs/features/<slug>/` as `docs(features): approve <slug> spec`, and records the commit and a content hash of the folder in `.forge/features/<slug>.json`

## S16: The spec cannot be approved without an approving review of the current content

- Given: a feature with no architect review, a rejecting review, or files changed since the approving review
- When: the user approves the spec
- Then: approval is refused with the reason, and nothing is committed

## S17: Approving scenarios records their IDs

- Given: a feature whose spec is approved and unchanged since approval
- When: the user approves the scenarios
- Then: the approved scenario IDs, commit and content hash are recorded in `.forge/features/<slug>.json`, and the feature status becomes `scenarios approved`

## S18: Changing an approved spec reopens it

- Given: a feature with an approved spec or approved scenarios
- When: any file in `docs/features/<slug>/` changes afterwards (by co-authoring or manual editing)
- Then: the feature status returns to `draft`, previous approvals are kept as history, and a new architect review and approval are required

## S19: Runtime state is outside the repository

- Given: any feature with reviews or approvals
- When: its runtime state is saved
- Then: it is written to `.forge/features/<slug>.json`, never to `docs/features/`, and the file stays valid JSON after an interrupted write

## S20: Editing agents get pen.dev instructions when a stage involves designs

- Given: a stage whose instructions or acceptance criteria reference a `.pen` file or a feature's `design/` folder
- When: the engine starts the implementer or fixer for that stage, with either Claude or Codex
- Then: the prompt explains how to edit `.pen` files headlessly with `pen interactive` through the shell and points to the pen.dev CLI's bundled skill file

## S21: Stages without designs are unchanged

- Given: a stage that does not reference `.pen` files or a `design/` folder, and no `.pen` file changed during the stage
- When: the stage runs
- Then: no pen.dev instructions are added to prompts and `pen` is never invoked

## S22: A PNG is exported for every changed .pen file

- Given: an implementer or fixer turn created or changed a `.pen` file
- When: the turn finishes and before the stage snapshot is reviewed or committed
- Then: the engine exports the design headlessly, the PNG files next to the `.pen` file match its current content, and they are part of the same stage commit

## S23: Export file names follow the design's top-level frames

- Given: a changed `design/<name>.pen` file
- When: the engine exports it
- Then: a design with one top-level frame produces `<name>.png`; a design with several produces `<name>.<frame-slug>.png` per frame (frame name lowercased, non-alphanumerics replaced by `-`); PNGs from an earlier export of that file that no longer match a frame are removed

## S24: A missing or unauthenticated pen CLI blocks with an actionable message

- Given: a `.pen` file changed, but `pen` is not installed or `pen status` reports no active session
- When: the engine tries to export it
- Then: the stage does not commit, and the run blocks with a message naming the problem and the fix (install `@pen.dev/cli`, or run `pen login`)

## S25: A failed export goes back to the agent

- Given: `pen` is available, but exporting a changed `.pen` file fails (for example the file cannot be opened)
- When: the engine exports it
- Then: the export error is returned to the same stage's implementer or fixer to repair, like a failing check, and the stage does not commit until the export succeeds

## S26: Reviewers review designs through the exported PNGs

- Given: a stage snapshot containing `.pen` files and their exported PNGs
- When: stage or plan reviewers run in their read-only sandbox
- Then: their prompts point them to the PNGs and the `.pen` JSON, and they are not required to run `pen`

## S27: A milestone of an approved feature is planned from the panel

- Given: a valid feature whose status is `scenarios approved` for its current content, and a planned milestone whose `Covers:` scenario IDs are all among the approved scenario IDs
- When: the user chooses "Plan milestone" for that milestone in the panel's Features tab (`POST /api/features/plan` with the slug and milestone ID)
- Then: Forge starts planning with a goal built by the engine from the feature and milestone, the resulting plan records the feature slug, milestone ID and covered scenario IDs, and `.forge/features/<slug>.json` records a link to that plan

## S28: Milestone planning is refused unless the feature is ready

- Given: a feature that is invalid or not `scenarios approved` for its current content, or a milestone that is unknown, already implemented, has `Covers: none yet` or covers a scenario ID that is not approved, or an engine that is busy with another plan or the queue
- When: the user requests planning of that milestone
- Then: the request is refused with the reason, no agent is started, and neither the current plan nor the feature's runtime state changes

## S29: The planner gets the feature context and plans failing tests first

- Given: a milestone plan is being generated
- When: the planner returns its plan
- Then: the planner's prompt named the feature folder, the milestone and its covered scenario IDs; the plan's first stage turns every covered scenario into an executable test named after its scenario ID that fails before implementation; and a plan whose first stage does not name every covered scenario ID is returned to the planner through the shared response-correction budget

## S30: The architect gets a compact feature context

- Given: a milestone plan
- When: the engine builds the architect's context for any architect turn of that plan
- Then: the context includes the feature slug, folder path, milestone ID and title and the covered scenario IDs, but not the contents of the feature's files

## S31: Plan review requires every covered scenario to be tested and passing

- Given: the stages of a milestone plan are committed and plan review runs
- When: the plan reviewers build their verdicts
- Then: the review criteria contain one item per covered scenario ID (an executable test traceable to that ID exists and passes), stage and plan reviewers are pointed to the feature folder, and the plan cannot be approved while any of those items fails

## S32: Every plan keeps all business tests of all features passing

- Given: one or more features whose milestones list business test files on `Business tests:` lines
- When: any plan (a milestone plan or any other plan) is reviewed
- Then: the stage and plan reviewers are given the list of registered business test files of all features and must run them, a failing business test prevents approval, and a `Business tests:` line naming a file that does not exist makes the feature invalid with a reason naming that file

## S33: Changes to business test files are surfaced to reviewers

- Given: a stage or plan-fix diff that modifies or deletes a registered business test file
- When: the engine prepares the reviewers' prompts
- Then: the engine lists the affected business test files in the prompts, and the reviewers reject the change unless it only adds tests or implements a scenario change approved in the feature spec; an implementer that finds an approved scenario in conflict with the plan escalates it to the architect instead of changing the test

## S34: A completed milestone plan marks its milestone implemented

- Given: a milestone plan whose stages are all committed and whose plan review approved it
- When: the plan completes
- Then: the milestone's `Status:` line in `milestones.md` reads `implemented` and its `Business tests:` line names the test files covering its scenarios, and `.forge/features/<slug>.json` records the plan as completed with its commit range

## S35: Plans not started from a feature get no feature context

- Given: a plan started from a goal, a discussion, a refactor request or the queue
- When: it is planned, executed and reviewed
- Then: no feature context is added to its planner, architect or reviewer prompts and no feature runtime state is written, while the business-test rules of S32 and S33 still apply
