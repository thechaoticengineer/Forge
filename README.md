# Forge

A minimal wrapper around AI coding agents (Claude Code, Codex CLI) for
building software — including Forge itself — with almost no ceremony.

Rust engine + Quickshell (Omarchy) panel.

## The loop

1. Point Forge at a git repository and describe a goal.
2. A planner agent (Claude Code or Codex) writes a staged plan to
   `.forge/plan-candidate.json`; Forge validates and publishes it to
   `.forge/plan.json` — each stage has instructions, acceptance criteria,
   and a proposed commit message.
3. You mark the plan OK in the panel.
4. Forge runs each stage automatically:
   - the **implementer** (one tool) implements the stage,
   - an independent, adversarial **reviewer** (the other tool, always a fresh session)
     reviews the uncommitted diff and writes `.forge/verdict.json`,
   - **changes requested** loop back to the implementer with all requested
     edits, up to a bounded number of fix rounds,
   - every fix gets another independent review in a fresh session, with the
     previous findings supplied for verification,
   - a **clean approval** ends the loop and the stage is committed with the
     proposed message; remaining requests after the budget is exhausted block
     the stage.
5. After the last stage, Forge pushes to `origin`.

The reviewer starts by assuming there is a defect and actively looking for
it in the diff and surrounding code. Before approving, it must verify each
acceptance criterion individually and independently run the project's
available build and tests. It must record the evidence and results, including
exact commands; if a build or test is unavailable, it must explain how it
established that. A failed or unrun available check, or an acceptance
criterion it could not verify, requires changes requested.

The reviewer's verdict has the shape
`{"approved": bool, "summary": str, "issues": [str, ...], "notes": [str, ...], "checks": [str, ...]}`.
The summary describes what the reviewer inspected and found, even on
approval. `issues` contain every specific, actionable requested edit, including
in-scope improvements; `checks` list the commands and inspections actually
performed and their results, including each acceptance criterion. New verdicts
must leave `notes` empty; the field remains for compatibility. `approved: true`
requires both `issues` and `notes` to be empty and all acceptance criteria and
checks verified. Requested edits require `approved: false` with actionable
issues. Forge normalizes any verdict containing issues or legacy notes to
changes requested, even if the reviewer supplied `approved: true`. Missing or
unreadable verdicts and negative verdicts without findings produce a change
request explaining the review failure.

The `max_fix_rounds` setting (default `3`) limits extra fixer/review rounds
after the initial implementation and review: by default, at most four reviews
per stage attempt. A second round is conditional on findings; a clean first
review proceeds directly to commit. Each subsequent round must independently
verify the previous requests and inspect the current implementation for
remaining or newly discovered in-scope defects. Reviewers must not repeat
resolved requests without evidence or invent findings merely because this is
a later round. Every remaining request requires another fix and review while
budget remains; exhausted requests block, including on the initial review
when `max_fix_rounds` is `0`. A clean approval on any round ends the loop.

Legacy settings containing `apply_review_notes` still load, but its value is
ignored and it is no longer a default setting. Neither `false` nor exhausted
budget allows notes to bypass the clean-approval requirement. There is no
approval-triggered polishing step.

Every review round is recorded in the stage's `reviews` array in
`.forge/plan.json`, with `round` (starting at 1), `approved`, `summary`,
`issues`, `notes`, `checks`, and `unix` (a Unix timestamp in seconds).
The latest normalized verdict is also stored as `last_verdict`. Earlier
feedback stays available through every fix and the final approval; completed
entries are not rewritten. Round numbering restarts at 1 when a stage is
retried in a new attempt, while earlier history remains intact. Saved
`approved: true` records with no issues and nonempty notes display amber
`approved with optional notes` in both stage summaries and review history,
with the feedback labeled `optional notes`. Saved approvals containing issues
retain the red `legacy approval with change requests` display. This historical
rendering does not rewrite saved records or change the current reviewer
contract: new approvals require empty issues and notes, and incoming legacy
notes still normalize to changes requested.

If the reviewer still requests changes after the fix rounds, the stage is marked
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

### Plan identity and architectural records

Plans now carry `contract_version: 1`, a project-local `plan_id`, and a
monotonic `revision` starting at 1. Manual edits and AI revisions retain the
identity and advance its revision; execution updates retain the revision.
Replacement plans, including successive queue goals, get fresh identities.
Discarding a plan archives it; the next plan also gets a fresh identity.
Every stage execution attempt has an `attempt_id`; review `round` numbering
still restarts at 1 for each attempt. New review history includes its role,
attempt identity, and plan revision.

Legacy plans remain readable without a write on startup or `/api/state`.
Their first mutation lazily imports them, retaining unknown metadata, usage,
reviews, and verdicts. Committed stages retain exactly the same JSON
values through edits and revisions. Other stages reconcile by stable IDs,
retain their history and usage, and mark obsolete `last_verdict_valid`,
`context_valid`, guidance, and agreements false. Removed stages remain in the
immutable historical snapshots. `stage_id_high_water` prevents recycling their
IDs. New stages may omit `id`.

Stages may specify `depends_on: [stage_id, ...]`, referring only to earlier
stages; `[]` explicitly declares independence. Without this field, all preceding
stages are dependencies. Relevant inputs are the goal, stage content, and
these dependency IDs and their transitive input descriptions. Content or goal
changes invalidate affected work and its transitive dependants. An unrelated edit or a reorder consistent with explicit
dependencies retains an unchanged stage's agreements. Editing a draft still
requires human approval again.

Per-plan artefacts live under `.forge/architecture/<plan-id>/`:

- `events.jsonl`: serialized append-only committed events for decisions,
  guidance, model proposals/agreements/selections/invalidations, and role-tagged
  reviews. Event envelopes include version, ID, plan, revision, timestamp, and
  checkpoint reference. Imported legacy details remain in immutable review files.
- `checkpoints/<checkpoint-id>.json`: immutable bundles containing the exact
  published plan and a bounded architecture checkpoint. The checkpoint holds
  the exact provider session and provider checkpoint reference, context summary,
  up to eight recent decision summaries, current guidance/agreements, and review
  policy with its rationale. Architecture checkpoint data is limited to 64 KiB;
  execution loads restore the original review arrays and historical metadata.
- `reviews/<file-id>.jsonl` and `.idx`: immutable review arrays and binary
  little-endian u64 record offsets. The plan's `architecture.review_history`
  manifest binds stage IDs to exact files, counts, byte lengths, and at most eight
  previews. On disk, a stage's reviews use `{"$forge_reviews":"<file-id>"}`;
  execution and editing hydrate them back to the original JSON arrays. Unchanged
  arrays reuse files, and removed stages retain their manifest entries.
- `archived.json`: the last published plan reference, written before replacement
  or discard so its committed history remains addressable.

`src/contracts.rs` defines version-one provider-neutral invocation/result,
catalogue, decision, guidance, model-selection, and review records. Decisions
have stable IDs, rationale, alternatives/tradeoffs, status/supersession, stage,
revision, and timestamps. Model records distinguish proposals, agreements, and
effective provider/model/native effort; retain both participants' reasons,
capability-policy/catalogue provenance, availability verification state, trigger,
and superseded agreement. These are storage contracts only. Model discovery,
architect invocation, routing, and dual review gates are **inactive** in this
stage. The existing independent review path remains in use; the panel reports
architecture context as `inactive` until later lifecycle work is implemented.

Publication uses an explicit commit protocol under a per-project persistence
mutex (one engine writer per project):

1. Keep the authoritative `plan.json` in place while a planner writes its
   separate candidate. Validate/reconcile the candidate before publishing.
2. Append and sync a versioned event. Write, sync, and read back any new review
   files/indexes, then write, sync, and read back an immutable checkpoint bundle. Sync parent directories as well. The proposed plan records
   the checkpoint ID and exact committed byte range in the event log.
3. Validate nested checkpoint contracts against the plan, the review files, and
   the persisted bundle/event linkage, then atomically publish `plan.json`. This
   rename is the single transaction publication boundary;
   independent snapshot renames are preparation, not additional commits.

On restart, only the checkpoint and event prefix named by `plan.json` are
current. A crash before publication keeps the old plan/context; a crash after
publication selects the complete new plan/context/review references. Unreferenced
snapshots, review files, and temp files are ignored. Before another append, only the uncommitted event suffix is
truncated; committed bytes and checkpoint files are retained. A reported sync
failure after rename restores a checked rollback link to the old publication.
If storage also prevents rollback, the error explicitly reports an uncertain
outcome and retains the rollback file for recovery. Malformed, unsupported,
truncated, or mismatched authoritative records fail closed and surface through
API state; Forge does not silently pick an arbitrary checkpoint.

Session references require `resume_policy: "fork_from_checkpoint"`: a later
provider adapter must fork/recover from the exact saved provider checkpoint,
never resume its mutable latest tail. Revision publication marks existing
session context `needs_recovery`; a failed revision cannot publish its provider
turns as valid architecture context. Providers without checkpoint-fork support
must start a fresh session from the bounded checkpoint in the later session
implementation.

`GET /api/state` adds a bounded `architecture` summary (identity, revision,
checkpoint/event position, context status, at most 2,000 summary characters and
eight decisions of at most 240 summary characters) and `persistence_error`.
The panel displays revision, context status, and recent decisions read-only.
Stage review arrays in state contain at most eight previews (at most 4 KiB each),
with `review_count` and `reviews_truncated` when abbreviated. Complete review
arrays are not read during polling of indexed publications. The first read of
an unchanged legacy file scans it once and caches only a bounded state view;
state reads do not migrate it. Its next mutation writes indexed review files
without changing any historical record. Existing inline snapshots remain readable.

`GET /api/architecture/reviews?stage_id=1&cursor=0&limit=20` returns complete review
records with a record-number `next_cursor` and total `count`. Pages contain at
most 100 records and normally at most 256 KiB; a single larger legacy record is
returned intact on its own page. Use the returned `checkpoint` on subsequent
requests to pin a snapshot while execution continues. Removed-stage reviews are
still addressable by stage ID. `plan_id` selects an archived plan and `project`
selects its project without switching the active project. Before migration,
review pagination reads the legacy inline array.

`GET /api/architecture/history?cursor=0&limit=20` pages committed events; follow
`next_cursor` until null. Cursors are byte offsets at event boundaries. Pages
are capped at 100 events and 256 KiB, with each event capped at 64 KiB. Add
`plan_id=<archived-id>` to inspect a replaced/discarded plan, and the usual
`project` query parameter for another project. State never reads the full
architecture log. Existing feed/chat/report tail reads are capped at 2 MiB.
All these artefacts remain inside the existing `.forge` commit exclusion.

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

### Token usage and run reports

Forge tracks token usage from each agent invocation when the CLI supplies it.
Claude Code's `stream-json` `result` event provides input, output, and cache
counts in `usage`, plus per-model detail in `modelUsage`. Forge folds
`cache_creation_input_tokens` and `cache_read_input_tokens` into
`input_tokens`, without storing separate cache counts; `total_tokens` is
input plus output. It uses `modelUsage` to select the model with the largest
token count and attributes the entire invocation's total to that model.
The full per-model breakdown is not retained.

Codex's `tokens used` output supplies only `total_tokens`; its stored input
and output counts are zero. When usage has no model, Forge uses the configured
model for the call if known.

Usage is accumulated in `.forge/plan.json`:

- Each stage's `usage` includes implementation, review, and fix
  calls; the plan's top-level `usage` holds the run's accumulated stage totals.
- Top-level `planner_usage` records planning usage separately; it is not
  included in `usage`.
- Each usage map is keyed by tool (`claude` or `codex`). Each tool entry has
  `input_tokens`, `output_tokens`, `total_tokens`, `calls`, and `models`
  (a map from model name to attributed total tokens).

Stage usage is saved after each successful call with nonzero usage and
retained when resuming a plan. Missing or zero usage does not add a call
to the totals. Failed agent invocations are not accumulated. Plan Q&A
calls log reported usage on completion but do not add it to plan totals.

Each completed run appends one JSON object for its goal to
`.forge/reports.jsonl`, keeping reports available after the current plan is
replaced. Each line has:

- `unix`: completion time as a Unix timestamp in seconds.
- `goal`: the plan's goal text.
- `duration_secs`: elapsed seconds for the finishing run attempt, excluding
  planning and earlier attempts if the plan was resumed.
- `stages`: the number of stages in the plan.
- `commits`: an array of stage commits, each with `sha`, `message` (the
  stage's `commit` text, or `forge: stage` if `commit` is missing or is not
  a string), and `title` (the stage title). Stages without a recorded SHA
  are omitted.
- `usage` and `planner_usage`: copies of the plan's token totals by tool and
  model, included when present.

`GET /api/state` returns the last 100 reports for the project under `reports`
(an empty array when none exist). In the panel, open **History** and select
**reports** to see completed tasks with their duration, commit count, and
token totals per tool. Expand a task to see commit SHAs and messages, input/output/total
counts, call counts, model totals, and separate planner usage. The reports
filter appears once reports exist. The plan header and expanded stage rows
also show token summaries when available.

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
Each reviewed stage has a **last completed review** chip showing its own
round and decision: a clean `approved`, amber `approved with optional notes`
for saved `approved: true` records with no issues and nonempty notes, or a red
change-request decision. Saved approvals containing issues display
`legacy approval with change requests`; rejections display `changes requested`.
Only decisions displayed as change requests have a request-count suffix
(`· N requests`): counts combine issues and legacy notes, counting identical
requests once. Historical optional-note approvals have no request-count
suffix. Current activity appears separately in bold, for example
`now: reviewing · round 2` or `now: fixing for review · round 2`, so round one's
decision cannot be mistaken for approval of work under review. An older clean
decision is muted during current activity. Budget exhaustion displays
`blocked · review budget exhausted` alongside the last decision.

Expand a stage to see recent review previews with recorded round, decision, summary,
change requests, notes, checks under `verified:`, and timestamp (UTC).
Earlier requests remain visible after final approval. Expanded history uses
the same decisions, colors, and request counts as the chip: historical
notes-only approvals display amber `approved with optional notes`, with their
feedback labeled `optional notes`. Saved approvals containing issues retain
the red `legacy approval with change requests` label and request count;
notes on change-request decisions are labeled `legacy notes (change requests)`.
All feedback remains available through the paginated review history API. Plans with only `last_verdict`
use the same fallback record for both the chip and expanded feedback. Missing
notes, checks, or history are supported; unavailable timestamps are omitted and
unknown review rounds are labeled `round unknown`, rather than borrowing the
active round.

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
| `6` | History: Reports (when reports exist) |
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
