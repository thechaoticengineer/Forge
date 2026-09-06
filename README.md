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
   - an independent, adversarial **reviewer** (the other tool, always a fresh session)
     reviews the uncommitted diff and writes `.forge/verdict.json`,
   - rejections loop back to the implementer with the reviewer's issues,
     up to a bounded number of fix rounds,
   - approval with improvement notes can trigger one **polish round** and
     another review before committing,
   - an approved stage is committed with the proposed message.
5. After the last stage, Forge pushes to `origin`.

The reviewer starts by assuming there is a defect and actively looking for
it in the diff and surrounding code. Before approving, it must verify each
acceptance criterion individually and independently run the project's
available build and tests. It must record the evidence and results, including
exact commands; if a build or test is unavailable, it must explain how it
established that. A failed or unrun available check, or an acceptance
criterion it could not verify, requires rejection.

The reviewer's verdict has the shape
`{"approved": bool, "summary": str, "issues": [str, ...], "notes": [str, ...], "checks": [str, ...]}`.
The summary describes what the reviewer inspected and found, even on
approval. `issues` are specific, actionable blocking defects; `notes` are
concrete non-blocking improvements; `checks` list the commands and inspections
actually performed and their results, including each acceptance criterion.
The reviewer must look for improvements, but may leave notes empty if it
finds none. Forge treats an approving verdict with blocking issues as a
rejection.

The `max_fix_rounds` setting (default `3`) limits extra fixer/review rounds
after the initial implementation and review. Alongside it,
`apply_review_notes` (default `true`) enables one polish round per stage:
when the reviewer approves with notes and budget remains, Forge sends those
notes to the fixer to apply, then re-reviews. Polish uses the same
`max_fix_rounds` budget as rejection fixes. An approval can therefore still
lead to an improvement round before the stage's commit. Further notes do
not trigger another polish round; if polishing is disabled or the budget
is exhausted, approval with notes proceeds to commit. A rejection after
polishing follows the usual fix loop within the remaining budget.

Every review round is recorded in the stage's `reviews` array in
`.forge/plan.json`, with `round` (starting at 1), `approved`, `summary`,
`issues`, `notes`, `checks`, and `unix` (a Unix timestamp in seconds).
Earlier feedback stays available through fix rounds, polish, and approval.

If the reviewer still rejects after the fix rounds, the stage is marked
blocked and Forge stops for you. Full history of every agent session,
verdict, and git action is visible in the panel and kept in
`.forge/history.jsonl`. Final review approval events include the reviewer's
summary, limited to 300 characters; the stage's `reviews` array keeps
the full summary.

### Refactor plans

Click **Refactor plan** next to **Create plan** in the panel to have the
planner read the code and propose a staged refactoring plan that preserves
observable behavior. Use the goal field as an optional focus hint, or leave
it empty for the planner to choose refactorings on its own. Edit the draft,
revise it with AI, approve it, and run it exactly like any other plan.

The JSON API accepts POST requests to `/api/plan` with
`{"mode":"refactor","goal":"optional focus"}`. Omit `goal` or leave it blank
to use "Refactor the codebase" as the plan goal. The `mode` field is optional;
omitting it keeps standard goal-based planning. Unknown modes are rejected
with HTTP 400.

### Editing the plan

After the planner writes a draft, use **Edit plan** in the panel to repair
it by hand: edit each stage's title, instructions, acceptance criteria,
and proposed commit message, or add, remove, and reorder stages. Committed
stages are locked. Click **Save** to keep your changes or **Cancel** to
discard them.

To ask AI for changes, type feedback into the field beside **Improve with
AI**, then click the button or press `Enter`. Forge re-runs the planner
against the current plan and your feedback, keeping the same overall goal.
A failed AI revision keeps the previous plan.

Saving manual edits or completing an AI revision produces a new draft that
needs your approval again before running. Both paths preserve committed
stages. You can also edit or revise an approved or completed plan while
Forge is idle and the queue is inactive.

The JSON API accepts POST requests to `/api/plan/edit` with
`{"plan":{"goal":"…","stages":[{"id":1,"title":"…","instructions":"…","acceptance":"…","commit":"…"}]}}`
and `/api/plan/revise` with `{"feedback":"…"}`. For manual edits, submit the
complete stage list, keeping committed stages at the start in their original
order; omit `id` for new stages to have Forge assign one. AI feedback must
not be blank. Both endpoints require an existing plan and reject requests
while the project is busy or its queue is active.

### Asking about the plan

Type a question about the current plan into the field beside **Plan Q&A**
in the panel, then click **Ask** or press `Enter`. The selected planner tool
answers without modifying the plan. Expand **Plan Q&A** to read the
conversation. Asking requires an existing plan, with Forge idle, the queue
inactive, and plan editing closed.

The JSON API accepts POST requests to `/api/plan/chat` with
`{"question":"..."}`. Questions must not be blank. The transcript is stored
in `.forge/chat.jsonl` and cleared when new plan generation starts (including
an AI revision) or the plan is reset.

## Queue

Add multiple goals from the panel's queue section, reorder them, then
start the queue. Forge processes one goal at a time through the same
plan → approve → run loop above.

The `queue_auto_approve` setting is off by default: Forge pauses at each
plan for your usual approval. Enable it to approve each plan automatically
and run the queue unattended. A blocked or failed item stops the queue
for human intervention; remaining goals stay queued.

Successfully completed goals are removed from the queue automatically.
Failed or blocked goals stay visible until you dismiss them with their
**×** remove button in the panel.

Queue state lives in `.forge/queue.json` in the project. The panel shows
each item's status; the bar widget shows the pending count and a queue tooltip.

The JSON API accepts POST requests to `/api/queue/add` with `{"goal":"…"}`,
`/api/queue/remove` with `{"id":1}`, and `/api/queue/move` with
`{"id":1,"dir":"up"}` (or `"down"`). Removal accepts queued, failed, or
blocked goals. `/api/queue/clear` removes pending
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
Each reviewed stage has a review chip showing approval (or
`approved · N notes` when notes remain) or the latest issue count, plus the
number of rounds when there is more than one. Expand a stage to see its full
per-round review history: approved or rejected, the reviewer's summary,
blocking issues, non-blocking notes, and checks under `verified:`, including
reviews after fix and polish rounds.

### Updating and troubleshooting

Run `./install.sh` to install the current working tree. The panel's **Update
Forge** button posts to `/api/self_update`, which runs the same script in a
transient `forge-update` systemd user unit so it survives the engine restart.
The script builds the release binary, validates the plugin, and restarts
`forge-engine.service`. When any plugin file changed, the installed plugin
tree is swapped atomically and `omarchy restart shell` runs after a short
settling delay, so the panel always shows the new UI; an unchanged plugin
is left in place and the shell keeps running. Before the engine restarts,
the update leaves an `update-pending` marker in the project's `.forge/`
directory; the restarted engine turns it into a "self-update finished"
event in the panel's feed, so a completed update is visible there.

If an update misbehaves, check the update log, engine log, and shell crashes:

```sh
journalctl --user -u forge-update
journalctl --user -u forge-engine
coredumpctl list /usr/bin/quickshell
```

If the panel does not reflect an installed update, compare the shell start time
from `ps -o lstart= -p $(pgrep -x quickshell)` with the update time in
`journalctl --user -u forge-update`. If the shell predates the update, run
`omarchy restart shell` manually to recover from a stale shell.

The 2026-09-06 crash came from an in-place plugin copy racing a shell restart.
The directory swap introduced afterward broke watcher-based hot reload, and
per-file atomic renames turned out not to refresh an already loaded panel
either, so plugin updates now always swap the tree and restart the shell.

### Keyboard

The panel uses vim-inspired normal and insert modes. In normal mode, `i`
focuses the goal for typing; clicking a text field also enters insert mode.
`Escape` leaves the field, or closes the top overlay when already in normal mode.

Open keyboard help with the `? Help` button in the panel header or by clicking the
mode hint at the bottom of the panel, which says "click here or press ? (Shift+/)
or F1 for keyboard help" and underlines on hover.

Actions follow the buttons’ enabled state. Uppercase keys use `Shift`.

#### Panel (normal mode)

| Key | Action |
| --- | --- |
| `i` | Edit the goal (insert mode) |
| `I` | Edit plan feedback (insert mode); Enter improves with AI |
| `Escape` | Leave a text field, close the top overlay, or cancel plan editing |
| `j` / `k` | Select next / previous stage |
| `gg` / `G` | Select first / last stage |
| `Enter` / `o` / `Space` | Expand or collapse selected stage; focus its title when editing |
| `Tab` | Toggle Live / History |
| `h` / `l` | Select Live / History |
| `Ctrl+d` / `Ctrl+u` | Scroll Live / History half a page down / up |
| `1` / `2` / `3` / `4` / `5` | History: All / Runs / Git / Reviews / Errors |
| `p` | Create plan from goal |
| `e` | Edit plan stages by hand |
| `a` | Approve draft plan |
| `r` | Run approved or completed plan |
| `x` | Stop run or active queue |
| `d` | Open uncommitted diff |
| `c` | Change project |
| `?` (`Shift+/`) / `F1` | Open keyboard help |

#### Diff viewer

| Key | Action |
| --- | --- |
| `j` / `k` | Scroll down / up |
| `Ctrl+d` / `Ctrl+u` | Scroll half a page down / up |
| `gg` / `G` | Jump to top / bottom |
| `R` | Refresh diff |
| `q` / `Escape` | Close diff |

#### Project chooser

These shortcuts apply in normal mode, with text-field behavior noted below.

| Key | Action |
| --- | --- |
| `j` / `k` | Select next / previous project |
| `Enter` | Open selection; in filter, open first match; in path field, set path |
| `/` / `i` | Edit project filter (insert mode) |
| `q` / `Escape` | Close chooser (Escape leaves a text field first) |

#### Keyboard help

| Key | Action |
| --- | --- |
| `?` (`Shift+/`) / `F1` / `q` / `Escape` | Close help before any other overlay |

Press `?` (`Shift+/`) or `F1` in the panel to open the same reference.
