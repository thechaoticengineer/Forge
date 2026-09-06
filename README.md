# Forge

A minimal wrapper around AI coding agents (Claude Code, Codex CLI) for
building software — including Forge itself — with almost no ceremony.

Rust engine + Quickshell (Omarchy) panel.

## The loop

1. Point Forge at a git repository and describe a goal.
2. A planner agent (Claude Code or Codex) writes a staged plan to
   `.forge/plan.json` — each stage has instructions, acceptance criteria,
   and a proposed commit message.
3. You mark the plan OK in the panel.
4. Forge runs each stage automatically:
   - the **implementer** (one tool) implements the stage,
   - an independent **reviewer** (the other tool, always a fresh session)
     reviews the uncommitted diff and writes `.forge/verdict.json`,
   - rejections loop back to the implementer with the reviewer's issues,
     up to a bounded number of fix rounds,
   - an approved stage is committed with the proposed message.
5. After the last stage, Forge pushes to `origin`.

The reviewer's verdict has the shape
`{"approved": bool, "summary": str, "issues": [str, ...]}`.
The summary describes what the reviewer inspected and found, even on
approval; issues are specific, actionable feedback for the implementer.
Every review round is recorded in the stage's `reviews` array in
`.forge/plan.json`, with `round` (starting at 1), `approved`, `summary`,
`issues`, and `unix` (a Unix timestamp in seconds). Earlier feedback stays
available through fix rounds and approval.

If the reviewer still rejects after the fix rounds, the stage is marked
blocked and Forge stops for you. Full history of every agent session,
verdict, and git action is visible in the panel and kept in
`.forge/history.jsonl`. Review approval events include the reviewer's
summary, limited to 300 characters; the stage's `reviews` array keeps
the full summary.

## Queue

Add multiple goals from the panel's queue section, reorder them, then
start the queue. Forge processes one goal at a time through the same
plan → approve → run loop above.

The `queue_auto_approve` setting is off by default: Forge pauses at each
plan for your usual approval. Enable it to approve each plan automatically
and run the queue unattended. A blocked or failed item stops the queue
for human intervention; remaining goals stay queued.

Queue state lives in `.forge/queue.json` in the project. The panel shows
each item's status; the bar widget shows the pending count and a queue tooltip.

The JSON API accepts POST requests to `/api/queue/add` with `{"goal":"…"}`,
`/api/queue/remove` with `{"id":1}`, and `/api/queue/move` with
`{"id":1,"dir":"up"}` (or `"down"`). `/api/queue/clear` removes pending
goals; `/api/queue/start` starts processing. `GET /api/state` includes
the queue and whether it is active.

## Run

```
cargo run [/path/to/project]
```

The engine serves a JSON API on `http://127.0.0.1:8734` for the panel
(also usable with `curl`). Requires `claude` and/or `codex` CLIs on
PATH, logged in.

The Omarchy plugin (`manifest.json`, `quickshell/`) provides the bar
widget and the Forge panel: pick planner/implementer/reviewer, set the
project path, type the goal, create the plan, approve, start.
Each reviewed stage has a review chip showing approval or the latest issue
count, plus the number of rounds when there is more than one. Expand a
stage to see its full per-round review history: approved or rejected,
the reviewer's summary, and issues, including reviews after fix rounds.
