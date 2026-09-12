# Reviewer smoke test

1. Start in a clean worktree (`git status --short` produces no output). In the Forge panel, select `implementer: codex` and explicitly select `reviewer: codex`, not `reviewer: auto (other provider)`.

   **Expected result:** Both controls explicitly show Codex, with the reviewer in configured mode rather than automatic other-provider mode.

2. With automatic approval disabled, enter a small test goal and generate a new plan. Inspect every pending stage card before approving it.

   **Expected result:** Each stage shows an agreed capability tier of `basic`, `standard`, or `strong` and says `model selected at implementation start`. No concrete model is expected yet.

3. Approve the plan, start implementation, and inspect each running or completed stage card.

   **Expected result:** The card shows the selected and/or execution identity as `codex/<actual-model-id>`. A successful invocation is marked `execution_verified`.

4. Allow reviewer review to run at its configured cadence: after a stage for `per_stage`, or after all stages commit for the default `per_plan`. Inspect the completed reviewer verdict in the full architecture history (`GET /api/architecture/history?plan_id=<plan-id>`, following `next_cursor` as needed). Select the record for the relevant scope and latest review round; state previews may omit provenance fields.

   **Expected result:** The verdict has `role: reviewer`, `provider: codex`, a provider-reported concrete `model`, and `fresh_session: true`. Review runs in a fresh read-only session, including after Codex implementation. Its approval or requested changes are shown separately; a stage marked `deferred` is still awaiting plan review.
