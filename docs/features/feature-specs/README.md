# Feature specs (self-specification)

**Status: planned; the engine does not implement it yet.** This folder specifies the feature-spec workflow itself, using the very format it describes (see `docs/features/README.md`). Nothing here changes how Forge currently discovers, validates or acts on anything; today there is no engine support for `docs/features/` at all.

## Goal

Let a feature be specified in the repository, under `docs/features/<slug>/`, and turned by Forge into reviewed, approved scenarios and then into one plan per milestone, with each milestone's scenarios becoming executable tests.

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

See `docs/features/README.md` for the full folder format and flow description, and `milestones.md` in this folder for how this specific feature is broken into milestones.

## Risks

- **pen.dev cost (FS-RISK-PEN-COST).** pen.dev requires an account and may become a paid product. The documented fallback is OpenPencil, an MIT-licensed tool that reads `.pen` files, so mockups would remain usable even if pen.dev access is lost.
- **pen.dev MCP is desktop-only (FS-RISK-PEN-MCP).** pen.dev's MCP server only connects to the running desktop application. It cannot be used headlessly by agents; the planned headless path instead drives `pen interactive` through the shell using the CLI's bundled skill.
- **QML cannot render Mermaid (FS-RISK-MERMAID-QML).** The panel is built with QML, which has no Mermaid renderer. A later implementation must choose between pre-rendering diagrams to SVG with `mmdc` (mermaid-cli) and showing the Mermaid source as text.
- **Codex MCP parity is unknown (FS-RISK-CODEX-MCP).** Whether Codex-based agents have MCP capabilities equivalent to Claude's is not established and must not be assumed when M4's headless pen.dev integration is designed.

## Open questions

- Which Mermaid rendering option (pre-rendered SVG via `mmdc`, or showing source) will the panel viewer use?
- What is the exact schema of `.forge/features/<slug>.json`?
- How do scenario IDs (S1, S2, ...) map to generated test names?
- How are spec co-authoring writes restricted to a single feature folder?
- What does "architect spec review" require beyond the existing plan-review mechanics?
