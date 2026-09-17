# Decisions

## D1: Specs live in the repository as documentation

Decision: Feature specifications are committed under `docs/features/<slug>/` as plain documentation, not stored in an external system.

Reason: Keeps the spec reviewable and versioned alongside the code it describes, using the same review tools (diffs, PRs) as everything else in the repository.

## D2: pen.dev for UI mockups, Mermaid for diagrams

Decision: A feature's `design/` folder uses pen.dev (`.pen` files with an exported PNG) for UI mockups and Mermaid for diagrams.

Reason: pen.dev gives structured, agent-editable mockups with a CLI path to headless use; Mermaid diagrams are plain text, diffable, and widely supported in documentation tooling.

## D3: Markdown scenarios become executable tests in the first stage of each plan

Decision: A milestone's `scenarios.md` entries are turned into executable tests as the first stage of that milestone's plan, before any implementation stage.

Reason: Makes the acceptance criteria concrete and verifiable up front, and gives every later implementation stage a failing test to work against.

## D4: One feature may span multiple plans

Decision: A feature is split into milestones, and each milestone becomes its own Forge plan, rather than one plan covering the whole feature.

Reason: Keeps individual plans reviewable in scope and size, matching how Forge already prefers small staged plans over large ones.

## D5: The existing goal/discussion/queue flow stays unchanged

Decision: The feature-spec workflow is additive; the existing goal, optional discussion, planning, staged execution and queue flow is not modified by it.

Reason: Feature specs are meant to feed into that flow (by producing plans), not replace or fork it, minimizing risk to already-working behavior.

## D6: Runtime state lives outside the repository

Decision: Approvals, plan links and progress for a feature are stored in `.forge/features/<slug>.json`, not committed to `docs/features/<slug>/`.

Reason: Keeps the committed spec folder as pure documentation, and matches how other runtime/engine state is already kept out of the repository under `.forge/`.

## D7: Business tests always pass unless their scenario is changed

Decision: After every plan, all business tests generated from approved scenarios, across all features, must pass. A business test may be changed or removed only after its scenario is deliberately changed or removed in the feature spec and that change is approved. Agents escalate conflicts with approved scenarios to the architect instead of adjusting the tests.

Reason: Business tests are the executable form of the business documentation. If they could drift or be edited to fit an implementation, the documentation would stop describing what the product actually does.

## D8: The engine, not the sandbox, restricts co-authoring writes

Decision: The co-authoring agent runs read-only and returns proposed file contents as structured JSON. The engine validates every path against `docs/features/<slug>/` and writes the files itself.

Reason: Path validation in the engine works the same for every provider and follows the existing pattern where agents return JSON and the engine alone applies effects.

## D9: Architect spec review is a structured verdict of the persistent architect

Decision: Spec review uses the plan's persistent architect session and returns `approved`, `issues` and `questions`, validated through the shared response-correction policy. It reuses existing verdict mechanics but has no stage, snapshot or project-check requirements.

Reason: The architect already holds the project's architectural context; a spec is documentation, so running builds and tests adds nothing.

## D10: Approvals are bound to content

Decision: `.forge/features/<slug>.json` records, for each approval, the commit and a content hash of `docs/features/<slug>/`. Any later change to the folder returns the feature to `draft`; earlier approvals remain as history.

Reason: An approval must always refer to exactly the content that was reviewed.

## D11: Agents use pen.dev through the shell, not MCP

Decision: Editing agents drive `pen interactive --in/--out` through the shell, using `execute` and `save()` calls and the CLI's bundled skill as documentation. The desktop app and its MCP server are only for manual design work.

Reason: pen.dev's MCP server needs the running desktop app, and MCP support differs between Claude and Codex. The shell path works headlessly for every provider and was verified on 2026-09-17 (creating frames and exporting PNGs without the app).

## D12: The engine owns PNG export

Decision: The engine, not the agent, exports PNGs for changed `.pen` files after each editing turn, one per top-level frame, and treats export failures like failing checks.

Reason: Exported images must always match the committed design, and reviewers in read-only sandboxes need the PNGs because they cannot run `pen` themselves.
