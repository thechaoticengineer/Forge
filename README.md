# Forge

A minimal wrapper around AI coding agents (Claude Code, Codex CLI) for
building software — including Forge itself — with almost no ceremony.

Rust engine + Quickshell (Omarchy) panel.

## The loop

1. Point Forge at a git repository and describe a goal.
2. A planner returns a staged plan, and a persistent architect supplies
   architectural context and stage guidance. Forge validates their output and
   publishes `.forge/plan.json` — each stage has instructions, acceptance
   criteria, and a proposed commit message.
3. You mark the plan OK in the panel.
4. Forge runs each stage automatically:
   - the **implementer** implements the stage,
   - the engine classifies the full implementation snapshot,
   - a fresh, adversarial **independent reviewer** verifies the stage and its scope,
   - the persistent **architect** also reviews code and architectural contracts,
   - requested edits return to the implementer, followed by all required reviews again,
   - the engine commits only a current, clean aggregate gate and the exact reviewed tree.
5. After the last stage, Forge pushes to `origin` when `auto_push` is enabled.

### Scope and review authority

Scope policy version 1 belongs to the engine. Ordinary documentation needs one
fresh independent review; **architect review is not required**. The engine
records the committed outcome in the architectural checkpoint so subsequent
stages retain context without an architect approval or summary turn.

The documentation exception requires both prose-oriented stage intent and the
actual full diff: staged changes, unstaged changes and untracked implementation
content. Spelling, explanations and prose examples consistent with existing
behavior can qualify. An extension or implementer declaration alone cannot
qualify. API/schema/interface contracts, design decisions, normative architecture
or security requirements, executable examples, build/configuration changes,
mixed changes and uncertain scope require **both roles**, including in Markdown.

The classifier deliberately errs toward dual review. It considers existing,
non-executable `.md`, `.txt` and `.rst` files only; unknown/new/deleted files,
mode changes, executable syntax, code fences and contractual/normative language
in intent or full diff context require both roles. Its checks are conservative
lexical signals, not proof of semantic equivalence. The independent reviewer
must inspect actual behavior and confirm scope. It promotes suspected
architectural impact with `requires_dual`, explaining why in `scope_reason`.
Promotion records a new policy and requires new verdicts bound to that policy;
previous verdicts remain historical. Scope is checked after each fix and before
commit. Once dual review is required, it remains required for the attempt.

Both roles inspect the same HEAD, index and working content before a fixer
runs. Reviews are sequential because the engine has one active-agent/log state.
The architect checks recorded decisions, cross-stage interfaces and regressions.
The independent reviewer receives agreed constraints and preceding actionable
requests, never the architect's current approval as an endorsement. The engine
resolves its provider as the other provider relative to the actual implementer.
The configured reviewer must match that provider; known unavailable models or
incompatible effort/permission capabilities block execution. Configured explicit
unverified fallbacks remain visibly unverified until execution verifies them.
Automatic stage routing selects the implementer; the review gate enforces the
same requirements for every selected model.

Each role retains its authority. The aggregate keeps requests with `[architect]`
or `[reviewer]` provenance. A reported `architecture_context_gap` requests a
persistent architectural clarification and records its guidance/decisions;
it does not erase either role's unresolved requests. Fixes must satisfy both
roles wherever dual review applies. Clean first-round results need no fixer.

### Verdict protocol and verification

Reviewers return JSON in their final response; the engine alone publishes
validated output. Old `.forge/verdict.json` and `architect-verdict.json` files
are cleared before calls and cannot supply an approval. Every verdict echoes an
exact `identity`: plan ID/revision, stage ID, attempt ID, round, role, policy and
snapshot. The snapshot fingerprints HEAD and its symbolic reference, staged
and unstaged diffs, index entries, and tracked/untracked implementation bytes
and executable/symlink modes. Forge runtime artifacts under the root `.forge/`
are excluded. A normalized tree hash separately represents the eventual commit.

Alongside `approved`, `summary`, `issues`, `notes` and `checks`, output includes:

- `requires_dual` and `scope_reason` for scope verification;
- `criteria`, with each supplied criterion, its status and concrete evidence;
- `acceptance_evidence`, echoing the complete acceptance text, verification and evidence;
- `project_checks`, recording exact commands, passed/failed/unavailable status,
  and actual output or evidence establishing that a check does not exist.

Every acceptance criterion must be individually verified. The independent
reviewer must run all available required project builds/tests, including for
documentation. Rust repositories with a root `Cargo.toml` additionally require
successful `cargo build` and `cargo test` entries at the engine boundary.
Other project-specific requirements are discovered and verified by the reviewer.
An unavailable dependency or sandbox restriction preventing an available check
from running is a rejection. Evidence is validated structurally and inspected
by the reviewing agents; the engine does not infer correctness from a command
name alone. Failed, unrun or unevidenced available checks prevent approval.

Reviews use Bubblewrap (`bwrap`) to make the host repository, Git metadata and
Forge runtime files read-only. Private `/tmp` is writable for faithful scratch
copies and build/test outputs. Only the selected provider's session directory
is writable host state. Provider hooks, apps and external tool servers are
disabled; the independent reviewer has no resume reference. Missing sandbox or
permission capabilities fail closed. Reviewers run checks in scratch copies of
the actual implementation without editing source, then return only scoped JSON.
The engine also verifies content has not changed after each role.

Missing/malformed/stale identities, malformed fields, contradictory approvals
and missing evidence invalidate the round. Incoming legacy `notes` normalize
to actionable requests even when `approved` was true; new output must leave
notes empty. A rejected verdict without details receives a verification request.
`apply_review_notes` is ignored and cannot bypass the gate.

### Budgets, commits and history

`max_fix_rounds` (default `3`) means extra fix/review rounds after the initial
round: at most four implementation/fix rounds by default, each with all required
roles. A scope promotion can require a fresh independent verdict in the same
round under the new policy. Rounds are reserved durably before calls. Stops,
errors, partial paired-review failure and process restarts consume the reserved
round and never reset the saved budget or reuse approval. Exhaustion blocks the
stage. An approved plan revision starts a new attempt; changing a setting or
pressing Run again does not replenish the current attempt.

Commit validation checks current required verdicts, identity, policy and the
unchanged snapshot. After staging, the actual index tree must equal the reviewed
normalized tree, with unchanged worktree content and HEAD. The engine creates
that exact tree's commit with `git commit-tree` and advances HEAD with an expected
old-value `git update-ref`; concurrent HEAD changes are rejected. This path does
not run commit hooks, so required project checks must be evidenced during review.
Successful commits add an engine checkpoint outcome without modifying reviewed
files. The final content check happens immediately before the ref update;
external tools should not edit a repository during an active stage.

Immutable role records accumulate in `reviews`; `last_verdict` remains the
latest independent verdict for compatibility. `review_policy` and the distinct
`review_gate` expose current policy/reason, each role's outcome, aggregate status
and actionable requests. An architect outcome of `not_required` explicitly means
**architect review not required**, never approval. Historical approval is shown
separately from current pending, blocked, error, interrupted or invalidated gates.
Legacy note-only approvals keep their historical rendering; incoming notes never
permit a current clean gate. Full feedback remains in paginated review history.

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
Every stage execution attempt has an `attempt_id` and saved review budget. Stops
and restarts preserve both the attempt and consumed rounds. An approved plan
revision starts a new attempt at round 1. Review records bind role, attempt,
plan revision, scope policy and implementation fingerprint.

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
  policy with its rationale. Stored architecture checkpoint data is limited to 64 KiB;
  execution loads restore the original review arrays and historical metadata.
- `reviews/<file-id>.jsonl` and `.idx`: immutable review arrays and binary
  little-endian u64 record offsets. The plan's `architecture.review_history`
  manifest binds stage IDs to exact files, counts, byte lengths, and at most eight
  previews. On disk, a stage's reviews use `{"$forge_reviews":"<file-id>"}`;
  execution and editing hydrate them back to the original JSON arrays. Unchanged
  arrays reuse files, and removed stages retain their manifest entries.
- `archived.json`: the last published plan reference, written before replacement
  or discard so its committed history remains addressable.

Checkpoints and history events that exceed 64 KiB as plain JSON use a lossless
`{"$forge_compact":1,"strings":[...],"value":...}` storage envelope. String values
are stored once, so long stage descriptions repeated in transitive dependencies,
guidance and model agreements do not multiply the stored size. Tagged object and
string nodes preserve arbitrary user metadata without reference collisions.
The stored limit remains 64 KiB (including the newline for events); expanded
records and history pages are bounded at 4 MiB. Reads restore the exact original
JSON before contract validation, routing comparisons, prompts and API responses.
Existing plain version-one files remain readable and small new records retain
their original format. Unknown encodings, invalid references and expansion beyond
the bound fail closed. Oversized unique data reports its measured size and limit.

`src/contracts.rs` defines version-one provider-neutral invocation/result,
catalogue, decision, guidance, model-selection, and review records. Decisions
have stable IDs, rationale, alternatives/tradeoffs, status/supersession, stage,
revision, and timestamps. Model records distinguish proposals, agreements, and
effective provider/model/native effort; retain both participants' reasons,
capability-policy/catalogue provenance, availability verification state, trigger,
and superseded agreement. Discovery, persistent guidance, joint stage routing,
bounded reassessment and dual review gates are active. Independent reviews
always start fresh.

Publication uses an explicit commit protocol under a per-project persistence
mutex (one engine writer per project):

1. Keep the authoritative `plan.json` in place while the planner returns its
   separate candidate. Validate/reconcile the candidate and architect output
   before publishing.
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

Architect sessions use `resume_policy: "exact_if_committed"`. Before a provider
turn, Forge syncs a plan-owned `architect-pending.json` marker. The resulting
checkpoint records the same turn ID, exact provider session UUID and effective
model. Only publication of the plan/checkpoint makes that session tail resumable.
Unchanged boundaries reuse saved guidance without an agent call. A direct plan
edit invalidates only affected guidance and dependency inputs, while retaining
the committed session identity.

An interrupted or failed turn leaves the old plan, decisions and checkpoint
authoritative. Its pending marker forces a fresh session reconstructed from that
checkpoint, the full current plan, verified outcomes and retrievable decision
history. A missing/expired session or provider change also triggers reconstruction;
replacement identity and reason are recorded. Older `fork_from_checkpoint`
references are reconstructed rather than resumed. Failed draft revisions never
make their proposed plan or provider tail authoritative. A marker matching the
published turn remains safe even if the engine stops immediately after publication.

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
are capped at 100 events and 256 KiB of stored data (4 MiB expanded), with each
stored event capped at 64 KiB. Add
`plan_id=<archived-id>` to inspect a replaced/discarded plan, and the usual
`project` query parameter for another project. State never reads the full
architecture log. History pages include the stable `plan_id` and current
`checkpoint`; event payloads retain decisions (including supersessions), model
agreements and invalidations. Review pages retain each role's full verdict and
identity. For example, after a queued goal has completed and been replaced:

```text
GET /api/architecture/history?plan_id=<completed-plan-id>&cursor=0&limit=20
GET /api/architecture/reviews?plan_id=<completed-plan-id>&stage_id=1&cursor=0&limit=20
```

Use the event byte cursor only with the history endpoint, and the review record
cursor only with the reviews endpoint. Both endpoints are read-only and accept
an encoded `project` path without switching the active project. Existing
feed/chat/report tail reads are capped at 2 MiB each.
State keeps only eight recent model invocations per stage with
`model_invocation_count` and `model_invocations_truncated`, and four reassessment
history entries. Full selection dialogue and transitive input descriptions stay
in durable artefacts; polling preserves the choice, both reasons, native effort,
policy provenance, current gates, editing content and token totals.
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

Codex JSONL supplies input/output totals; cached input is already included in
input tokens. Legacy `tokens used` text remains a total-only fallback. When usage
has no model, Forge uses the reported effective model or configured model.

Usage is accumulated in `.forge/plan.json`:

- Each stage's `usage` includes implementation, review, and fix
  calls; the plan's top-level `usage` holds stage and architect totals.
- Top-level `planner_usage` records planning usage separately; it is not
  included in `usage`. `role_usage` additionally separates planner, architect,
  implementer, fixer and reviewer totals.
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
- `version: 1`, `project`, `plan_id` and `revision`: the completed plan's stable identity.
- `architecture`: the published checkpoint reference, architectural summary,
  recent decisions/supersessions, constraints, risks and verified outcomes.
- `stage_outcomes`: each stage's final assignment and original validated proposal,
  both selection reasons, configured tier/cost provenance, nullable published
  pricing, last actual invocation, reassessment counts, review policy and its
  rationale, separate role outcomes and aggregate identity, and usage.
- `usage`, `planner_usage` and `role_usage`: copies of the plan's token totals,
  included when present. Role totals retain provider/model breakdowns after
  restart or plan replacement. `/api/state.role_usage` uses durable plan totals;
  `session_role_usage` contains transient session counters.

Report appends are synced before a queue goal is marked complete. A storage
failure is reported and keeps that queue goal for recovery; its commits remain
local. Full decisions and superseded agreements remain addressable through
architecture history using the report's plan ID. Reports summarize token usage;
they do not estimate CLI spend or convert API list rates into subscription costs.
The append log is not an exactly-once completion ledger: explicitly rerunning a
completed plan can append another report for the same plan/revision.

`GET /api/state` returns the last 100 reports for the project under `reports`
(an empty array when none exist). In the panel, open **History** and select
**reports** to see completed tasks with their duration, commit count, and
token totals per tool. Expand a task to see commit SHAs and messages, input/output/total
counts, call counts, model totals, separate planner and role usage, architecture,
routing reasons and recorded review gates. Older reports without these fields
retain their original rendering. The reports
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
PATH, logged in. Review execution also requires `bwrap` and `sha256sum`.

The Omarchy plugin (`manifest.json`, `quickshell/`) provides the bar
widget and the Forge panel: pick planner/architect/implementer/reviewer, set the
project path, type the goal, create the plan, approve, start.
Each stage shows its **review policy**, **architect and independent outcomes**,
and **current aggregate gate**. Each reviewed stage also has a **historical review** chip showing its own
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
feedback labeled `legacy notes (change requests)`. Saved approvals containing issues retain
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

### Model catalogue and explicit fallback policy

Forge schedules one application-owned model refresh at engine startup, then
repeats discovery every `discovery_refresh_minutes` (default 360, range
5–10080) on an engine-side timer — the panel never polls for this. Both
providers are probed concurrently; project sessions share that work. Startup,
project workers and `/api/state` never wait for discovery. The panel shows
provider status, sanitized refresh/cache errors, configured provenance and
revision, and the last execution's availability evidence. **Refresh models**
starts another asynchronous refresh; **Cancel refresh** cancels discovery only.

Discovery states distinguish `discovered`, `cached_stale`,
`unsupported_discovery`, `discovery_failed`, `pending` and `unavailable`.
Configured selection options independently carry `configured_unverified` when
availability cannot be verified. An absent discovery mechanism is not proof
that a model is unavailable. Exact registry entries and existing explicit role
model settings can run with unverified availability, using provider-default
reasoning or an explicitly configured native effort supported by the adapter. Planner and architect bootstrap additionally require a configured
strong tier (see the bootstrap section below). The run state and history retain that qualification until successful
execution supplies evidence. Missing executables, observed authentication
failures, rejected models and previously discovered models that were removed
block selection. A transient refresh failure cannot clear an observed blocker.
Successful discovery can establish availability again after a discovery failure.
Execution-auth and model/effort rejections stay blocked within their configuration
scope even if a later catalogue lists the option. After correcting authentication
or provider configuration, change the scope identifier to request new evidence.

Use **Model settings & options** to edit the JSON policy. The policy is saved
atomically to `$XDG_CONFIG_HOME/forge/model-policy.json` (default
`~/.config/forge/model-policy.json`) and loaded at engine startup. An invalid
file is reported in the panel; Forge uses an empty registry. Set a new
`policy_revision` whenever changing the policy. For example, replace
`your-exact-model-id` with an ID from your CLI or your explicit configuration:

```json
{
  "policy_revision": "2",
  "codex_scope": "default",
  "claude_scope": "default",
  "claude_bridge": "",
  "entries": [
    {
      "provider": "codex",
      "model": "your-exact-model-id",
      "tier": "strong",
      "suitability": ["general"],
      "limits": {},
      "relative_cost_preference": 2,
      "effort": "provider_default"
    }
  ]
}
```

The registry has at most 64 unique provider/model entries. `tier` is `basic`,
`standard` or `strong`; these are **configured user policy**, not provider claims
or rankings inferred from prices. Optional suitability tags and numeric limits
also have configured provenance. For stage routing, empty suitability or `general`
permits every task; otherwise use explicit task tags: `documentation`,
`functionality`, `concurrency`, `persistence`, or `security`. Older descriptive
suitability strings must be replaced with these tags before using an entry for
stage assignments. Numeric limits are descriptive inputs for both participants;
they do not grant a higher capability tier. `relative_cost_preference` is nullable, ranges
from 0 to 1000, and lower means preferred; it is not a monetary amount. Configured
entries do not supply verified prices; missing official prices remain null.
No current model IDs or capability rankings are built into Forge.
The planner and architect use these inputs to agree on stage assignments before
approval, with the engine enforcing adequacy and supported native efforts.

`provider_default` emits no native effort override. A specific effort must be
advertised for that model by discovery and supported by the adapter, or match an
explicit registry `effort` when model-specific effort support is unknown. The
latter fallback accepts Codex `none`, `minimal`, `low`, `medium`, `high`, `xhigh`
and Claude `low`, `medium`, `high`, `xhigh`, `max`. These are adapter-supported
values, not a claim that an unverified model supports them. A discovered empty
or contradictory supported-effort list, an observed effort rejection, and known
operational failures always block. Unknown or unconfigured efforts are rejected
without emitting arguments; use `provider_default` or correct the configuration. Discovery never sends a generation prompt, requests new
credentials, reads credential files, scrapes interactive pickers, or treats an
Anthropic API model list as Claude Code subscription access.

Read-only details are at `GET /api/models`: normalized IDs, aliases/resolved IDs,
defaults, native efforts, known capabilities, nullable pricing, catalogue diffs,
and configured options with their eligibility and provenance. To inspect one
selection input, use
`GET /api/models?provider=codex&model=your-exact-model-id&effort=provider_default`.
`POST /api/models/refresh` returns 202 with `started: false` when joining existing
work; `POST /api/models/cancel` also returns immediately. `/api/state` contains
counts and status rather than the full registry/model arrays. Save the policy
with `POST /api/settings` and a `model_catalogue` object. Policy changes during
refresh return 409; invalid changes return 400 without partially updating other
settings. Scope/bridge changes schedule a refresh automatically.

### Official metadata refresh (bounded research)

A separate metadata service enriches the catalogue with routing-relevant facts
from official documents only. It is a small deterministic HTTP path, not a
research agent, and never issues generation requests or paid model calls.
Runtime discovery (evidence), configured policy (user intent) and official
metadata (facts) stay separate: official pages never grant runtime
availability, and cache-only or failed research never prevents explicitly
configured tiers from routing.

Sources are limited to an enforceable HTTPS allowlist —
`developers.openai.com`, `platform.openai.com`, `learn.chatgpt.com`,
`code.claude.com` and `platform.claude.com` — with every redirect hop
revalidated; off-allowlist URLs, ports, credentials and non-HTTPS schemes are
rejected. Two documented source adapters are fetched, one index document per
provider (`learn.chatgpt.com/docs/models.md` and
`platform.claude.com/docs/en/models/overview.md`). A JSON body must contain
`{"models":[{"id", "context_window", "max_output_tokens", "reasoning",
"lifecycle", "pricing"}]}`; a markdown body is read only through two exact,
bounded patterns the official pages publish — comparison tables keyed by a
"Claude API ID" row (context window and max output per column) and
`slug="…"` model attributes, which name models without providing routing
facts. Prose is never interpreted; unparseable values stay unknown. Document
content is data, never instructions.
Unsupported documents and models absent from their source get negative-cache
entries with exponential backoff (1 h doubling, capped at 24 h) instead of
arbitrary browsing or fabricated facts. Fields a source does not provide are
recorded as explicitly unknown — including quality comparisons, which are
never inferred; configured tiers and relative preferences still route.

Research is selective: only new, retargeted or materially changed models
(tracked by a discovery fingerprint), newly missing routing fields, or records
past `metadata_ttl_hours` (default 168, range 1–8760) trigger requests, and a
fresh unchanged startup makes none. Identical source requests are coalesced,
conditional revalidation (`ETag`/`Last-Modified`) is used where available, and
transfers are bounded (2 MiB, timeouts, one retry). The engine refreshes
metadata every `metadata_refresh_minutes` (default 1440, range 15–10080);
`metadata_research: false` keeps the store cache-only. Refresh never alters
in-flight model selections, and a `304 Not Modified` bumps only the
verification time — timestamp changes are never treated as material.

Each record stores its source URL, verification time, content fingerprint,
provenance (`official`) and explicit unknowns; last-known-good records survive
fetch failures and are kept for audit (flagged `removed`) when discovery drops
a model. Unknown costs stay null. A reported rate is stored only complete —
currency, unit, billing basis, source date and the explicit `api_list_rate`
label — and is never turned into an inferred CLI subscription charge;
`relative_cost_preference` is user policy, not a measured rate. When official
metadata contradicts discovery (e.g. reasoning support), the conflict is
surfaced and discovered native support wins. The store lives beside the
discovery cache at `$XDG_CACHE_HOME/forge/models/v1/metadata.json`. Freshness,
provenance, unknown pricing, negative-cache and retry/error details appear in
the panel's catalogue view and under `metadata` in `GET /api/models` and
`/api/state`; `POST /api/models/metadata/refresh` starts a pass manually.

Availability caches use atomic, versioned JSON under
`$XDG_CACHE_HOME/forge/models/v1/` (default `~/.cache/forge/models/v1/`). Partitions
include the provider, explicit scope, executable search path, provider config
location and non-credential endpoint/backend configuration. They retain CLI
version, last successful refresh, added/removed IDs, alias retargeting and
capability/default/effort changes. Refresh failures preserve the last good file;
corrupt or mismatched caches are ignored and reported. Removed IDs remain
recorded across restarts. Discovery uses the user's home directory, independently
of the active project; change the relevant scope identifier after switching
accounts or user configuration in place. Project-specific CLI overrides are not
authoritative catalogue evidence. Every probe has a 15-second total budget,
page/output limits, stderr draining, cancellation, and cleanup of its own child
process group. The separate engine metadata timer applies the selective refresh
rules above; it does not start a selection dialogue for unchanged work.

### Optional Claude discovery bridge

Codex discovery was checked against installed **codex-cli 0.153.2**, its locally
exported `generate-json-schema` protocol, and the official
[Codex App Server documentation](https://developers.openai.com/codex/app-server).
The adapter performs `initialize`, `initialized`, an optional `account/read`
check, then paginated `model/list` with hidden entries included. Interleaved
notifications and unknown optional metadata are tolerated; no thread or turn is
started.

Claude Code **2.1.261** has no supported `claude models` command. The optional
bridge uses the official
[TypeScript Agent SDK initialization API](https://code.claude.com/docs/en/agent-sdk/typescript),
verified against the publisher's **0.3.261** package types and implementation.
It awaits `supportedModels()` with an input stream that never yields a message,
then closes the SDK query. It selects the existing `claude` executable and its
existing CLI authentication, disables persistence/tools/MCP execution for the
probe, and emits no account data. Models returned by initialization describe
CLI options; actual execution remains the evidence of successful access.

The bridge is optional and is not installed by `install.sh`. It needs Node.js
18+ on the engine's PATH and the pinned SDK dependency. To install it manually:

```sh
cd bridges/claude-models
npm install --omit=optional
```

Set `claude_bridge` to the absolute path of `bridges/claude-models/bridge.mjs`
(and change `policy_revision`). The SDK's bundled CLI is unnecessary: Forge
uses the existing `claude` executable. Protocol v1 deliberately accepts only
SDK 0.3.261 / Claude Code 2.1.261; a different version requires rechecking the
SDK/CLI schema and updating the bridge and fixtures. Missing Node, bridge or
SDK, or an unsupported version reports unsupported discovery. It never falls
back to an API-key request or a generation turn. Without the bridge, explicit
Claude entries continue to work with unverified availability and provider-default
effort.

`cargo test` uses fake discovery/process protocols and isolated temporary
caches, including HTTP responsiveness tests. The optional bridge's no-prompt
contract and panel controls can be tested without installing its SDK:

```sh
node --test bridges/claude-models/discovery.test.mjs tests/panel_catalogue.test.mjs
```


### Architect bootstrap and session operation

The planner and architect have separate provider/model settings: `planner` /
`planner_model` (default provider `claude`) and `architect` / `architect_model`
(default provider `codex`). Each selects an eligible `strong` entry from the
configured model registry. An explicit model must match a strong registry entry;
an empty model selects the first eligible strong entry for that provider. This
uses configured tiers even when official comparative metadata is absent. An
explicit registry model may remain `configured_unverified` under the existing
fallback policy. Known unavailable models are blocked. An empty registry requires
configuration before planning; Forge never silently substitutes an unclassified
provider default for these two roles. `mock` remains available for offline tests.

Configure both providers' strong entries through `model_catalogue.entries` in
`POST /api/settings` (or the persisted model policy described above). Native
reasoning effort comes from each entry's `effort`. `provider_default` sends no
effort flag. Other values require discovered native support or the explicit
configured effort fallback described above; Forge never translates effort names
between providers.

The planner inspects the repository and returns a candidate JSON plan. The
architect receives the goal, full candidate and repository observations before
publication, and returns a structured checkpoint, decisions/supersessions,
guidance and unresolved risks tagged with the expected plan ID and revision.
Forge validates the complete output and publishes plan and context together.
Architect guidance is included in implementation and fix prompts. A reviewer
may supply a nonempty `architecture_context_gap` (at most 2,000 bytes) in a
rejected verdict to request refreshed guidance before a fix. Ordinary fix rounds,
unchanged stage boundaries, stops and completion do not incur summary turns.
Successful commits append engine-verified outcomes to durable history and a
bounded recent checkpoint preview. The full plan retains all committed stages.

Stored checkpoints are capped at 64 KiB (4 MiB expanded), and individual architect
responses at 48 KiB.
Saved constraints and completed interfaces cannot be silently dropped; unresolved
risks require explicit resolution by ID. Recent decision details remain bounded,
while the append-only history retains full rationale, alternatives and
supersessions. Subsequent prompts include the saved checkpoint, at most 24 KiB of
active decision details and the history path for further retrieval. Capacity or
validation failures stop required work and expose a recoverable error, preserving
the last good checkpoint.

Read-only roles use enforceable provider permissions. Codex runs with the
read-only sandbox, approval policy `never`, ignored user configuration/exec rules,
empty MCP configuration and disabled hooks/apps/delegation/browser/computer/image
tools. Claude uses safe mode, `dontAsk`, only `Read,Glob,Grep`, no MCP servers and
disabled hooks. Claude can inspect files; checks requiring shell execution must
be performed by another role. The engine writes validated planner, architect and
Q&A artefacts. Unsupported permission flags produce a recoverable capability
failure. Implementation permissions and the existing independent review gate
are unchanged in this stage.

Adapter flags were checked against installed Codex CLI **0.153.4** (`exec --help`
and `exec resume --help`) and Claude Code **2.1.263** (`--help`), and the official
[Codex CLI reference](https://developers.openai.com/codex/cli/reference),
[Codex non-interactive output documentation](https://developers.openai.com/codex/noninteractive),
[Claude CLI reference](https://code.claude.com/docs/en/cli-reference) and
[Claude settings reference](https://code.claude.com/docs/en/settings).
Codex JSONL and Claude stream-json are decoded defensively, including exact
session IDs, terminal failures, effective model and usage. Codex cached input is
a subset of input tokens; Claude cache creation/read tokens are added to input.
Human-readable activity stays in `agent.log`; role usage is exposed separately.
Child process groups are cleaned up on completion, stop and reader failure.
Neither adapter uses generic `--continue` or `--last`.

Q&A always uses a fresh read-only conversation with a snapshot of saved context.
It writes chat answers but never mutates the plan, decisions, checkpoint or
authoritative architect session. `GET /api/state` and the panel expose architect
activity/recovery, exact session identity, current guidance, decision details,
unresolved risks and usage per role, including legacy plans without context.


### Joint stage model assignments

`automatic_routing` defaults to `true`, including when older settings omit the
key. A legacy `implementer` provider alone is a preference, not a pinned model.
A nonempty legacy `implementer_model` remains a global provider/model constraint.
Set `automatic_routing: false` to constrain unpinned stages to the configured
implementer provider while still requiring a validated joint assignment.
Planner and architect bootstrap settings remain separate. With automatic routing,
an unpinned reviewer follows the other provider and uses an eligible strong registry
entry. A nonempty `reviewer_model` pins its configured reviewer provider/model;
disabling automatic routing also preserves the configured reviewer provider. Like other role settings,
these switches are engine settings; the model registry has its separate persisted
policy file.

Selection precedence is explicit:

1. A stage's user-authored `model_constraint` narrows choices first. It accepts
   `provider`, `model`, and/or `native_effort`. `null` clears it. This overrides
   the global implementation constraint, but cannot waive capability or review.
2. Otherwise a nonempty `implementer_model` pins that model and its `implementer`
   provider. With no pinned model, disabling automatic routing pins the provider.
3. Otherwise the validated agreed assignment determines provider, model and
   native effort. Existing provider settings serve as preferences/defaults.

The plan editor exposes exact constraint fields. HTTP clients may include, for
example, `"model_constraint":{"provider":"codex","model":"your-exact-id"}`
on a pending stage in `POST /api/plan/edit`. Constraints survive AI revision;
agent output cannot manufacture user overrides or committed records. Invalid
IDs, efforts, inadequate tiers and cross-provider reviewer conflicts are reported
before approval rather than silently overriding settings. The independent
reviewer must use the other provider relative to the selected implementer.

The planner proposes risk, complexity, task, provider/model, native effort and a
stage-specific rationale in its existing standard/refactor/revision output.
The persistent architect independently evaluates cross-stage constraints and
failure impact in its guidance turn. Both must explicitly agree on classification
and selection. Missing proposals from manual edits or legacy plans are batched
into one strong planner turn. Disagreement allows one further planner/architect
exchange for only the disputed stages, then blocks with the architect's reasons
and correction instructions. A planner proposal alone never supplies architect
approval. Malformed output or a policy violation also blocks publication.

The engine's minimum policy is `stage-routing-1`. Critical or complex stages,
including concurrency, persistence and security implementation, require the
strongest suitable eligible tier, `strong`. A conservative implementation-text
check also protects sensitive persistence/security/concurrency work from both
participants underclassifying it. Explanatory prose about existing behavior is
exempt from that text heuristic when explicitly classified as documentation;
contracts, normative requirements and implementation work are not. Standard work requires `standard` or `strong`;
simple work permits `basic` or higher. Unclassified models cannot meet these
requirements. Tiers express configured adequacy, never quality inferred from
price, provider, name or list order. The planner and architect must still verify
that the selected option is suitable for the specific work.

Simple functionality and documentation prefer a cheaper adequate option when
both options have explicitly configured `relative_cost_preference` values, or
when published prices have comparable currency, unit and billing basis and one
option is no more expensive for both input and output (and cheaper for at least
one). Configured preferences take precedence over published prices. Set
`routing_billing_basis` to the exact published basis only when it applies to your
execution billing; its default is `null`, because API list rates do not establish
CLI subscription cost. Missing prices stay unknown. Different currencies, units,
bases, or input/output tradeoffs do not establish a cheaper option. Neither price
nor preference can make an inadequate tier acceptable. Every model has the same
acceptance checks and review gate regardless of cost.

Agreements retain both reasons, distinct proposal identities, explicit agreement,
bootstrap provenance, relevant goal/stage/dependency/constraint inputs, an input
fingerprint, resolved model and native effort, configured capability facts and
billing provenance. Previous checkpoints and events retain superseded agreements;
committed stage records remain unchanged. Explicit `depends_on` lists let an
independent edit affect only its own pending stages; absent lists conservatively
mean all earlier stages. Goal or applicable constraint changes reconcile affected
pending stages before manual/queue approval or legacy execution. Approval,
unchanged starts, ordinary fixes, restart, unrelated revisions and catalogue
clock/revision-only changes reuse agreements without selection calls.

Before every implementation/fix invocation, a local check verifies the relevant
saved inputs, chosen option and policy facts. Unrelated catalogue metadata and
availability becoming verified do not invalidate the choice. Material capability, supported effort, alias resolution or removal changes trigger
bounded reassessment at the next safe turn boundary. Cosmetic metadata, price,
provenance and catalogue clock changes do not reopen an existing agreement. Invocations record
proposed, requested and provider-reported effective models, including unexpected
substitution. Every handoff includes the saved architecture summary, decisions,
guidance, constraints, completed interfaces, outstanding findings and worktree/
diff context, and directs a replacement agent to inspect and preserve partial work.

Collapsed stage cards show the model/effort, availability verification, tier
provenance and both rationales before approval and during execution. Expand a
stage for classification, constraints, cost qualification and invocation details.
Planning/revision failure retains the previous published plan and context as one
atomic unit. Read-only plan Q&A never selects models or publishes architectural
changes.


### Bounded reassessment and recovery

The engine reuses each agreement until there is concrete evidence: changed stage
scope, risk or architectural constraints; the same unresolved role-attributed
review request or implementer test failure repeated at the threshold; a validated
implementer escalation request; measured provider input-token pressure against a
known configured `limits.context_window`; a relevant capability/alias/removal
change; or an operational provider failure that requires another assignment.
Unknown context limits produce no pressure trigger. Input-token usage is a
conservative per-invocation pressure signal, not a claim about exact remaining
context or cost. Selection is never run just because a stage, review, retry or
metadata refresh occurred. Review clarification and a triggered selection share
one necessary architect turn where possible; valid guidance and checkpoints are
reused. Every replacement must inspect and preserve inherited partial work.

`POST /api/settings` accepts `reassessment_limits` (all four keys required):

```json
{"reassessment_limits":{"max_reassessments":3,"max_operational_retries":2,"repeat_threshold":2,"context_percent":85}}
```

Limits are copied into each attempt. Reassessments allow 0–8, operational retries
0–5, repeated failures 2–10, and context pressure 50–95 percent. Defaults allow
three evaluations and two operational retries total per attempt. A trigger
signature can reserve only one evaluation. Reservations are persisted **before**
paid selection or retry backoff. Transient rate limits, overload and timeouts
receive exponential backoff (1, 2, 4, 8, 16 seconds if the retry budget allows),
with stop checks every 25 ms. Authentication failures receive no paid retries;
Forge either agrees an eligible alternative or blocks with an actionable error.
Tool/process errors are recorded separately from reasoning failures and do not
receive transient retries. Independent review uses the same shared retry budget
and fails closed if its provider cannot supply a valid review.

For reasoning failures, a supported higher native effort on the same adequate
model is preferred when the previous effort is explicit. Provider-default effort
is unknown and is not treated as a known lower level. Otherwise selection must
choose a stronger suitable configured capability tier. Effort cannot replace
required capability. Explicit user constraints, the agreed capability floor and
other-provider review eligibility remain mandatory. Retired assignments cannot
be revisited; an unchanged assignment is allowed only when revalidating material
scope/policy changes. No assignment changes while a provider turn is running.
Provider-reported model identity and native effort eligibility are checked; a
missing or unexpected effective model report blocks with work retained.

Implementers return an engine-owned JSON outcome on their final output channel.
Forge supplies and validates the exact plan, stage, attempt and unique turn IDs,
status (`completed`, `test_failure`, `escalation`), bounded evidence and optional
request (`kind`, `reason`, `required_capability`). Unknown fields and stale IDs
are rejected. Legacy prose is accepted as ordinary completion and cannot request
an escalation. Agents must never write routing or outcome data into `.forge`.

A stop preserves the worktree, unresolved requests and last committed architect
checkpoint. Restart retains spent fix rounds, retry/evaluation counts and trigger
signatures. An interrupted or failed selection reservation blocks automatic replay,
so repeated restarts cannot create paid selection loops. Correct the reported
provider/configuration issue; for a blocked selection or effective-model mismatch,
revise the affected stage scope/constraint and reconcile explicitly to start a new
attempt. Switching models never resets `max_fix_rounds`. New goals and projects
have separate identities and budgets.

The state API exposes attempt status, limits, counters, pending trigger evidence
and the latest four history entries (full history stays in the saved plan/events).
Stage cards show retry/escalation/blocked status, the trigger, and both planner and
architect reasons. History retains old/new agreements and invocations retain actual
model reports and role token usage. Unknown prices remain unknown.

### Default configuration and offline validation

The shipped registry is empty. Before live planning, configure at least one
eligible strong model for each provider used by planner/architect and independent
review, then add cheaper adequate entries for simpler stages. This example uses
placeholder IDs, not verified provider capability claims:

```json
{
  "policy_revision": "my-routing-1",
  "entries": [
    {"provider":"codex", "model":"your-strong-codex-id", "tier":"strong",
     "effort":"provider_default", "relative_cost_preference":5},
    {"provider":"codex", "model":"your-small-codex-id", "tier":"standard",
     "effort":"provider_default", "relative_cost_preference":1},
    {"provider":"claude", "model":"your-strong-claude-id", "tier":"strong",
     "effort":"provider_default", "relative_cost_preference":5}
  ]
}
```

`standard` permits ordinary functionality and simpler tasks; `basic` is sufficient
only for simple tasks, and `strong` is required for critical/complex work. These
are your configured judgments. A lower relative preference chooses among adequate
options even when every price is unknown. An absent preference establishes no cost
ordering. Published prices are used for comparison only with matching explicit
`routing_billing_basis`; its default `null` leaves API list rates informational.

The default providers are Claude for planning, Codex for architecture and the
implementation preference, and Claude for independent review. Role model strings
start empty; bootstrap resolves eligible configured strong entries. Automatic
routing defaults on, stage overrides take precedence over global model pins, and
adequacy/review requirements always apply. Only `model_catalogue` policy is saved
to the user configuration file; other engine settings are process settings.
`auto_push` defaults to `true` (disable it for local-only runs), and
`queue_auto_approve` defaults to `false`. This stage does not change Git behavior.

Discovery runs at startup and every 360 minutes; metadata is first considered
after discovery and every 1440 minutes, with a 168-hour TTL. The scheduler ticks
every 30 seconds. Missing/unsupported official documents use a 1-hour negative
cache/backoff doubling to 24 hours. Only the documented official HTTPS sources
and accepted document format are supported; unavailable metadata remains unknown.
`curl` is needed for live official research; the optional Claude bridge needs its
separately provisioned Agent SDK. Neither is required for offline fixture tests.
An unsupported discovery mechanism allows explicit configured unverified choices;
missing executables, authentication failures and known rejected choices block.

One disagreement reconciliation exchange is allowed. Each stage attempt defaults
to three reassessments and two transient operational retries; repeat failures
trigger at two and measured context pressure at 85%. Three additional fix rounds
are allowed after the initial review. All reservations survive restart. Existing
agreements are reused at approval/execution unless relevant inputs change; a
restart alone neither reselects models nor refreshes the review budget. Recovery
uses the authoritative plan/checkpoint and pending-session marker described above.

Validation from this checkout uses no provider credentials or network:

```sh
cargo build --offline
cargo test --offline
node --test tests/*.test.mjs bridges/claude-models/discovery.test.mjs
omarchy plugin validate "$PWD"
/usr/lib/qt6/bin/qmlformat quickshell/Panel.qml > /dev/null
/usr/lib/qt6/bin/qmlformat quickshell/BarWidget.qml > /dev/null
```

Use an already installed Node binary if a version-manager shim has no selected
version. Qt tool locations depend on the distribution. QML parsing, JavaScript
rendering tests and plugin manifest validation pass in the stage-8 environment.
Standalone `qmllint` cannot fully resolve the runtime `qs.Commons`/`qs.Ui` imports
and reports the resulting unresolved widget types, plus an existing `enabled`
property shadow warning. Live shell rendering is not validated because that would
require loading changes into the running Omarchy instance.

The deterministic lifecycle fixture joins startup discovery, unknown pricing,
configured unverified execution, conditional periodic official refresh, joint
draft agreement, approval reuse, cheap documentation/functionality, interrupted
review and process reconstruction, evidenced escalation/context handoff, current
dual gates, commits, durable reports and a fresh critical queued plan. It asserts
selection and review call counts. Companion fixtures cover transaction failures,
provider switching, disagreement bounds, negative caching, legacy editing,
project isolation and unchanged acceptance/build/test requirements. Model output
and check evidence are simulated at injected provider boundaries; the Forge build
and full test suite run normally. HTTP fixtures bind ephemeral loopback ports.
Do not use the live port 8734 for a smoke instance; use `FORGE_PORT=18734` and an
isolated project/configuration. Validation needs no installation, instance restart,
push, or paid generation.
