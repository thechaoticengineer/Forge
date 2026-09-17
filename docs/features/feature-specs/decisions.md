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
