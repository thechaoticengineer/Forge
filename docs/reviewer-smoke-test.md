# Reviewer smoke test

1. In the Forge panel, cycle the provider controls until they explicitly show `implementer: codex` and `reviewer: codex`. Do not leave the reviewer as `reviewer: auto (other provider)`.

   **Expected result:** Both controls explicitly show Codex, with the reviewer in configured mode rather than automatic other-provider mode.

2. Enter a small test goal, generate the plan, and approve it. Before implementation starts, inspect every pending stage card.

   **Expected result:** Each stage shows an agreed capability tier of `basic`, `standard`, or `strong` and says `model selected at implementation start`. No concrete model is expected yet.

3. Start implementation and inspect each running or completed stage card.

   **Expected result:** The card shows the selected and/or execution identity as `codex/<actual-model-id>`. A successful invocation is marked `execution_verified`.

4. After the review gate runs, inspect the review outcome and the completed reviewer record in Forge review history/state or `.forge/plan.json` if the panel omits provenance fields.

   **Expected result:** The independent-review outcome is shown, and the record has `role: reviewer`, `provider: codex`, the provider-reported concrete `model`, and `fresh_session: true`. The review is a fresh read-only session and does not resume the implementer session.
