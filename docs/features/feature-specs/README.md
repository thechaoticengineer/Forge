# Feature specs (self-specification)

**Status: M1 (format and discovery) is implemented.** This folder specifies the feature-spec workflow itself, using the very format it describes (see `docs/features/README.md`). M1 enables the engine to discover and validate feature folders and expose them via an API and panel list. Milestones M2–M5 (spec phase, feature to plans, pen.dev integration, panel viewer) are not yet implemented, and the existing goal, discussion, planning, execution and queue workflows are unchanged.

## Goal

Let a feature be specified in the repository, under `docs/features/<slug>/`, and turned by Forge into reviewed, approved scenarios and then into one plan per milestone, with each milestone's scenarios becoming executable tests.

## What we want to achieve

- **Business documentation as the source of truth.** Every feature is described in the repository in business terms: its goal, scope, behavior, design and acceptance scenarios. People and agents read the same description before any plan is made.
- **Design next to the spec.** UI mockups (pen.dev) and diagrams (Mermaid) live in the feature's `design/` folder, so the intended result is visible, not only described.
- **Scenarios become business tests.** Every approved scenario becomes an executable test traceable to its scenario ID, written before the implementation it verifies.
- **Business tests always pass.** After every plan, all business tests of all features pass. A business test changes or disappears only when its scenario is deliberately changed in the spec and that change is approved; agents never adjust business tests just to get a plan through.
- **Specs drive planning.** Forge plans work from an approved feature milestone rather than from a free-form goal, and reviews check the result against the feature's scenarios.
- **Simple editing first.** Specs are edited as plain files (for example in nvim); a panel viewer for reading, checking test results and approving comes later.

## Scope

- The `docs/features/<slug>/` folder format and its validation rules.
- Engine discovery of feature folders and a read-only API exposing them.
- A panel list of discovered features and a viewer for their content.
- A spec co-authoring flow and an architect spec review.
- Scenario and decision approvals.
- Conversion of an approved milestone into a Forge plan whose first stage writes failing tests from that milestone's scenarios, with later stages making them pass.
- Use of pen.dev for UI mockups as part of a feature's `design/` folder.

## Out of scope

- Changes to the existing goal, optional discussion, planning, staged execution or queue flow: that flow stays unchanged.
- Storing runtime state (approvals, plan links, progress) in the repository. It lives in `.forge/features/<slug>.json`, outside version control.
- Design tools other than pen.dev beyond the documented fallback (OpenPencil).

## Behavior

The intended flow, once implemented, is:

1. **Draft** — a feature spec folder is written under `docs/features/<slug>/`.
2. **Architect spec review** — the architect reviews the draft's goal, scope and scenarios for consistency and feasibility.
3. **Scenario approval** — the acceptance scenarios are reviewed and approved.
4. **Milestones** — the feature is split into successive milestones (M1, M2, ...), each covering a subset of the approved scenarios.
5. **One plan per milestone** — for each milestone, Forge creates one plan. The plan's first stage turns that milestone's scenarios into executable tests that fail before implementation; later stages implement the feature until those tests pass.
6. **Business tests stay green** — every later plan must keep all existing business tests passing; changing or removing one requires an approved change to its scenario first.

See `docs/features/README.md` for the full folder format and flow description, and `milestones.md` in this folder for how this specific feature is broken into milestones.

## Risks

- **pen.dev cost (FS-RISK-PEN-COST).** pen.dev requires an account and may become a paid product. The documented fallback is OpenPencil, an MIT-licensed tool that reads `.pen` files, so mockups would remain usable even if pen.dev access is lost.
- **pen.dev MCP is desktop-only (FS-RISK-PEN-MCP).** pen.dev's MCP server only connects to the running desktop application. It cannot be used headlessly by agents; the planned headless path instead drives `pen interactive` through the shell using the CLI's bundled skill.
- **QML cannot render Mermaid (FS-RISK-MERMAID-QML).** The panel is built with QML, which has no Mermaid renderer. A later implementation must choose between pre-rendering diagrams to SVG with `mmdc` (mermaid-cli) and showing the Mermaid source as text.
- **Codex MCP parity is unknown (FS-RISK-CODEX-MCP).** Whether Codex-based agents have MCP capabilities equivalent to Claude's is not established and must not be assumed when M4's headless pen.dev integration is designed.

## Open questions

- Which Mermaid rendering option (pre-rendered SVG via `mmdc`, or showing source) will the panel viewer use?
- What is the exact schema of `.forge/features/<slug>.json`? (M2 fixes its content: status, architect reviews, and approvals bound to a commit and content hash — see D10; field names are decided during implementation.)
- How do scenario IDs (S1, S2, ...) map to generated test names?
- ~~How are spec co-authoring writes restricted to a single feature folder?~~ Decided in D8.
- ~~What does "architect spec review" require beyond the existing plan-review mechanics?~~ Decided in D9.
