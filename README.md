# Forge

A minimal wrapper around AI coding agents (Claude Code, Codex CLI) for
building software — including Forge itself — with almost no ceremony.

Rust engine + Quickshell (Omarchy) panel.

A feature-spec planning workflow is documented in [docs/features/](docs/features/README.md); milestones M1 to M4 are implemented by the engine, and M5 remains planned.

## The loop

1. Point Forge at a git repository and describe a goal.
2. A planner returns a staged plan, and a persistent architect supplies
   architectural context and stage guidance. Forge validates their output and
   publishes `.forge/plan.json` — each stage has instructions, acceptance
   criteria, and a proposed commit message.
3. You mark the plan OK in the panel.
4. Forge runs each stage automatically:
   - the **implementer** implements the stage and leaves it uncommitted. The
     implementer must fix every build error and failing test before handing off,
     regardless of which change caused it; such failures are never out of scope
     and must never be reported as pre-existing or as blockers instead of being
     fixed. If git history changed meanwhile, the engine changes nothing itself and asks
     the architect to decide (see [Git history changes during a stage](#git-history-changes-during-a-stage)),
   - the engine classifies the full implementation snapshot,
   - a fresh, adversarial **independent reviewer** verifies the stage and its scope
     when scheduled per stage,
   - the persistent **architect** also reviews code and architectural contracts
     when scheduled per stage,
   - requested edits return to the implementer, followed by the stage-required reviews again,
   - the engine commits the exact gated tree locally under an approved or deferred gate.
5. After the last stage, Forge runs any deferred reviews over the plan's commit
   range and fixes their findings. The plan fixer must fix every build error and
   failing test found by review, regardless of which change caused it; such
   failures are never out of scope and must not be reported as pre-existing or
   as blockers instead of being fixed. Only after that gate is clean and any approved
   fixes are committed does it attempt a push to `origin` when `auto_push` is
   enabled and append the completed-run report. Plans without deferred roles
   skip this phase. Push failures are logged; report-storage failures are returned
   for recovery.

### Scope and review authority

Scope policy version 1 belongs to the engine. Ordinary documentation requires
the fresh independent reviewer; **architect review is not required**. The engine
records the committed outcome in the architectural checkpoint so subsequent
stages retain context without an architect approval or summary turn.

`review_cadence` sets when each required role reviews:
`{"architect":"per_plan","reviewer":"per_plan"}` is the default. Either role
can independently be set back to `per_stage`. The engine's scope policy still
decides which roles are required at all; cadence only schedules those required
reviews. Each fresh stage attempt durably captures both normalized cadence
values alongside its review budget. Changing settings
mid-run does not alter an in-flight attempt's gate, including on resume. Captured
attempts retain their recorded values, including `per_plan`. Missing, legacy or
unknown cadence values behave as `per_stage`.

The recorded policy keeps `required_roles` and partitions it into
`stage_required_roles` and `deferred_roles`. A required role with `per_plan`
cadence receives no stage review invocation; its stage gate outcome is `deferred`,
never an approval or `not_required`. Under the default, every required role is
deferred, so the stage commits locally after implementation with a `deferred`
gate and no stage review or fix call. With mixed cadence, the stage-required
roles must approve before that local commit; the other roles remain deferred.
These recorded obligations survive settings changes and must be discharged by
plan review.
Deferring architect review does not disable architectural guidance or routing.

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
must inspect actual behavior and confirm scope when invoked. During stage review,
it promotes suspected architectural impact with `requires_dual`, explaining why
in `scope_reason`.
Promotion records a new policy and requires new stage-required verdicts bound to
that policy, retaining any architect deferral; previous verdicts remain historical.
Stage scope is checked after each fix and before commit. Once dual review is
required, it remains required for the attempt.

Roles reviewing in the same round inspect the same HEAD, index and working
content before a fixer runs. Reviews are sequential because the engine has one
active-agent/log state.
The architect checks recorded decisions, cross-stage interfaces and regressions.
The independent reviewer receives agreed constraints and preceding actionable
requests, never the architect's current approval as an endorsement. The engine
uses the reviewer provider selected in the panel, in a fresh read-only session.
The separate `auto (other provider)` option requires a different provider from
the implementer. Known unavailable models or
incompatible effort/permission capabilities block execution. Configured explicit
unverified fallbacks remain visibly unverified until execution verifies them.
Stage routing selects a tier; launch resolves a model within the selected implementer provider; the review gate enforces the
same requirements for every selected model.

Each role retains its authority. The aggregate keeps requests with `[architect]`
or `[reviewer]` provenance. A reported `architecture_context_gap` requests a
persistent architectural clarification and records its guidance/decisions;
it does not erase either role's unresolved requests. Fixes must satisfy both
roles wherever dual review applies. Clean first-round results need no fixer.

After all stages commit, plan review runs the union of their recorded deferred
roles. It freezes the goal, revision, ordered inputs and commit references of
all completed stages, implementation provider provenance, required roles, base,
anchored HEAD, attempt identity and budget. The base is the `attempt_head` of
the earliest stage with deferred obligations, falling back to that stage commit's
parent, and must be an ancestor of HEAD. Reviewers inspect
`git log --stat <base>..HEAD` and `git diff <base>..HEAD` together with staged,
unstaged and untracked changes. All completed stages supply acceptance and
integration context, including earlier stages outside that range. The combined
acceptance text preserves plan order and prefixes each non-empty criterion line
with `stage <id> (<title>): `; every item needs its own evidence. The frozen
subject is revalidated after reloads and before finalization; moved HEAD or
changed inputs block continuation under the old subject.

Plan review uses the same exact identity, snapshot, evidence normalization,
read-only sandbox and independent project-check rules described below. Its
identity has `stage_id: null` and `scope: "plan"`; each role's validated verdict
is published as a `plan_review` event in the architecture event log. The architect
uses its persistent session. In `auto (other provider)` mode, the independent plan
reviewer must use a provider no stage implementer used, including recorded implementation/fixer invocation
provenance and plan fixes; retries and fallback cannot waive this exclusion.
Unknown or unverified implementation provenance blocks independent plan review.
An explicitly selected reviewer may use the same provider in a fresh session;
provider provenance and all evidence, sandbox and approval checks remain required.
In automatic other-provider mode, if no
eligible independent provider remains, the run blocks and commits stay local,
with guidance to use reviewer cadence per stage for a revised plan or pin the
implementer provider. These choices can prevent conflicts in future work;
changing current settings or approving a revision cannot erase committed stages'
existing deferrals or provider history.

### Shared response correction

Every structured agent response uses one engine correction policy: validate the
response, return the concrete validation error and rejected response to its author,
and allow at most **three corrections after the initial response**. Changing error
kinds does not reset the budget. Parsing and response-field validation share that
budget. A valid response needs no extra call; exhausting the budget retains the
previous published state and reports the last validation error.

Every response parser rejects duplicate JSON fields, including nested fields,
before converting the response into an operation-specific contract. The same
correction budget covers the following checks before their results are used:

| Response | Checks inside the correction boundary |
| --- | --- |
| Plan generation and revision | Valid stages and dependencies, proposal fields, and the minimum capability tier for the proposed work |
| Routing selection | Exact proposal/evaluation IDs, complete fields, planning capability floor, and consistent explicit agreement |
| Architect guidance | Plan/revision identity, required guidance, retained interfaces, constraints retained or explicitly retired, valid decisions and explicit risk resolution |
| Scope response | Exactly one revision or refusal, valid revision fields, and an actual change or a concrete clarification |
| Review | Review identity, consistent approval, acceptance/check evidence, and actionable rejection details |
| Implementer/fixer outcome | Non-empty response, structured outcome identity/status/evidence, and a request consistent with its status |
| Plan chat, goal enhancement and model policy | Required fields and types, supported fields, and valid model assignments |

An underpowered planning tier returns to the planner before calling the architect.
A review rejection without actionable details returns to its author instead of
receiving an engine-invented issue. A scope answer containing both `revised` and
`refused` is rejected without choosing either branch. Corrections may preserve a
valid refusal; validation never requires a model to approve something it rejects.
Existing documented normalization of optional fields and legacy completion prose
remains supported. Empty outcomes, arrays and malformed fenced outcomes cannot
bypass structured validation as legacy completion prose.

This applies to initial plans and revisions, plan Q&A, goal enhancement, scope
answers, routing proposals and evaluations, architectural guidance, stage and plan
review verdicts, and structured implementer/fixer stage outcomes. Planner proposal
schemas are checked before calling the architect, including nested unknown fields.
An architect evaluation with `agree: true` must match the planner's risk,
complexity and task fields. A mismatch returns the exact conflicting fields to
the architect through the same three-correction budget, both in initial guidance
and after a routing reconciliation. The architect can correct the fields or
explicitly disagree with `agree: false`; the engine never invents agreement.
Corrections keep the selected provider/model and operation context. Architectural
corrections retain the exact session; independent reviews remain fresh and inspect
the same snapshot. Correcting an implementer's report uses a read-only invocation
and does not repeat implementation. Successful operations account for every measured
response invocation, including corrections.

The shared policy lives in `src/response.rs`. New response operations must supply
one complete, side-effect-free validator and a correction invocation to this policy;
publication and other effects run only after validation succeeds. A valid rejection
or scope clarification retains its meaning and follows the existing review/fix or
scope workflow. Provider failures, cancellation, changed repository/model/session
identities and storage failures are not response corrections. Their existing failure
and recovery paths remain responsible; the engine does not replay writes, commits or
pushes to repair JSON. Review fix budgets, scope renegotiation limits, constraint-conflict
escalation budgets and routing reassessment budgets are separate and are never reset by
response correction.

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
- `constraint_conflict` (optional, non-empty when present): a verdict field stating
  which of the stage's own constraints cannot all be met together. Distinct from
  `architecture_context_gap`, which records role disagreement and routes to the architect.
  A reported constraint conflict routes to the planner for one revision attempt;
- `criteria`, with each supplied criterion, its status and concrete evidence;
- `acceptance_evidence`, echoing the complete acceptance text, verification and evidence;
- `project_checks`, recording exact commands, passed/failed/unavailable status,
  and actual output or evidence establishing that a check does not exist.

Every acceptance criterion must be individually verified. The independent
reviewer must run all available required project builds/tests, including for
documentation. Reviewing agents discover the project-specific requirements, run
the appropriate commands, and verify their results. The engine validates the
structure of their evidence without matching command names or shell syntax.
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

The engine writes the current role identity to `.forge/review-identity.json`
before each review and asks the reviewer to load it with a script when assembling
the final JSON. The file is read-only during review; the returned identity still
must match exactly. A prose preamble or a JSON Markdown fence is accepted, while
duplicate keys, multiple JSON candidates and trailing commentary are rejected.

Codex reviews enable `sandbox_workspace_write.network_access` so scratch tests
can bind ephemeral loopback ports. The outer Bubblewrap filesystem restrictions
remain in force, matching the host network access already available to Claude
reviews. This setting applies only to review roles.

### Budgets, commits and history

`max_fix_rounds` (default `3`) means extra fix/review rounds after the initial
round: at most four implementation/fix rounds by default, each with its
stage-required roles. A scope promotion can require a fresh independent verdict
in the same round under the new policy. Rounds are reserved durably before calls.
Stops, errors, partial paired-review failure and process restarts consume the reserved
round and never reset the saved budget or reuse approval. Exhaustion blocks the
stage. An approved plan revision starts a new attempt; changing a setting or
pressing Run again does not replenish the current attempt.

Plan review captures its own `max_fix_rounds` budget when the phase starts:
one initial review plus at most that many corrective fix/review cycles, so `0`
allows one review and no fixer, and the default `3` allows at most four review
rounds and three corrective cycles. Each round is durably reserved before any
invocation. Saved action boundaries distinguish work not yet launched from an
interrupted invocation: an in-flight fixer or review is not replayed under its
old round identity. Stops, errors and restarts retain consumed rounds, and may
leave fewer corrective cycles available; neither Run nor a settings change
replenishes the attempt. Exhaustion blocks with an `exhausted` gate. Editing and
approving a new plan revision starts a fresh plan-review attempt while retaining
historical verdicts and implementation provenance. This does not discharge the
recorded deferred obligations.

Plan fixes receive the goal, frozen stage text, commit range, role-tagged
requests and architectural guidance/constraints. The fixer edits only the
working tree, without committing or rewriting history. Its model uses the shared
resolver, capability requirements, invocation accounting, operational retries
and quota fallback. When a fix round changes the tree, the engine commits it as
`fix(review): apply round <n> plan review findings`, with the addressed
role-tagged requests in the message body, and advances the anchored HEAD. Each
round is therefore reviewed again by all required plan roles from a clean working
tree, and `plan_review.fixes` records every round's commit, addressed requests and
files. A round that changes nothing makes no commit. Because the engine commits
each round, a review may never ask an agent to commit; uncommitted content it sees
is content no fix round has committed yet.

A fix round that commits nothing while the outstanding requests are unchanged from
the previous such round stalls the review: the gate becomes `stalled`, the phase
blocks and the remaining budget is not spent re-reviewing identical content. That
signals requests that no working-tree edit can satisfy, such as a saved
architectural constraint to reconcile. Before blocking, Forge hands the stalled or
exhausted plan review to the planner once per conflict signature for a corrected plan.

A stage that exhausts its review budget, or any stage or plan review where a role reports
a `constraint_conflict` verdict or request, is handed to the planner once per conflict
signature. The planner receives the complete context (plan, stage text, role requests,
implementer/fixer evidence, check results, changed files, round usage) and returns one of
three decisions: `revise` (apply through plan publication; current-stage-only revisions
keep approval and restart the stage; later-stage changes, insertions or plan-review scope
changes return the plan to draft), `constraint_wrong` (corrected stage text; stage keeps
approval and is reviewed again), or `refused` (explanation passed to next implementer/fixer
round if budget remains, otherwise blocks). A reported constraint conflict skips the
repeated-findings model reassessment that round. The same conflict after an applied
correction blocks with the escalation record kept, bounding escalation to one planner pass
per signature per stage or plan-review lineage.

Only a clean evidenced plan gate can authorize the remaining
`fix(review): apply deferred plan review findings` commit of the approved tree,
which covers content the review saw that no fix round committed; if that tree
already equals HEAD's tree, there is no extra commit. Rejected or exhausted review
keeps the fix commits already made but prevents push and run-report publication.
Stops retain work and return to `plan_ready`; other phase failures block.
Successful finalization is saved and validated before `auto_push` or the
completed-run report. Both commit paths record their intended parent, tree and
deterministic message before moving HEAD, so recovery adopts exactly the commit
that landed and can never create a duplicate or approve new work.

Stage commit validation checks current stage-required verdicts, identity, policy
and the unchanged snapshot. After staging, the actual index tree must equal the
reviewed normalized tree, with unchanged worktree content and HEAD. The engine creates
that exact tree's commit with `git commit-tree` and advances HEAD with an expected
old-value `git update-ref`; concurrent HEAD changes are rejected. This path does
not run commit hooks, so required project checks must be evidenced during review.
Successful commits add an engine checkpoint outcome without modifying reviewed
files. The final content check happens immediately before the ref update;
external tools should not edit a repository during an active stage.

Deferred stage gates retain the same identity, persisted-plan, policy partition,
snapshot and tree checks, with evidence required for each stage-required role;
a `deferred` aggregate is valid only with no stage-required roles and non-empty
deferred obligations. Plan fix commits use the same staged-tree equality,
`commit-tree` and compare-and-swap `update-ref` protocol, revalidating the frozen
subject and every required verdict before committing.

Immutable role records accumulate in `reviews`; `last_verdict` remains the
latest independent verdict for compatibility. `review_policy` and the distinct
`review_gate` expose current policy/reason, each role's outcome, aggregate status
and actionable requests. An architect outcome of `not_required` explicitly means
**architect review not required**, never approval. Historical approval is shown
separately from current pending, deferred, blocked, error, interrupted or
invalidated gates.
Legacy note-only approvals keep their historical rendering; incoming notes never
permit a current clean gate. Full feedback remains in paginated review history.

### Git history changes during a stage

Implementers and fixers must leave their work uncommitted: their prompts and, for
Claude, the appended system prompt say so and override instruction files that ask
for commits. When HEAD nevertheless differs from the stage's base after such an
agent finishes (or from the captured HEAD after a plan review fixer), the engine
does not touch history. It collects evidence — the new commits, the files they
change, the uncommitted work, their overlap and whether history was rewritten —
and asks the persistent architect for exactly one action:

- `uncommit`: the commits hold this work. The engine moves HEAD back to the base,
  keeps their content staged, and the normal review and engine commit follow.
- `continue`: the commits are an external change that leaves this work alone.
  They stay in history and the stage (or plan review) continues on the new HEAD.
  The engine rejects this unless uncommitted work exists and no file overlaps.
- `block`: anything else. The stage (or plan review) stops with the architect's
  reason and history stays as it is.

Rewritten history allows only `block`, and a failed or invalid architect answer
blocks too. Each outcome is stored in `history_decisions` on the stage or
`plan_review`, and the architect's choice is recorded as an accepted decision
in the plan's architecture history.

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

### Improving the goal description

Type a rough description in the goal field and press **Enhance with AI**.
The configured planner tool reads the repository read-only and returns a
rewritten description, with instructions to preserve your intent, invent no
requirements, and neither split the work into stages nor implement changes.
If the field still matches the text submitted, the rewrite replaces it
automatically. If you edited the field while waiting, the rewrite waits behind
**Apply AI description**. **Undo enhance** restores the text that was in the
field immediately before the rewrite was applied. A failed enhancement leaves
the field text unchanged and reports its error.

Enhancement never modifies the plan, chat transcript or repository source
content. Forge still records operational activity. When the provider reports
token usage, it appears in the live feed and session role totals under `enhance`;
like Plan Q&A, it is not added to plan totals.

The JSON API accepts `POST /api/goal/enhance` with `{"goal":"…"}` and returns
HTTP 200 `{"ok":true,"request_id":n}`, with an increasing request ID per
accepted request in that project session. No existing plan is required. The
usual optional `project` field targets another project. Input is trimmed and
limited to 20000 characters: blank, missing or non-string goals return HTTP 400
`{"error":"goal required"}`, and over-long goals return HTTP 400
`{"error":"goal too long"}`. A busy project or active queue returns HTTP 409
`{"error":"busy"}`.

`GET /api/state` exposes the targeted session's result under `goal_enhancement`:
initially `null`, then an object with `status` (`running`, `ready` or `failed`),
`request_id`, `original` (the trimmed input) and `unix` (a Unix timestamp).
A `ready` result adds `goal` (the trimmed rewrite); a `failed` result adds `error`.

### Discussing before planning

Before creating a plan, you can have a multi-turn conversation with the configured
planner tool about the repository and possible solutions. Click the **Discuss before
planning** entry point on the Overview tab (or press `t`) to open a dedicated chat view showing the full
discussion. The chat replaces the visible panel content while it is open: the header, tab bar,
the current view and the keyboard hint are all hidden, and the
chat view alone fills the window. Type a message into the composer and click
**Send**, or press `Enter`.
The planner reads the repository read-only and replies in a normal back-and-forth
conversation. The chat view shows the conversation in a scrollable transcript that
takes the full height freed by hiding the rest of the panel, reading top to bottom
with the newest message at the bottom; below it sit the status line, the composer and
the **Send**, **Create plan from discussion** and **Clear discussion** actions. Your
messages are right-aligned with an accent tint, and Forge's replies are left-aligned
in a neutral surface color. The view displays a clearly visible **← Back** return
control at the top; press it, `Escape`, or `q` to close the chat and restore the full
panel with its previous state intact. While the chat is closed, a one-line status
beside the entry point shows any pending reply or error.

While a reply is pending, your message and a "Forge is replying…" indicator appear
at the bottom of the chat. A failed reply shows the error in the status line and puts
your message back into the composer for revision. The chat scrolls to new messages
when they arrive, unless you have scrolled up to read earlier ones. Each message has
a Copy button in its header, and you can select and copy the text directly. Tab moves
focus between messages. In the composer, `Enter` sends the message and `Shift+Enter`
inserts a newline; `Escape` leaves the field (a second `Escape` closes the chat).
Press `i` to return focus to the composer after leaving it. Each new reply includes
all prior messages, so the discussion can refine and settle on an approach.

Sending discussion messages never starts plan creation. When you are ready to
plan from the conversation, click **Create plan from discussion** (or press `P`)
to begin planning. Forge passes the entire discussion transcript plus any optional
note from the goal field to the planner, which produces a plan reflecting the
refined ideas settled on during the discussion, not just the first message.

**Clear discussion** resets the conversation without affecting any existing plan.
The transcript lives in `.forge/discussion.jsonl` and survives plan generation and
plan reset; it is removed only by the explicit **Clear discussion** action. The
last 100 discussion entries are shown and used for new replies. Token usage is
attributed to the `chat` role in session totals, similar to Plan Q&A.

Discussion never modifies the plan, goal, phase, plan-candidate file, Plan Q&A
transcript or repository source content. A failed reply leaves the transcript
bytes untouched and reports the error.

The JSON API accepts `POST /api/discussion/message` with `{"message":"…"}` and
returns HTTP 200 `{"ok":true,"request_id":n}`. Input is trimmed and limited to
20000 characters: blank, missing or non-string messages return HTTP 400
`{"error":"message required"}`, and over-long messages return HTTP 400
`{"error":"message too long"}`. A busy project or active queue returns HTTP 409
`{"error":"busy"}`. The usual optional `project` field targets another project.

`POST /api/discussion/reset` clears the transcript and returns HTTP 200
`{"ok":true}`. When the project is busy or its queue is active, it returns HTTP
409 `{"error":"busy"}`. It does not touch the plan.

`GET /api/state` exposes the targeted session's transcript under `discussion`:
initially an empty array, then an array of up to 100 recent entries, each with
`role` ("user" or "assistant"), `text` and `unix` (a Unix timestamp). The
`discussion_activity` field is initially `null`, then an object with `status`
(`running`, `ready` or `failed`), `request_id` and `unix`. A `running` request
adds `message` (the original trimmed input). A `ready` result has no additional
fields. A `failed` result adds `message` (the original trimmed input) and
`error`.

`POST /api/plan` with `{"discussion":true}` (and optional `goal` for a note)
starts planning from the discussion. Non-boolean `discussion` values return HTTP
400 `{"error":"discussion must be a boolean"}`. Combining `discussion: true` with
refactor mode returns HTTP 400 `{"error":"discussion planning requires standard
mode"}`. When the transcript lacks a user message followed by an assistant reply,
it returns HTTP 400 `{"error":"no discussion"}`. These validations occur before
the busy check and do not claim busy. A busy project or active queue returns HTTP
409 `{"error":"busy"}`. Omitting the `discussion` field keeps the existing
direct goal-to-plan behaviour.

### Editing the plan

After the planner writes a draft, use **Edit plan** in the panel to repair
it by hand: edit each stage's title, instructions, acceptance criteria,
and proposed commit message, or add, remove, and reorder stages. Committed
stages are locked. Click **Save** to keep your changes or **Cancel** to
discard them.

To ask AI for changes, type feedback into the field beside **Improve with
AI**, then click the button or press `Enter`. Forge re-runs the planner
against the current plan and your feedback, keeping the same overall goal.
The planner's prompt only receives the plan's content — goal, stages,
instructions, acceptance criteria, commit messages, status and explicit
dependencies — never model selection, agreement, proposal-input,
reassessment, or invocation records; the engine still revises against the
complete saved plan internally, so committed stages, saved model constraints
and other bookkeeping are preserved exactly as before. A failed AI revision
keeps the previous plan.

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

Guidance and agreement records store a `relevant_inputs` fingerprint, a copy of
the stage text that grows with every transitive dependency. Storage and every
validity comparison keep it, but no provider prompt contains it: one shared
prompt view (`src/prompt_view.rs`) removes it from the architect, planner chat,
routing, stage review, plan review and plan-fix prompts. Planner chat receives
that compact view of the saved checkpoint instead of the raw record, and the
routing reconciliation prompt carries the checkpoint once. The view is a clone,
so stored checkpoints and events are unchanged.

Per-plan artefacts live under `.forge/architecture/<plan-id>/`:

- `events.jsonl`: serialized append-only committed events for decisions,
  guidance, model proposals/agreements/selections/invalidations, and role-tagged
  reviews. Event envelopes include version, ID, plan, revision, timestamp, and
  checkpoint reference. Imported legacy details remain in immutable review files.
- `checkpoints/<checkpoint-id>.json`: immutable bundles containing the exact
  published plan and a bounded architecture checkpoint. The checkpoint holds
  the exact provider session and provider checkpoint reference, context summary,
  up to eight recent decision summaries, current guidance/agreements, and review
  policy with its rationale. Stored and expanded architecture checkpoint data is limited to 4 MiB;
  execution loads restore the original review arrays and historical metadata.
- `reviews/<file-id>.jsonl` and `.idx`: immutable review arrays and binary
  little-endian u64 record offsets. The plan's `architecture.review_history`
  manifest binds stage IDs to exact files, counts, byte lengths, and at most eight
  previews. On disk, a stage's reviews use `{"$forge_reviews":"<file-id>"}`;
  execution and editing hydrate them back to the original JSON arrays. Unchanged
  arrays reuse files, and removed stages retain their manifest entries.
- `archived.json`: the last published plan reference, written before replacement
  or discard so its committed history remains addressable.

Checkpoints and history events larger than 64 KiB can use a lossless
`{"$forge_compact":1,"strings":[...],"value":...}` storage envelope. String values
are stored once, so long stage descriptions repeated in transitive dependencies,
guidance and model agreements do not multiply the stored size. Tagged object and
string nodes preserve arbitrary user metadata without reference collisions.
History events retain a 64 KiB stored limit (including the newline). Whole-plan
checkpoints use the existing 4 MiB expanded-record budget for storage as well;
64 KiB is only a compaction threshold for checkpoints. Compression is used when
it reduces size, while unique checkpoint facts can remain plain JSON. Expanded
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

When the phase exists, `GET /api/state` exposes `plan.plan_review` as a bounded
projection with status, attempt ID, base and anchored HEAD, `rounds`, captured
`budget`, `required_roles`, optional `fix_sha`, usage and the gate's status, per-role
outcomes, role-tagged requests and identity. It includes at most eight recent
fix-round previews with `fix_count` and `fixes_truncated`, each keeping its round,
SHA and message subject while bounding the addressed requests and file list. It
includes at most eight recent review previews with `review_count` and
`reviews_truncated`; individual previews
are explicitly shortened even for small verdicts. Auxiliary fields and strings
are bounded too, with flags such as `requests_truncated` and
`identity_truncated` on the gate. Polling uses stored projections without reading
or returning full plan verdicts. Complete records have a separate
`architecture.plan_review_history` reference for execution hydration and remain
available through the architecture event log; the per-stage `review_history`
manifest is unchanged. A plan without this phase omits `plan_review`.

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
identity. Plan-scope verdicts and their full identities are in history events
whose `payload.kind` is `plan_review`, within the same event/page size bounds;
they do not use a synthetic stage ID or the stage reviews endpoint. For example,
after a queued goal has completed and been replaced:

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
history entries. The saved plan itself also keeps only the four most recent
reassessment history entries per stage, each with a monotonic `seq`, plus the
true total in `history_count` and a `history_archived_through` watermark.
Publishing the plan moves older entries, complete and unmodified, into that
publication's architecture event as `archived_reassessment_history` (grouped by
stage; at most 32 entries or about 128 KiB per event, so an oversized history
drains over successive saves), where they stay fully readable through
`GET /api/architecture/history`. Republishing a stale copy never archives an
entry twice, and plans saved without these fields are numbered on first touch.
Reassessment limits and counters (`count`, `operational_retries`, signatures)
are unaffected. A stage's `model_proposal_inputs` is a small fingerprint
(`version`, serialized `bytes` and `digest`) of the goal, stage, dependency and
constraint inputs of its selection proposal, not a copy of them, so it stays
constant-sized as the plan grows. Full selection dialogue and transitive input
descriptions stay in durable artefacts; polling preserves the choice, both reasons, native effort,
policy provenance, current gates, editing content and token totals.
All these artefacts remain inside the existing `.forge` commit exclusion.

### Asking about the plan

Type a question about the current plan into the field beside **Plan Q&A**
on the Plan tab, then click **Ask** or press `Enter`. The selected planner tool
answers without modifying the plan. Its prompt receives only the plan's
content — goal, stages, instructions, acceptance criteria, commit messages,
status and explicit dependencies — not model selection, agreement,
proposal-input, reassessment or invocation records. It also carries the saved
architecture context as the compact prompt view (see
[Plan identity and architectural records](#plan-identity-and-architectural-records)),
never the raw checkpoint. Expand **Plan Q&A** to read the conversation. Asking requires an existing plan, with Forge idle, the
queue inactive, and plan editing closed.

This is separate from the **Discuss before planning** chat, which does not
require an existing plan and produces a new plan from the discussion rather than
answering questions about it.

The JSON API accepts POST requests to `/api/plan/chat` with
`{"question":"..."}`. Questions must not be blank. The transcript is stored
in `.forge/chat.jsonl` and cleared when new plan generation starts (including
an AI revision) or the plan is reset. The pre-planning discussion transcript
(`.forge/discussion.jsonl`) is not affected by plan operations.

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
  calls; the plan's top-level `usage` holds stage, architect and plan-review/fix totals.
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

When at least one stage captured cadence, the report adds
`review_cadence: {"version":1,"stage_attempts":[...]}`. Each entry, in plan order,
has `stage_id`, `attempt_id`, `revision` (the attempt revision) and `cadence`
(the recorded architect/reviewer pair). This preserves mixed cadences across
completed stage attempts. An older stage with no capture has `cadence: null`;
live settings are never substituted, and wholly legacy plans omit the field.
If plan review exists, `plan_review` records final gate `status`, `rounds` used,
per-role `roles`, `base`, `fix_sha` (null when no extra finalization commit was
needed), `fixes` (each round's `round` and `sha`), and its `usage` and `role_usage`
contribution when present. That contribution is already included in plan totals.
The existing `commits` and `stage_outcomes` remain stage lists; review fix commits
are identified by `plan_review.fixes` and `plan_review.fix_sha`. Reports
without plan review omit that object, and older reports remain readable.

Report appends are synced before a queue goal is marked complete. A storage
failure is reported and keeps that queue goal for recovery; commits are retained
locally, but a preceding enabled push may already have succeeded. Deferred review
must pass and its approved tree be durably finalized before either push or report
publication; a failed push is logged and does not prevent the report.
Full decisions and superseded agreements remain addressable through
architecture history using the report's plan ID. Reports summarize token usage;
they do not estimate CLI spend or convert API list rates into subscription costs.
The append log is not an exactly-once completion ledger: explicitly rerunning a
completed plan can append another report for the same plan/revision.

`GET /api/state` returns the last 100 reports for the project under `reports`
(an empty array when none exist). In the panel, open the **Activity** tab, choose **History** and select
**reports** to see completed tasks with their duration, commit count, and
token totals per tool. Expand a task to see commit SHAs and messages, input/output/total
counts, call counts, model totals, separate planner and role usage, architecture,
routing reasons and recorded review gates. Older reports without these fields
remain readable with the available fields. The reports
filter appears once reports exist. The plan header and the stage detail Output
sub-tab also show token summaries when available.

## Queue

Add multiple goals with **Add to queue** on the Overview tab, reorder them on the
Queue tab, then start the queue. Forge processes one goal at a time through the same
plan → approve → run loop above.

The `queue_auto_approve` setting is off by default: Forge pauses at each
plan for your usual approval. Enable it to approve each plan automatically
and run the queue unattended. A blocked or failed item stops the queue
for human intervention; remaining goals stay queued. This also applies to another
**Start queue** request and after an engine restart: only the first unfinished
goal may start. A blocked or failed goal is never silently skipped. After fixing
the cause of the block (for example, model availability), click **Start queue**
again. It retries planning if no plan for that goal was published, or continues
that goal's saved plan. Committed stages and review budgets are preserved.
Drafts follow the current `queue_auto_approve` setting; with it enabled, the
queue proceeds through planning, approval and execution without extra clicks.
An unresolved blocker stops the same goal again. Start is disabled while the
project is busy or the queue is already active.
Approving or running a saved plan for a later queued goal cannot bypass this order.

Successfully completed goals are removed from the queue automatically.
Failed or blocked goals stay visible until you dismiss them with their
**×** remove button on the Queue tab. Removing a failed or blocked goal explicitly
allows the following goal to proceed; do this only when intentionally abandoning
that prerequisite. Pending goals can be reordered within a pending group, but
cannot be moved across a blocked, failed, or active goal.

Queue state lives in `.forge/queue.json` in the project. The Queue tab shows
each item's status and its label shows the count; the bar widget shows the pending count and a queue tooltip.

The JSON API accepts POST requests to `/api/queue/add` with `{"goal":"…"}`,
`/api/queue/remove` with `{"id":1}`, and `/api/queue/move` with
`{"id":1,"dir":"up"}` (or `"down"`). Removal accepts queued, failed, or
blocked goals. `/api/queue/clear` removes pending
goals; `/api/queue/start` starts processing. `GET /api/state` includes
the queue and whether it is active.

### Feature specs

The feature-spec workflow ([documented in `docs/features/`](docs/features/README.md)) organizes
specifications, scenarios and milestones for a feature before it is planned. Milestone M1 lets the
engine discover and validate feature folders and expose them through a read-only API and a panel list.
Milestone M2 adds the spec phase: creating a feature from the template, a read-only co-authoring agent
whose proposed file changes the engine validates and writes, an architect spec review, and spec and
scenario approvals bound to a Git commit and a content hash. Milestone M4 adds pen.dev integration for
UI mockups in feature specifications. Milestone M3 plans an approved milestone as a Forge plan and
registers business tests that every later plan keeps passing. Milestone M5 (panel viewer) remains
planned.

```
GET /api/features[?project=<path>]
```

Returns `{"project": "/path/to/project", "features": [...], "activity"}`, features sorted by slug.
Each feature keeps its M1 fields — `slug` (the folder name under `docs/features/`; names starting with
`_`, like `_template`, are excluded), `title` (the feature's first `# ` heading in `README.md` outside
fenced code blocks, or the slug if there is none), `path` (the feature folder's absolute path),
`status` (`"valid"` or `"invalid"`) and `reasons` (validation errors — missing or unreadable required
files, malformed or duplicate scenario IDs, unknown scenario IDs in `Covers:` lines, malformed or
repeated `Status:` lines, and malformed or repeated `Business tests:` lines or missing registered files —
empty when valid) — plus the milestone progress read from `milestones.md`:

- `milestones`: `[{"id": "M1", "title", "status": "implemented"|"planned", "covers", "business_tests",
  "plan"}, ...]`, one per `## ` heading, from its `Status:` line (a milestone without one is `planned`).
  `covers` is the milestone's scenario IDs in order (`[]` for `Covers: none yet`). `business_tests` is its
  registered business test files, as repository-relative paths in order without duplicates, parsed from
  its `Business tests:` line (see [the registry grammar](docs/features/README.md#business-test-registry)).
  `plan` is `null` for a milestone that was never planned, otherwise the latest plan link for it:
  `{"status": "planning"|"planned"|"failed"|"completed", "plan_id", "started_unix", "completed_unix",
  "commit_range": {"base", "head"}|null}`.
- `progress`: `"implemented"` when every milestone is implemented, `"in progress"` when some are, and
  `"planned"` otherwise. It is independent of `spec_status`.

It also adds the M2 spec-phase fields:

- `spec_status`: `"draft"`, `"spec approved"` or `"scenarios approved"`, derived on every read from the
  feature's approvals and the folder's current content hash; it is never stored as authoritative state.
  A feature whose runtime state file exists but cannot be read or parsed reports `spec_status: "error"`
  with a `state_error` reason instead of being silently treated as `"draft"`.
- `content_hash`: a lowercase sha256 hex digest of the feature folder's file paths and contents
  (symlinks are hashed by their target text and never followed).
- `latest_review`: the feature's most recent architect spec review record (see below), or `null`.
- `review_current`: `true` when `latest_review`'s content hash equals the current `content_hash`.

The top-level `activity` is `null`, or the most recent feature chat or architect-review request:
`{"kind": "chat"|"review", "slug", "request_id", "status": "running"|"ready"|"failed", "error"?}`. The
endpoint remains read-only: it never creates, modifies or removes files. `?project=<URL-encoded-path>`
targets another project without switching the active one. When `docs/features/` is absent or empty,
`features` is `[]`.

```bash
curl http://127.0.0.1:8734/api/features
curl http://127.0.0.1:8734/api/features?project=%2Fpath%2Fto%2Fproject
```

```
GET /api/features/state?slug=<slug>[&project=<path>]
```

Returns one feature's full picture: `{"project", "slug", "valid", "reasons", "spec_status",
"content_hash", "latest_review", "review_current", "state", "activity"}`, where `state` is that
feature's complete runtime state record (see [Feature runtime state](#feature-runtime-state) below).
HTTP 400 for an invalid slug; 404 when discovery does not list the slug; 500 `{"error"}` when the
runtime state file exists but cannot be read or parsed, rather than a silent reset.

```
GET /api/features/content?slug=<slug>[&project=<path>]
```

Serves one discovered feature's content read-only, whether the feature is valid or invalid:

- `slug`.
- `files`: an object keyed by `README.md`, `scenarios.md`, `decisions.md` and `milestones.md`, each
  `{"text": string|null, "error": string|null}`. A file that is missing, not a regular file, a
  symbolic link, unreadable, not UTF-8 or larger than 2 MiB has `text: null` and a reason in `error`;
  the request itself does not fail.
- `scenarios`: `[{"id", "title", "given", "when", "then", "milestone", "result"}]` from the
  `## S<n>: <title>` sections of `scenarios.md` (headings inside fenced code are ignored, the first
  section of a repeated ID wins), each with its `- Given:`, `- When:` and `- Then:` text (or `null`)
  and the ID of the milestone whose `Covers:` names it (or `null`). `result` is `null` when no review
  recorded the scenario, else its latest `scenario_results` entry (see
  [Feature runtime state](#feature-runtime-state)) as `{"status", "evidence", "role", "plan_id",
  "milestone", "unix", "out_of_date"}`; `out_of_date` is true when the scenario's section now hashes
  differently from the recorded `scenario_hash`.
- `results_error`: `null`, or why the runtime state could not be read; the results are then `null`.
- `milestones`: `[{"id", "title", "status", "covers", "business_tests"}]` as discovery reads them.
- `design`: the files under `design/`, walked recursively in name order, each `{"path", "kind",
  "error"}` with `path` relative to the feature folder (e.g. `design/main.pen`) and `kind` one of
  `"pen"`, `"png"`, `"mermaid"` (`.mmd`) or `"other"`. A `pen` entry adds `png`, the relative path of
  its exported PNG (`<name>.png`, or the first `<name>.<frame>.png`) or `null`, and `pngs`, every
  export by the engine's naming rule. A `png` entry adds `absolute_path`. A `mermaid` entry adds
  `text` (or `null` with an `error`, like `files`). Symbolic links are listed with an `error` and never
  followed.

No symbolic link under `docs/features/<slug>/` is ever followed, the folder itself included; `..` or
absolute components are refused, and nothing outside the folder is opened. The endpoint never creates,
modifies or removes a file (the runtime state is only read; a missing state file is not created).
HTTP 400 for a missing or invalid slug; 404 when discovery does not list
the slug; 403 when the feature folder itself cannot be served (for example because it is a symbolic
link).

```
POST /api/features/create {"project"?, "slug", "title"}
```

Creates `docs/features/<slug>/` from `docs/features/_template/`, replacing the first `# ` heading
(outside fenced code) of the copied `README.md` with `<title>`. Nothing is staged or committed; approval
owns the folder's first commit (see approve_spec below). Returns 200 `{"ok": true, "slug"}`. HTTP 400
for an invalid slug (`^[a-z0-9]+(-[a-z0-9]+)*$`, at most 64 bytes) or title (trimmed, non-empty, at most
200 characters, a single line); 409 when the folder already exists, or when `docs/features/_template/`
is missing (there is no engine-bundled fallback template); 409 `{"error":"busy"}` while the engine is
busy or the queue is active.

```
POST /api/features/chat {"project"?, "slug", "message"}
```

Sends one message to the read-only co-authoring agent for the feature. Returns 200
`{"ok": true, "request_id"}` and runs the turn in the background; poll `GET /api/features` or
`GET /api/features/state` and watch `activity` for its outcome. HTTP 400 for an invalid slug, an empty
message, or a message over 20000 characters; 404 for an unknown feature; 409 `{"error":"busy"}` while
the engine is busy or the queue is active.

The agent never writes files itself: it returns a reply and a list of proposed `{"path", "content"}`
files as JSON, and the engine alone validates every path against `docs/features/<slug>/` and then
writes the whole set, or nothing. A path that is not strictly inside that folder — absolute, containing
a `.` or `..` component, a prefix trick such as `docs/features/<slug>-x/`, or passing through a symlink
anywhere along it — is rejected and sent back to the agent through the shared response-correction
budget (see [Shared response correction](#shared-response-correction)); every destination is checked
again immediately before writing. On success, the user's message and the agent's reply are appended to
the feature's `chat` history; on any failure (an exhausted correction budget, a provider error, a
stopped run, or a transcript that could not be persisted) the folder is left exactly as it was, `chat`
is unchanged, and `activity.status` becomes `"failed"` with an `error`. The write set is a
transaction: a failure part-way through it, or after it, restores every file and directory it had
touched, so the published files and the recorded transcript never disagree.

```
POST /api/features/review {"project"?, "slug"}
```

Requests a structured architect spec review of the feature's current content. Returns 200
`{"ok": true, "request_id"}` and runs the turn in the background. HTTP 400 for an invalid slug; 404 for
an unknown feature; 409 `{"error": "feature is invalid: <reasons joined by '; '>", "reasons": [...]}`
when the feature fails M1 validation — checked, and refused, before any agent starts; 409
`{"error":"busy"}` while the engine is busy or the queue is active.

The review is a read-only turn of the persistent architect: no stage, snapshot or project-check
requirements, validated through the same shared response-correction policy used for stage and plan
reviews. When the plan's architect session is ready and uses the same provider, the review resumes that
session and the turn is recorded in the plan's architecture history; otherwise the feature keeps its own
architect session in its runtime state and resumes it on the next review. On success, a review record
`{"id", "approved", "summary", "issues", "questions", "content_hash", "provider", "model", "session",
"unix"}` is appended to the feature's `reviews` history; on any failure nothing is recorded and
`activity.status` becomes `"failed"` with an `error`.

```
POST /api/features/approve_spec {"project"?, "slug"}
```

Approves the spec of a feature whose latest architect review approved its current content. Synchronous:
commits only `docs/features/<slug>/` — a path-scoped `git add` followed by a path-scoped `git commit`,
so other staged or unstaged changes are left exactly as they were — as `docs(features): approve <slug>
spec`, appends `{"kind":"spec", "commit", "content_hash", "unix"}` to the feature's `approvals`, and
returns 200 `{"ok": true, "commit", "content_hash", "spec_status": "spec approved"}`. A folder that
already matches HEAD produces no new commit; the recorded commit is HEAD either way, which makes a
retry after a failed state write safe. HTTP 400 for an invalid slug; 404 for an unknown feature; 409
`{"error":"busy"}` while the engine is busy or the queue is active; 409 with a reason and no commit when
the feature fails M1 validation, has no architect review, its latest review requested changes, or its
latest review's content hash no longer matches the folder.

```
POST /api/features/approve_scenarios {"project"?, "slug"}
```

Approves the scenarios of a feature whose spec approval still matches the current content. Appends
`{"kind":"scenarios", "scenario_ids", "commit", "content_hash", "unix"}` — `scenario_ids` are the IDs
from `scenarios.md` in document order, and `commit`/`content_hash` are copied from the spec approval, so
this endpoint creates no commit of its own — and returns 200 `{"ok": true, "scenario_ids", "commit",
"content_hash", "spec_status": "scenarios approved"}`. HTTP 400/404/409 busy as above; 409 when the
feature is invalid, has no spec approval, or its spec approval no longer matches the current content.
The folder is hashed again after `scenarios.md` is read and before the approval is appended, so an edit
that lands while the endpoint runs is refused with `feature changed during approval` rather than
recorded: the stored `scenario_ids` and `content_hash` always describe the same content.

```
POST /api/features/plan {"project"?, "slug", "milestone"}
```

Plans one milestone of an approved feature (S27, S28). Every refusal is decided before any agent starts
and leaves `.forge/plan.json` and `.forge/features/<slug>.json` untouched. HTTP 400 for an invalid slug
or a missing milestone ID; 404 for an unknown feature or an unknown milestone; 409 with an `error`
reason when the feature is invalid (with its `reasons`), when its `spec_status` is not
`"scenarios approved"` for its current content, when the milestone is already implemented or has
`Covers: none yet`, when it covers a scenario ID that is not among the approved `scenario_ids` (the IDs
are named), and `{"error":"busy"}` while the engine is busy with a plan or the queue is active.

Otherwise the engine builds the goal itself — `Implement milestone <M> (<milestone title>) of the
feature <feature title> specified in docs/features/<slug>/, covering scenarios <IDs>.` — records a plan
link with status `planning` in the feature's runtime state, and starts planning in the background exactly
as `POST /api/plan` does. Returns 200 `{"ok": true, "goal"}`; follow the plan through `GET /api/state`. A
failed write of the plan link releases the engine and returns 500 without starting anything. Planning
does not start execution and does not add the milestone to the queue.

The resulting plan carries a top-level `feature` object `{"slug", "milestone", "title",
"scenario_ids"}`. Only plans started by this endpoint have one: a `feature` key supplied by the planner
is removed, and goal, discussion, refactor and queue plans carry no feature context and write no feature
state (S35). For a milestone plan the planner, architect and reviewers receive a compact feature
reference — the folder `docs/features/<slug>/`, the milestone ID and title, the covered scenario IDs and
the rules below, never the contents of the feature's files. The planner must make the first stage turn
every covered scenario into an executable test named after its scenario ID that fails before
implementation and name every covered ID in that stage's instructions or acceptance; a plan that misses
an ID goes back to the planner through the shared response-correction budget. The final stage sets the
milestone's `Status: implemented` and its `Business tests:` line, so those edits are part of the
reviewed commit range. Plan review of a milestone plan adds one criterion per covered ID — `an
executable test traceable to <ID> exists and passes` — to the criteria to evidence, so an unverified
scenario prevents approval. After the plan review approves (or the plan completes with per-stage review
only) the engine only records the link as `completed` with its commit range; it makes no repository
change after approval (S34).

Business test rules for every plan (S32, S33): when any feature registers business test files on
`Business tests:` lines, the stage and plan reviewers and architect reviews of every plan — milestone or
not — receive the sorted list of registered files of all features and must run them with the matching
project command and record them in the project checks; a failing business test prevents approval. The
engine lists the registered files that a stage diff (from the stage's attempt head to the working tree)
or a plan diff (from the plan review base, covering plan-fix rounds) modifies, deletes or renames; the
reviewers reject the change unless it only adds tests or implements a scenario change approved in the
feature spec. The engine never blocks such a diff itself. Implementer, fixer and plan-fixer prompts say
that a conflict with an approved scenario is escalated to the architect as an architectural context gap
and is never resolved by changing, skipping, weakening or deleting the business test. With no registered
business tests and no milestone feature context, these prompts are unchanged.

#### Feature runtime state

Reviews, approvals, the co-authoring chat transcript, milestone plan links and scenario results are
runtime state, stored at `.forge/features/<slug>.json` inside the project, never under `docs/features/`.
A missing file means the defaults `{"version":1, "slug", "reviews":[], "approvals":[], "chat":[],
"architect_session":null, "plans":[], "scenario_results":[]}`. A file written before milestone M3 has no
`plans` key and one written before M5 has no `scenario_results` key: each is read defaulted to `[]` and
the file is not rewritten by the read; if present, each must be an array. Reviews, approvals and
scenario results are append-only: a later entry never removes an earlier one, so the full history
survives every later change.

`scenario_results` holds the per-scenario test results of milestone plan reviews, in order:
`{"scenario_id", "status", "evidence", "role", "plan_id", "milestone", "unix", "scenario_hash"}`. After
the reviewer's verdict on a milestone plan is persisted — the plan review's, or the final stage review's
when every role reviews per stage and that stage carries the scenario criteria — the engine appends one
entry per verdict criterion `an executable test traceable to <ID> exists and passes`: `status` is
`"passed"` or `"failed"`, `evidence` is the reviewer's evidence, and `scenario_hash` is the digest of
that scenario's section of `scenarios.md` (its `## <ID>` heading through the line before the next `## `
heading outside fenced code) at recording time, or `null` if it could not be read. A scenario's shown
result is its latest entry. Plans not started from a feature record nothing. Recording is best effort: a
failed write is logged and never changes the review outcome.

`plans` holds one link per `POST /api/features/plan` that started, in order: `{"milestone", "title",
"goal", "scenario_ids", "started_unix", "status", "plan_id", "error"?, "completed_unix", "commit_range"}`.
`status` is `"planning"` from the request until the plan is published, then `"planned"` (with the
published `plan_id`) or `"failed"` (with an `error`), and `"completed"` once the plan review approved the
plan (or the plan finished with per-stage review only), when the engine sets `completed_unix` and
`commit_range` `{"base", "head"}` — the first commit under review and the finalized HEAD. `plan_id` and
`completed_unix` are `null` and `commit_range` is `null` until then. Recording completion is idempotent
and best effort: a failed write is logged and never fails the finished run. A milestone's `plan` in
`GET /api/features` is its latest link. The file is written through the engine's durable-publish
protocol (temp file, fsync, rename), so an interrupted write never leaves invalid JSON and a stray temp
file next to it is ignored on read; a file that exists but is unreadable or structurally wrong is
reported as an error rather than silently reset, since resetting it would discard approval history.

`spec_status` is derived from this state and the folder's current content hash, never stored: it is
`"scenarios approved"` when the latest `spec` and `scenarios` approvals both carry the current content
hash, `"spec approved"` when only the latest `spec` approval does, and `"draft"` otherwise. Changing any
file in `docs/features/<slug>/` afterwards — through co-authoring or by hand — therefore returns the
status to `draft` on the next read, while every earlier review and approval remains in the history; a
new approving review and a new spec approval are required before approving again.

The panel lists discovered features with their title, slug, validation status and spec-phase status
(with reasons for invalid ones), opened with the **Features** tab, the `f` key or `g f`. Fully
implemented features are hidden until **Show implemented (N)** or `i` lists them; partly implemented
ones show `N/M implemented` in place of their spec status; see
[Feature list](#feature-list) below for its controls. Opening a feature runs
`omarchy-launch-editor <folder>` to edit it in nvim. The panel fetches features when the tab is shown or
refreshed; while a chat or review request it started is still running, it also polls `GET /api/features`
(and the selected feature's state) once a second until that activity is ready or failed.

From the feature list you can create a feature (slug and title), select one to see its detail — the
latest architect review's verdict (approved or changes requested, with its summary, issues and
questions) and the co-authoring chat transcript — send a message to the co-authoring agent, request an
architect spec review, and approve the spec and then the scenarios once each becomes allowed. Engine
refusals (invalid slug or title, an unapproved or stale review, a busy engine, and so on) are shown
inline in the list. The detail also lists the feature's milestones with their status and plan status
(`planning`, `planned`, `failed` or `completed`, from `plan` in `GET /api/features`). Each planned
milestone that covers scenarios has a **Plan milestone** action, enabled only while the feature is
`scenarios approved`; it calls `POST /api/features/plan`, shows a refusal reason inline under that
milestone, and on success switches to the Plan tab.

#### pen.dev integration (M4)

Features can include UI mockups as pen.dev `.pen` files in the `design/` folder. Agents editing designs
do so headlessly through the shell:

**Requirements:** `npm install -g @pen.dev/cli` and `pen login` to authenticate. Mise-installed `pen` is also supported.

**Shell-based editing:** Agents use identical instructions for both Claude and Codex. They drive `pen interactive --in/--out` with
stdin tool calls like `execute({ input: '<js>' })`, `save()` and `exit()`, reading the CLI's bundled skill at `dist/out/skills/pen-dev/SKILL.md`
inside the installed `@pen.dev/cli` package. No MCP server or desktop app is used for automated design work; the desktop app and its MCP server
are only for manual editing.

**PNG export:** After each implementer or fixer turn that creates or modifies a `.pen` file, the engine exports PNGs
before review snapshots or commits. Naming follows the design: a single top-level frame produces `<name>.png` in the same directory
as `<name>.pen`; multiple frames produce `<name>.<frame-slug>.png` for each frame (lowercase with non-alphanumeric characters replaced by `-`).
Stale exports of that design are removed; exports of other designs and unrelated PNGs are kept.

**Blocking and resuming:** A missing pen CLI or authentication (`pen login` required) leaves the stage uncommitted with a `design_blocked` status and a
message naming the problem and the fix. Resuming the run retries the export without requiring a new agent turn. Export failures are treated like
failing checks: they are returned to the fixer alongside other issues, and the stage commits only after successful export.

**Reviewer access:** Reviewers and the architect inspect changed `.pen` files and their exported PNGs in read-only sandboxes without running `pen`.
Review prompts list the `.pen` paths and PNG paths, direct reviewers to inspect the PNG images and `.pen` JSON, and state that the engine
exported them (running `pen` is not required or permitted in the sandbox).

**Gate status:** The `design_blocked` gate status appears in stage listings alongside other statuses; blocked designs block the run
until the fix (install pen or run `pen login`) is complete and the run is resumed.

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

### Panel layout

The panel is a compact tabbed window instead of one long page. A fixed header and
tab bar stay in place while each view scrolls on its own, only when its content is
taller than the window. The design and scenarios are in
[`docs/features/panel-redesign/`](docs/features/panel-redesign/).

- **Header:** `FORGE`, the project switcher, the phase badge, the current step
  and the `⋯` menu. The switcher shows the current project name and `N projects`
  when several sessions exist. Its dropdown lists every session with its status
  marker (`●` busy, `!` needs attention, `✓` done, `+N` queued), the full path of
  the active project, and **Change project…**. Choosing a session selects that
  project and keeps the current tab. The `N projects active` indicator for
  background work stays in the header.
- **`⋯` menu:** Update Forge, Discard plan, Refactor plan, View diff, Change project
  and Keyboard help. Items use the same enabled rules as the old buttons.
- **Tab bar:** Overview · Plan · Activity · Architecture · Features · Queue (with
  the queued count) · Settings. The selected tab is underlined. Every tab view is
  a persistent instance that is shown or hidden, never rebuilt, so the tab, reading
  positions and drafts survive polling, project switches and reopening the panel.
  A new panel starts on Overview.
- **Overview:** the goal, the phase's actions and a status summary. While Forge is
  busy it shows a "now working" card (stage, role, tool, model, elapsed time,
  latest output line), a one-line progress summary, one compact line per stage
  (click or Enter opens its stage detail page) and **Stop**. A blocked or failed
  stage is shown as one line right under the goal with its status and reason
  (`! stage 3 · title · blocked — fix rounds exhausted ›`); clicking it opens that
  stage's detail page. It is shown while idle too. While idle it shows the goal field with
  **Create plan**, **Discuss first**, **Enhance with AI** and **Add to queue**, a
  short last-run card and the one-line quota summary. Only actions that are enabled
  in the current phase are shown as buttons; the others stay reachable from their
  own view or the `⋯` menu.
- **Plan:** a summary line (`N stages · M committed · review: per plan`) with
  **Approve**, **Start**, **Edit plan** and **Q&A** while they apply, then one line
  per stage: status icon, number and title, commit hash (or status or current
  activity) and duration, then `›`. These are the same rows the busy Overview uses.
  Clicking a row, or pressing Enter / `o` / Space on the selected one, opens its
  stage detail page. Routing, model and review policy are not shown on the list.
  Below the rows, a one-line **plan review strip** shows the verdict, round
  `n of budget + 1` and the architect and independent outcomes
  (`Plan review · approved · round 2 of 4 · architect ✓ independent ✓`); clicking it
  opens the plan review details in Architecture. Then come the “what should be
  improved” feedback field with **Improve with AI** and the plan Q&A. Plan editing
  (`e`), plan Q&A and feedback stay in Plan; while editing, the editor replaces the
  compact rows, with committed stages locked.
- **Stage detail:** its own page, pushed over Plan with the header and tab bar
  kept in place. A breadcrumb `‹ Plan / 3. Title` returns to Plan, and `‹ 2 · 4 ›`
  steps to the previous / next stage. A status line shows the status, commit hash,
  activity, elapsed time and the routing summary. The sub-tabs hold everything an
  expanded stage card used to show: **Instructions** (instructions, the proposed
  commit message, **View diff**, **Live output ›** and **Copy**), **Acceptance**
  (acceptance criteria), **Review** (review policy, architect and independent
  outcomes, the review-policy rationale and the historical reviews with **Load older
  reviews** and **Retry reviews**), **Routing** (model status, the planner and
  architect rationale and **Model agreement and routing details**) and **Output**
  (current activity, token usage and **Live output ›**). `[` / `]` move to the
  previous / next stage, `h` / `l` switch sub-tabs, and `q`, `Escape` or the
  breadcrumb return to Plan with that stage selected and Plan's scroll position
  unchanged. Choosing another tab, or `g` then a tab key, leaves the page first.
  Polling updates the status in place and never closes the page unless the project
  changes or the stage disappears.
- **Activity:** Live, History and Reports with their filters, using the full
  height of the view.
- **Architecture:** the architecture card, role token totals, and the plan review
  status with its fix commits and history.
- **Features:** the feature specs list (see [Feature list](#feature-list)).
- **Queue:** the queue with **Start queue**, ↑, ↓ and ×.
- **Settings:** planner, architect, implementer, reviewer, automatic routing,
  architect review, reviewer review, push at end and auto-approve as label/value
  rows that cycle on click; one compact line per quota window (`Claude · 5h 65%
  remaining · resets Sat 02:17`, with the full text in a tooltip and under
  **▸ details**) and the model catalogue summary with its errors, **Model settings & options**, **Refresh models**, **Cancel refresh**,
  **Refresh Claude limits**, **Update Forge** and **Change project**.

Pushed pages (stage detail, discussion chat, model settings, diff viewer, project
chooser and keyboard help) sit on top of the current tab; closing one returns to the same tab
with its selection and scroll position intact. The bottom hint line names the
current view's keys.

The Settings tab has **architect review: per stage | per plan** and
**reviewer review: per stage | per plan** rows. Each reflects the current
setting, initially **per plan** for both roles on engine startup, and shows an
unavailable placeholder before settings arrive. Each toggles only its role while
posting both cadence keys. They are disabled offline;
Tab, Space and Enter operate them, and Escape returns to panel shortcuts.
The new value applies to fresh attempts, not an already captured stage gate.

Each stage's detail page shows its **review policy**, **architect and independent outcomes**,
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

A deferred role displays **deferred to the plan review**. A fully deferred gate
distinguishes **awaiting commit under a deferred review policy** from
**committed under a deferred review policy**, without presenting either as an
approval. Plan review appears separately beside the architecture area, outside
the stage rows (summarised on Plan by the plan review strip), with phase status, current gate, round `n of budget + 1`,
architect/independent outcomes, each fix round's commit SHA and subject, and any
final fix commit SHA. Its role-tagged requests
expand to complete, wrapping, selectable plain text with **Copy full text**.
If state shortened the requests, opening the detail retrieves bounded pages of
architecture history and matches the exact plan, attempt, round, policy,
snapshot and role identities. Until retrieval succeeds, including after a load
failure, the shortened preview remains labelled and full selection/copy stays
unavailable; the detail offers retry. This uses disclosure-driven retrieval
without increasing polling or page caps.

Open a stage's detail page and choose **Review** to read complete, wrapping,
selectable review text: summaries,
change requests, notes and checks under `verified:`, with recorded round, decision
and timestamp (UTC). Ctrl+C copies the exact selection. Opening the Review sub-tab automatically
loads completion for shortened records; until verified, their summaries remain
explicitly labelled previews and selectable. A loading failure or unverifiable
history appears as one group status line with **Retry reviews**. Automatic loading
stays suppressed through polling, execution publications and closing/reopening the
page until Retry or the applicable project, visit, plan, revision or stage scope
reset. A closed page initiates no automatic load, but an already-started chain may
finish its pages after it closes. Complete records do not reload on reopening.
**Load older reviews** pages eight at a time; Retry and older-page loading are
group actions, with no per-review Expand, Load or Copy controls.
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

### Compact details

Chat (including Plan Q&A), live output, history, reports, providers, catalogue
and queue keep compact per-field expansion: the first non-empty line appears
with an ellipsis when it exceeds the available width. Each message or field stays
separate. The architecture card is unchanged, including its grouped disclosures
and compact guidance, decision and risk fields.
**Expand** opens selectable plain text; **Copy full text** copies the original,
including blank lines and whitespace. Use Tab and Space/Enter for the controls;
Escape returns to panel shortcuts. Empty text has an explicit empty preview.
Report expansion and its individual text expansions remain separate.
Statuses, errors, quota warnings, metadata and actions stay visible. Goal/plan/
model-policy editors, structured diffs and keyboard help keep their existing uses.

Plan rows are one line each; Enter / `o` / Space on the selected stage (or a click)
opens its stage detail page. The page's status line and sub-tabs show the stage
status and sha, current activity, review policy and gate, the latest historical
review line, elapsed time, model identity/effort, availability verification, tier
provenance, and routing or block errors. There are no preview lines, per-field
Expand controls or editors. The sub-tabs show Commit, all model-agreement
rationale (including routing-history rationale), review-policy rationale,
Instructions, Acceptance criteria and historical reviews as complete, wrapping,
selectable plain text, with no per-field controls. Ctrl+C copies the exact
selected original text, including whitespace and CR/CRLF line endings. The single
permitted extra toggle, **Model agreement and routing details** on the Routing
sub-tab, reveals only risk, constraint, cost qualification, routing price,
agreement id and latest invocation; rationale remains visible whenever the
sub-tab is shown. Committed stages remain read-only during plan editing.

An open detail page holds stage prose at the snapshot taken when it opened, so
polling cannot rebind an open editor or destroy a selection; status lines keep
updating. Newer prose appears after closing and reopening the page or on a
project, project visit, plan, revision or stage scope change. Review presentation separately retains
selected originals, labelled when held from an earlier publication, without
making them verified in the current review scope. A selected preview stays
labelled while its completed record appears separately; releasing the selection
allows reconciliation. Closing the page or a stage scope reset clears held
review presentation.

Inspecting chat or output suspends following new messages. Polling preserves
unchanged selections and open text; changed fields refresh on collapse (for stage prose: on closing and reopening the detail page). An open
field removed from the current snapshot stays labelled **previously shown** until
collapsed.
Legacy unstructured logs remain accessible as a single full-text entry: old
message boundaries and text discarded before this upgrade cannot be reconstructed.
See [panel detail implementation and isolated runtime checks](docs/panel-details.md).

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

Open keyboard help with **Keyboard help** in the `⋯` menu or by clicking the
hint line at the bottom of the panel, which underlines on hover. The hint line names
the keys of the current view, for example `NORMAL · g o/p/a/r/f/q/s or [ ] switch
views · j/k select · Enter open · ? help`, and reads `INSERT - Esc to normal mode`
while a text field has focus.

Actions follow the buttons’ enabled state. Uppercase keys use `Shift`. Keys act on the
view that shows their target and select that view: stage keys select Plan, and
`Tab`, `h`/`l`, `Ctrl+d`/`Ctrl+u` and the digit filters select Activity.

#### Panel (normal mode)

| Key | Action |
| --- | --- |
| `[` / `]` | Switch to the previous / next view tab |
| `g` then `o` / `p` / `a` / `r` / `f` / `q` / `s` | Go to Overview / Plan / Activity / Architecture / Features / Queue / Settings; `gg` is unchanged |
| `i` | Edit the goal (insert mode) |
| `I` | Edit plan feedback (insert mode); Enter improves with AI |
| `Escape` | Leave a text field, close the top overlay, or cancel plan editing |
| `j` / `k` | Select next / previous stage (a row on Settings, a report in Reports) |
| `gg` / `G` | Select first / last stage |
| `Enter` / `o` / `Space` | Open the selected stage's detail page; focus its title when editing |
| `Tab` | Toggle Live / History |
| `h` / `l` | Select Live / History |
| `Ctrl+d` / `Ctrl+u` | Scroll Live / History half a page down / up |
| `Page Down` / `Page Up` | Scroll the current view down / up |
| `Home` / `End` | Jump to the top / bottom of the current view |
| `1` / `2` / `3` / `4` / `5` | History: All / Runs / Git / Reviews / Errors |
| `6` | History: Reports (when reports exist) |
| `p` | Create plan from goal |
| `t` | Open the discussion chat |
| `P` | Create plan from discussion |
| `E` | Enhance the goal description with AI |
| `e` | Edit plan stages by hand |
| `a` | Approve draft plan |
| `r` | Run approved or completed plan |
| `x` | Stop run or active queue |
| `d` | Open uncommitted diff |
| `c` | Change project |
| `f` | Open the Features tab |
| `?` (`Shift+/`) / `F1` | Open keyboard help |

#### Stage detail

| Key | Action |
| --- | --- |
| `[` / `]` | Previous / next stage (a `[ ]` press here does not switch tabs) |
| `h` / `l` | Previous / next sub-tab: Instructions · Acceptance · Review · Routing · Output |
| `q` / `Escape` | Back to Plan with the stage selected |
| `g` then a tab key | Leave the page and go to that tab |
| `d` / `x` | View diff / stop, with their usual guards |
| `?` (`Shift+/`) / `F1` | Open keyboard help |

#### Diff viewer

| Key | Action |
| --- | --- |
| `j` / `k` | Scroll down / up |
| `Ctrl+d` / `Ctrl+u` | Scroll half a page down / up |
| `gg` / `G` | Jump to top / bottom |
| `R` | Refresh diff |
| `q` / `Escape` | Close diff |

#### Feature list

Typing in the new-feature or chat-message fields is handled by the field itself, like other panel
text fields; the shortcuts below apply in normal mode.

| Key | Action |
| --- | --- |
| `j` / `k` | Select next / previous feature |
| `Enter` / `o` | Open the selected feature folder in nvim (via `omarchy-launch-editor`) |
| `n` | Open the new-feature form (slug and title) |
| `c` | Select the feature and focus the co-authoring chat message field |
| `v` | Request an architect spec review of the selected feature |
| `a` | Approve the selected feature's spec |
| `A` | Approve the selected feature's scenarios |
| `R` | Refresh the feature list |
| `q` / `Escape` | Return to the tab shown before Features |

#### Project chooser

These shortcuts apply in normal mode, with text-field behavior noted below.

| Key | Action |
| --- | --- |
| `j` / `k` | Select next / previous project |
| `Enter` | Open selection; in filter, open first match; in path field, set path |
| `/` / `i` | Edit project filter (insert mode) |
| `q` / `Escape` | Close chooser (Escape leaves a text field first) |

#### Discussion chat

| Key | Action |
| --- | --- |
| `i` | Edit the message (insert mode) |
| `Enter` / `Shift+Enter` | Send the message / insert a newline |
| `q` / `Escape` | Close the chat (Escape leaves the message field first) |

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

Use **Model settings & options** in this order:

1. Click **Refresh models** for the latest available model list.
2. Click **Update shortlist with AI** to generate a proposed shortlist.
3. Review the proposal in the editor; where applicable, **Apply AI tiers** applies
   the proposal to the draft and **Undo AI tiers** restores the previous draft.
4. Click **Save model policy** to activate and persist the policy.

Refresh affects discovery; suggestion, editing, applying, and undoing affect only
the draft. Only **Save model policy** changes the active/persisted policy.

The AI selects up to four distinct current models per provider, covering simple,
everyday and complex work. Each explicit request fetches both providers' official
model pages when metadata research is enabled. It uses their current
recommendations and descriptions to choose from visible discovered and eligible
configured models. Family names and generations are not fixed in code: newly
discovered successors can replace older families. Aliases resolving to the same
model count once; hidden endpoints are only candidates if explicitly configured.
The validator requires four selections per provider, or all candidates if fewer
exist, and rejects invented IDs or duplicates. Incomplete or stale discovery and
unavailable sources are reported; cached context is not presented as current
verification, and no unknown model IDs are accepted.

This uses the configured planner in a fresh read-only call (the planner must
already have an eligible strong model). The AI suggests `basic`, `standard`, or
`strong` from official model descriptions, with explanations and uncertainty.
Default or maximum reasoning effort is not evidence of capability tier. The engine
computes cost preferences separately: when metadata research is enabled, it
fetches the official OpenAI pricing page and reads Claude prices from the fresh
official model comparison table. Discovery resolves CLI aliases to exact API IDs,
including the Claude context modifier; unresolved aliases stay unknown. Comparable
standard short-context input/output rates of the selected shortlist share one
ranking across providers, with equal prices tied. Each rank includes its source
and rates in the result. Unknown
prices and fetch failures stay `null`. Crossing input/output prices among selected
models prevent a cost ranking; discarded candidates do not affect it. A provider's
failed source does not discard verified prices from the other. AI-supplied cost
numbers are rejected. These draft preferences are an API-price proxy for user
policy, not measured CLI subscription costs. Tiers remain suggested user judgments,
not official capability ratings.

The validated result replaces the editor's entries with the shortlist, supplies a
new policy revision, and preserves existing scopes, bridge settings and refresh
settings, along with native efforts, suitability and limits of retained entries.
Suggestions do not change the plan or remove the existing strong floor for
planning/review roles. The engine exposes the suggestion as `POST
/api/models/suggest` with `policy` and optional `project`, returning 202 and a
request ID. `/api/state` includes only request status; `GET
/api/models/suggestion?project=...` returns the full draft result. One request at
a time shares the project's worker/cancellation controls.

Use **Model settings & options** to edit the JSON policy. Every successful
settings update saves the full engine configuration atomically to
`$XDG_CONFIG_HOME/forge/settings.json` (default `~/.config/forge/settings.json`).
This includes role providers/models, reviewer mode, routing, review cadence and
budgets, auto-push, queue auto-approval, project root and model policy. A failed
save returns an error and leaves the active settings unchanged. Startup reloads
this snapshot; a corrupt settings file stops startup with a diagnostic instead
of silently reverting providers or publication preferences.

For older installations without `settings.json`, Forge imports the existing
`model-policy.json` from the same directory and uses defaults for preferences
that older engines never persisted. The next settings update writes the complete
snapshot. The legacy file is retained, but `settings.json` takes precedence once
it exists. A corrupt legacy policy is reported in the panel with an empty registry.
Set a new `policy_revision` whenever changing the policy. For example, replace
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
they do not grant a higher capability tier. `relative_cost_preference` is a
relative user preference rather than a monetary amount: it accepts `null` or a
value from 0 through 1000, and a lower value favours a model as the cheaper
choice. `null` means that cost is unknown — neither cheap nor expensive.
Configured entries do not supply verified prices; missing official prices remain
`null`. Within a tier, cost ordering applies only when every candidate has a
configured preference; otherwise the registry order is retained. Ties also
retain registry order. This configured ordering applies independently of whether
official prices are available.
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
`relative_cost_preference` follows the policy-field definition above, not a
measured rate. When official metadata contradicts discovery (e.g. reasoning
support), the conflict is surfaced and discovered native support wins. The store
lives beside the
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

Claude Code **2.1.263** has no supported `claude models` command. The optional
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
uses the existing `claude` executable. Protocol v1 pins SDK 0.3.261 but does not
restrict Claude Code to a list of versions. CLI version is recorded as provenance;
each probe checks the actual control protocol and validates its response schema.
Compatible CLI upgrades continue working without a Forge update. An incompatible
protocol or malformed response reports unavailable discovery/usage, never a zero
balance. The integration was verified locally with Claude Code 2.1.265.
Missing Node, bridge or SDK, or an unsupported SDK reports unsupported discovery. It never falls
back to an API-key request or a generation turn. Without the bridge, explicit
Claude entries continue to work with unverified availability and provider-default
effort.

The same pinned bridge reads Claude subscription limits through the SDK's
experimental structured `/usage` control request (`--forge-usage-v1`), with
transcript analysis disabled and no user prompt or generation. The panel shows
remaining percentages and local reset times separately for the overall windows
and model windows such as Fable. These are subscription limits, not Forge's
per-run token counters. The engine caches readings across projects, refreshes
every five minutes and checks before Claude launches (at most once a minute).
`POST /api/quota/refresh` requests a refresh; `/api/state` includes the cached
`claude_quota` reading and never invokes the provider. Manual checks have a
15-second cooldown, and each probe has a 15-second deadline.

A fresh, exhausted applicable window prevents a Claude launch when usage credits
are explicitly disabled. Scoped windows use the selected/resolved model family;
an unresolved provider default cannot establish a scoped blocker. After reset,
with stale/missing evidence, or when credits may allow continued usage, the CLI
decides availability. Forge never enables credits automatically.
Unavailable readings remain unknown; failed refreshes label the previous reading
as stale. Incompatible CLI protocols or unsupported SDK versions degrade to
unavailable usage information.

All roles use the shared resolver in `src/model_selection.rs`: planner, chat,
architect (including review), independent reviewer, and fixed stage execution.
Roles supply constraints; the resolver owns catalogue facts, quota availability,
and replacement decisions. Chat and enhance use the planner configuration. Stage
routing uses the same candidate pool, with its stage risk/task requirements layered
on top.

With automatic routing and an empty role model setting, selection skips known
exhausted models. The shared resolver selects the weakest adequate eligible tier.
Within that tier, it follows the `relative_cost_preference` ordering rule in the
policy-field description above. For example, with no configured preferences,
configure Fable followed by Opus in the same tier to use Opus when the Fable
pool is depleted.
If the pre-launch probe discovers exhaustion after selection, or the CLI explicitly
reports the selected family's quota limit, the same resolver chooses another
eligible model without revisiting one already attempted. This also works when
usage discovery is unavailable. Generic capacity, rate-limit, and authentication
errors do not establish a model-family limit. Structured provider errors survive
nonzero process exits; `model is at capacity` receives the existing bounded
operational retries rather than immediately triggering reassessment.

The existing capability classes are `basic`, `standard`, and `strong`. A class is
a minimum: a stronger model can serve a lower class. A provider with one basic and
one strong model can therefore cover all three classes. Native effort remains a
separate provider setting. Planner, architect, chat, enhance, and unpinned
independent review keep a fixed `strong` floor as deliberate policy for planning,
guidance and review, independent of automatic stage routing. Implementation and
stage fixes instead use the floor the planner and architect agreed for the stage,
subject to the engine's risk, complexity and task minimums described below. The
plan-wide fix turn uses the strongest floor agreed by any stage in that plan
(defaulting to `strong` if a saved floor is missing), through the shared resolver
and implementer settings. Adding models changes the registry; adding a new
provider still requires its CLI adapter and catalogue discovery support.

In shared role resolution, an explicit role model is used with its configured
provider and registry native `effort`, or refused with the specific reason; it is
never silently replaced by a stronger or different model. Pin errors name the
provider/model and report the configured tier and required floor (`unclassified`
if absent), catalogue ineligibility such as an unknown ID or unsupported/rejected
native effort, quota
reason, or that the model was already attempted. Stage constraints fix their
supplied fields under the joint assignment rules below.

Actual choices are logged and retained in provenance. Fresh roles start fresh
sessions; an architect model change reconstructs its context from the checkpoint
before continuing guidance or review. Explicit model settings remain constraints.
An agreed implementation model is checked by the common resolver and retains its
assignment until the stage's reassessment lifecycle replaces it. Temporary quota
does not itself invalidate an agreement. Review continues to use the other
provider, without spending a review/fix round on model-family quota fallback.
If no adequate independent model remains, Forge preserves the work and blocks.

Model identity checks recognize Claude's exact `[1m]` context suffix: the CLI may
report it at initialization while assistant messages report the same model ID
without it. Raw reported IDs remain in the audit history. Different families,
versions, unknown suffixes and unresolved aliases still fail identity checks.

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
configured model registry. For these two roles, an explicit model must match a
strong registry entry; an empty model selects an eligible strong entry for that
provider using the policy-field ordering rule above. Chat and enhance specifically
use the planner settings and keep the same strong floor.
This uses configured tiers even when official comparative metadata is absent. An
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
architect receives a work-status context rather than the plan document: the
goal, the checkpoint, one compact entry per stage (id, title, status,
dependencies and a short outcome summary), the full design (instructions,
acceptance, commit, dependencies and any model constraint/proposal) of only
the stages the turn must act on, and the paths it is told to inspect itself
for anything else -- the architecture events log, the plan file (and an
unpublished candidate file when one exists), and the repository, with the
prompt instructing it to use `git log` and `git diff` there rather than assume
omitted detail does not exist. It returns a structured checkpoint,
decisions/supersessions, guidance and unresolved risks tagged with the
expected plan ID and revision. Forge validates the complete output and
publishes plan and context together.
Architect guidance is included in implementation and fix prompts. A reviewer
may supply a nonempty `architecture_context_gap` (at most 2,000 bytes) in a
rejected verdict to request refreshed guidance before a fix. Ordinary fix rounds,
unchanged stage boundaries, stops and completion do not incur summary turns.
Successful commits append engine-verified outcomes to durable history and a
bounded recent checkpoint preview. The full plan retains all committed stages.

Stored and expanded checkpoints are capped at 4 MiB, and individual architect
responses at 48 KiB.
Saved constraints and completed interfaces cannot be silently dropped; unresolved
risks require explicit resolution by ID. Completed interfaces are strictly
append-only. A saved constraint may stop applying only through an explicit
`retired_constraints` entry naming the exact saved text and a reason, and the same
turn must omit that text from `checkpoint.constraints`; retiring unsaved text,
retiring the same constraint twice or retiring one the turn still proposes is
rejected. This is how a constraint that a later revision made false, such as one
naming a stage number that now holds different work, is removed instead of being
reported as a contradiction forever. The last sixteen retirements are retained
with their reason and revision. A publication that needs no stage guidance,
routing or recovery reuses the architect boundary without a turn, except when the
plan revision changed: a revision is the one event that can falsify plan-wide
saved context, so it always takes a turn and gives the architect the chance to
retire what no longer applies. Recent decision details remain bounded,
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
When Codex exec omits the model from JSONL, Forge reads the provider's native
`$CODEX_HOME/sessions` rollout (default `~/.codex/sessions`). It matches the exact
stream thread UUID and accepts only a completed turn appended during that
invocation, with matching turn identity and project directory. This fallback was
checked against Codex CLI 0.153.4's `session_meta`, `task_started`, `turn_context`
and `task_complete` records. Old resume metadata and requested model names never
count as a provider report. Missing, ambiguous or changed metadata
leaves the report unverified, with a diagnostic in the model log; existing model
verification gates still block publication. Lookup is capped at 100,000 directory
entries, but session records and appended data have no fixed byte-size limit.
Only bytes present at verification time are read. Large compaction histories and
other unrelated records are validated, but only their identity-relevant fields
are decoded; their contents cannot supply a model or substitute for current-turn
metadata. Provider output events and final responses likewise have no fixed
byte-size limit; full records are retained independently of bounded UI previews.
Child process groups are cleaned up on completion, stop and reader failure.
Neither adapter uses generic `--continue` or `--last`.

Q&A always uses a fresh read-only conversation with a snapshot of saved context.
It writes chat answers but never mutates the plan, decisions, checkpoint or
authoritative architect session. `GET /api/state` and the panel expose architect
activity/recovery, exact session identity, current guidance, decision details,
unresolved risks and usage per role, including legacy plans without context.


### Stage tiers and implementation-time model selection

The planner and architect agree on a stage's required capability tier: `basic`,
`standard`, or `strong`. They do not choose an implementation provider or concrete
model. At implementation start, Forge resolves the weakest adequate eligible tier
within the **currently selected implementer provider**, using the current model
catalogue. For example, with Terra and Sonnet configured as `standard`, the same
standard-tier plan uses Terra when Codex is selected and Sonnet when Claude is
selected. Changing the implementer after planning requires no new AI planning turn.
Within the chosen tier, selection follows the `relative_cost_preference` ordering
rule in the policy-field description above. Price never allows a weaker tier or
silently changes the selected provider.

A missing, unavailable or inadequate model blocks launch with an actionable error;
the tier agreement stays valid. Update the catalogue or change the implementer and
retry. Once an attempt starts, its concrete selection is saved for retries and
restart recovery. Changing the selector affects subsequent stage attempts. Actual
invocations retain requested and provider-reported model identities separately
from the provider-independent tier agreement. Version-one concrete agreements
remain readable for older plans and execution reassessment.

`automatic_routing` defaults to `true`. The implementer selector determines the
provider even with automatic routing enabled. Planner and architect bootstrap
settings remain separate. The reviewer control cycles through `auto (other provider)`,
`codex`, and `claude`. An explicit provider uses `reviewer_provider_mode: configured`
and is respected even when automatic model routing is enabled and the same provider
implemented the work. Review always starts in a fresh read-only session. Missing
eligible models block rather than silently switching to another provider.

`reviewer_provider_mode: other_provider` retains the legacy automatic independence
policy: unpinned review uses the other provider, with plan review checking all actual
contributors. This is the default for older settings; the panel labels it as auto.
Posting `reviewer` alone to `/api/settings` selects configured mode. Clients can
explicitly include `reviewer_provider_mode` to choose either behavior. Automatic
review uses an eligible strong registry entry; explicit model pins retain their
existing capability policy. These are runtime engine settings; the catalogue has
its separate persisted policy.

Selection precedence at launch is explicit:

1. User-authored stage `model_constraint` fields take precedence over global model
   pins. It accepts `provider`, `model` and/or `native_effort`; `null` clears it.
   Omitted provider uses the currently selected implementer. Explicit stage
   provider constraints intentionally override the global selector.
2. Otherwise a nonempty `implementer_model` pins that provider/model and any native
   effort explicitly configured on its registry entry.
3. Forge resolves remaining fields locally within the required tier or a stronger
   adequate tier when necessary. Native effort comes from the chosen provider's
   registry entry, unless explicitly constrained. All selections must satisfy
   capability, task suitability, eligibility, quota and scheduled independent review.

Older saved policies may already contain serializer-inserted `"effort":"provider_default"`
values. Their original intent cannot be recovered: stored effort strings remain
explicit pins. Remove the `effort` field from an entry (and increment
`policy_revision`) to leave its global stage effort unconstrained. Explicitly
authored `provider_default` is never silently discarded.

The plan editor exposes exact constraint fields. HTTP clients may include, for
example, `"model_constraint":{"provider":"codex","model":"your-exact-id"}`
on a pending stage in `POST /api/plan/edit`. Constraints survive AI revision;
agent output cannot manufacture user overrides or committed records. Invalid
IDs, unsupported native efforts, catalogue ineligibility, quota blocks and
inadequate tiers are reported with the validation reason rather than silently
overriding settings. Tier failures state the required floor and selected model's
configured tier (`unclassified` if absent). Cross-provider reviewer conflicts are
checked there when the reviewer is scheduled per stage and other-provider mode is
selected. In that mode, deferred reviewer independence is checked over the frozen
plan subject when plan review runs.

The planner proposes risk, complexity, task, capability tier and a
stage-specific rationale in its existing standard/refactor/revision output.
The persistent architect independently evaluates cross-stage constraints and
failure impact in its guidance turn. Both must explicitly agree on classification
and capability requirements. Missing proposals from manual edits or legacy plans are batched
into one strong planner turn. Disagreement or an engine rejection of an agreed
selection allows up to three further planner/architect exchanges for only the
affected stages; agreement ends them immediately. Inconsistent `agree: true`
responses use the separate response-correction budget before they can enter an
exchange. Engine feedback includes the exact policy failure and names the required
tier; correcting the tier must preserve the already agreed risk, complexity and
task.
Unresolved disagreements or policy failures then block publication with reasons
and correction instructions. A planner proposal alone never supplies architect
approval, and malformed output cannot be published.

Tier agreements use `stage-tier-2`; concrete execution checks retain `stage-routing-1`. Critical risk, complex
complexity, or a concurrency, persistence or security task requires `strong`.
The floor comes only from that agreed classification. Stage wording is never
keyword-matched, so documentation or refactors that merely mention
authentication, persistence formats or atomic writes keep the tier their
classification allows. When no strong-floor rule applies, risk and
complexity both classified simple permit `basic` or higher; otherwise standard
work requires `standard` or `strong`. Scope floors and reassessment safeguards
can raise, but never lower, that minimum. Unclassified models cannot meet these
requirements. Tiers express configured adequacy, never quality inferred from
price, provider, name or list order. The planner and architect must still verify
that the selected option is suitable for the specific work.

Planning does not compare provider prices or consult implementation quota. Local
launch selection uses configured capability and task suitability first, then
the `relative_cost_preference` ordering rule above within the weakest adequate
tier of the selected provider. Unknown prices remain unknown. Execution
reassessment can compare eligible alternatives using configured preferences or
comparable published billing facts, while preserving its capability floor and
retry budget. API prices do not establish CLI subscription cost. Every model has
the same acceptance checks and review gate.

Agreements retain both reasons, distinct proposal identities, explicit agreement,
bootstrap provenance, relevant goal/stage/dependency/constraint inputs, an input
classification and required tier. The separate launch selection records the
resolved model, native effort, capability facts and billing provenance. Previous checkpoints and events retain superseded agreements;
committed stage records remain unchanged. Explicit `depends_on` lists let an
independent edit affect only its own pending stages; absent lists conservatively
mean all earlier stages. Goal or applicable constraint changes reconcile affected
pending stages before manual/queue approval or legacy execution. Approval,
unchanged starts, ordinary fixes, restart, unrelated revisions and catalogue
clock/revision-only changes reuse agreements without selection calls.
Whether a saved planner proposal still fits its stage is decided by comparing
the stage's stored fingerprint of the selection inputs with a fresh one, so
change detection is the same as when the plan stored the expanded inputs.
Plans written in that old format are still accepted: their stored inputs are
fingerprinted for the comparison, so unchanged stages need no new selection
turn, and the compact form replaces them when the stage is next proposed.

Before every implementation/fix invocation, a local check verifies the relevant
saved inputs, chosen option and policy facts. Unrelated catalogue metadata and
availability becoming verified do not invalidate the choice. Material capability, supported effort, alias resolution or removal changes trigger
bounded reassessment at the next safe turn boundary. Cosmetic metadata, price,
provenance and catalogue clock changes do not reopen an existing agreement. Invocations record
proposed, requested and provider-reported effective models, including unexpected
substitution. Every handoff includes the saved architecture summary, decisions,
guidance, constraints, completed interfaces, outstanding findings and worktree/
diff context, and directs a replacement agent to inspect and preserve partial work.

A stage's detail page shows the required tier and deferred model selection before
execution (status line and Routing sub-tab). After launch it also shows the captured model
and actual invocations. The Routing sub-tab shows all model-agreement rationale, including both selection reasons
and retained routing-history rationale. Its single extra **Model agreement and
routing details** toggle reveals risk classification, constraints, cost
qualification, routing price, agreement id and latest invocation only.
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
The `kind` field can be `scope` or `constraint_conflict`. A `constraint_conflict`
request reports that the stage's own constraints contradict each other and cannot
all be satisfied, with the `reason` explaining which constraints conflict and why.

A scope escalation returns the stage to the planner for one revision of its
instructions and acceptance. Forge saves that revision before selecting models
and refreshing architectural guidance; a failed publication can resume after
restart without another scope negotiation. Implementation then repeats under the
revised requirements, followed by review at each role's configured cadence.
If the planner instead explains why the existing requirements are buildable,
Forge saves that explanation and returns it to the implementer for one follow-up
within the current attempt's remaining budget. This does not change acceptance or
approve the implementation. The handoff survives restart; another scope escalation
under the same requirements blocks without repeating the planner dialogue.

### Constraint-conflict escalation

When a stage review exhausts its budget, stalls (for plan review), or when an implementer,
fixer, reviewer, plan fixer or architect reports a `constraint_conflict`, Forge hands the
stage to the planner once per conflict signature. The planner receives the full plan with
each stage's status and commit range, the current stage text, role-tagged outstanding
requests, implementer and fixer evidence from each round, check results, files changed
since the stage attempt began, round and budget counts, and who triggered the escalation
and why. The planner's response is a non-empty analysis and exactly one of three decisions:

- `revise`: changed or new stage instructions/acceptance and/or pending stages to insert
  before the current stage (for example, to move a test onto fixtures first). The engine
  applies the revision through the standard plan publication path. If only the current
  stage changes, the run retains its approval and stage execution restarts with replenished
  rounds. If later pending stages change, new stages are inserted, or the revision touches
  plan review scope, the plan returns to draft status for user approval; the stage does
  not commit and work is kept.
- `constraint_wrong`: a stage constraint was misspecified. The planner names that
  constraint, supplies corrected stage instructions/acceptance, and explains why the
  delivered work meets the corrected requirement. Forge applies the corrected text to the
  current stage (keeping its approval), and the stage is reviewed again under the new
  instructions without discarding the working tree.
- `refused`: the stage can be done as written. The planner explains how the outstanding
  requests can be satisfied within the current constraints. That explanation is recorded
  and passed to the implementer or fixer for one additional round within the current
  attempt's remaining budget.

When a constraint conflict is reported by any role or triggered by budget exhaustion,
Forge does not call repeated-findings reassessment for that round. Instead, it escalates
to the planner. A reported conflict skips the model reassessment logic and proceeds
directly to escalation.

The same conflict signature after one applied planner correction blocks the stage as
before, with the escalation record (trigger, conflict statement, planner analysis, decision,
outcome and timestamp) kept on the stage or plan review for future visibility. The engine
limits escalation to one planner pass per stage per conflict, across attempts and restarts.
If a planner call fails or if the same conflict returns after its correction was applied,
the stage or plan review blocks with the escalation record attached and a clear reason.
Malformed routing proposal fields are returned to the planner with the validation
error for up to three corrections. The full proposal batch must validate before any
stage receives a replacement proposal; unknown fields are never silently ignored.

### Planning rules for satisfiable constraints

To prevent contradictory or unreachable stage constraints, the planner follows four core rules:

1. **Stage constraints must be satisfiable together.** A stage instruction such as
   "change only documentation" or "change only X" must still allow all project test
   suites to pass. If any test would fail because of the restriction, the stage is
   constrained to fail—a contradiction the planner must avoid or resolve.
2. **Tests reading real project files must be moved onto fixtures first.** Before
   planning a stage that changes documentation, milestones, statuses, README titles,
   or other real project files, the planner finds tests that read those files and
   depend on their current content. If any exist, the planner must include an earlier
   stage to move them onto fixture data before the restricted stage runs.
3. **Committed stages are fixed history.** Reviews and corrections never ask to
   change, amend or rewrite commits of earlier stages. A finding must be satisfiable
   in the current working tree.
4. **Acceptance must be reachable in the working tree.** Every acceptance criterion
   must be one the implementing agent can meet by changing files. Acceptance never
   depends on what the engine produces, such as commit messages, commit boundaries
   or runtime state under `.forge/`. Anything that must be recorded, such as an
   authorisation or an audit result, goes into the stage's `commit` message or into
   a file the stage changes.

Agents must not disable tests, weaken assertions or suppress warnings merely to
pass; intentional exceptions require repository-supported justification. When a
test cannot pass because a stage or plan's intended behavior legitimately changes
what the test expects, the agent must not delete, skip or weaken that test on its
own. Stage implementers and fixers escalate through the engine outcome channel
with status `escalation` and request kind `scope`, naming the test, the behavior
change and the concrete evidence; the plan fixer surfaces it as an architectural
context gap in the final response with the same details. The architect decides
whether the test is updated or removed.

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
only for simple tasks, and `strong` is required for critical/complex work. The
configured-tier and `relative_cost_preference` policy-field rules above govern
these adequate options. Published prices are used for comparison only with
matching explicit `routing_billing_basis`; its default `null` leaves API list
rates informational.

The default providers are Claude for planning, Codex for architecture and the
implementation preference, and Claude for independent review. Role model strings
start empty; bootstrap resolves eligible configured strong entries. Automatic
routing defaults on, stage overrides take precedence over global model pins, and
adequacy/review requirements always apply. Only `model_catalogue` policy is saved
to the user configuration file; other engine settings are process settings.
`auto_push` defaults to `true` (disable it for local-only runs), and
`queue_auto_approve` defaults to `false`.

`POST /api/settings` accepts `review_cadence`, for example
`{"review_cadence":{"architect":"per_plan","reviewer":"per_stage"}}`.
When supplied, it must be an object containing exactly both `architect` and
`reviewer`, each the string `per_stage` or `per_plan`; it replaces the pair,
not one nested key. Missing or extra keys, non-objects and invalid values return
HTTP 400 with an `invalid review_cadence` error, leaving all settings unchanged
even if the same request included other updates. Omitting `review_cadence`
retains its current value. Accepted values are process settings returned as
`settings.review_cadence` by `GET /api/state`; both default to `per_plan` on
engine startup.
Saved attempt cadence and deferred obligations survive independently of those
current settings.

Discovery runs at startup and every 360 minutes; metadata is first considered
after discovery and every 1440 minutes, with a 168-hour TTL. The scheduler ticks
every 30 seconds. Missing/unsupported official documents use a 1-hour negative
cache/backoff doubling to 24 hours. Only the documented official HTTPS sources
and accepted document format are supported; unavailable metadata remains unknown.
`curl` is needed for live official research; the optional Claude bridge needs its
separately provisioned Agent SDK. Neither is required for offline fixture tests.
An unsupported discovery mechanism allows explicit configured unverified choices;
missing executables, authentication failures and known rejected choices block.

Up to three disagreement reconciliation exchanges are allowed. Each stage attempt defaults
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
for file in quickshell/*.qml; do /usr/lib/qt6/bin/qmlformat "$file" > /dev/null; done
QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software \
  /usr/lib/qt6/bin/qmltestrunner -input tests/qml
python3 tests/run_compact_clipboard.py
python3 tests/run_stage_details.py
python3 tests/run_panel_details.py
```

Use an already installed Node binary if a version-manager shim has no selected
version. Qt tool locations depend on the distribution. QML parsing and JavaScript
helper tests are separate from the isolated Qt runtime fixtures above.
Standalone `qmllint` cannot fully resolve the runtime `qs.Commons`/`qs.Ui` imports
and reports the resulting unresolved widget types, plus an existing `enabled`
property shadow warning. None of these commands renders the panel inside the live
Omarchy shell, because that would require loading the changes into the running
Omarchy instance. To check the layout, render the working tree offscreen instead.
Create a temporary directory with a `shell.qml` that instantiates
`quickshell/Panel.qml`, and symlink `Commons`, `Ui` and `services` in it to
`/usr/share/omarchy/shell/*`. Then run
`QT_QPA_PLATFORM=offscreen quickshell -p <dir>` and call `grabToImage` on the
window's content item. Point `apiBase` at a local stub, not at the engine's port,
so the harness cannot send actions. The panel redesign M1 renders made this way are
recorded in `docs/features/panel-redesign/milestones.md`.

HTTP worker fixtures have a five-second deadline. On a loaded machine or in a
review sandbox, use `cargo test --offline -- --test-threads=2` to reduce test
contention. A focused mock-architect test in a fresh PID namespace can also hit
the existing short-ID slicing panic in `src/architect.rs`: the formatter slices
the variable-length ID before padding it. Use the complete suite for sandbox
validation; a passing suite does not repair that focused-fixture limitation.

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

If Git commits a stage under an approved or deferred gate but checkpoint publication
fails, the next Run can recover that stage's commit without another implementation
or stage review. Deferred obligations still require plan review before run completion.
Recovery requires a valid saved gate and its stage-required approvals, a clean
index/worktree, the exact gated tree, a single parent matching the reviewed HEAD,
and the intended commit message. Other HEAD changes or unreviewed edits are
rejected. For offline recovery only, stop the
engine service first, then run `forge-engine /path/to/project --recover-committed`;
this command restores matching completion state without running subsequent stages.
