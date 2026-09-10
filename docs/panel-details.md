# Compact live and history details

`CompactDetail.qml` takes an unchanged `originalText`, separate metadata, and
externally owned expansion state. `DetailText.js` chooses the first line with
non-whitespace content, preserving that line's whitespace and handling LF, CRLF,
and CR. Empty previews show `(empty text)`. Width-based QML elision applies only
to the one-line plain-text preview. Every entry has a focusable Expand/Collapse
button, including empty values and long single lines. Expanded text is read-only,
selectable, wrapped and scrollable, and is built only while a row is expanded.
Copy full text sends the original string to `Quickshell.clipboardText`; it never
uses rendered text or shell commands.
Escape returns focus to the panel's normal shortcuts. While a detail has focus,
selection and copy keys cannot trigger panel actions.

Two layout guards bound the cost of one very long line without narrowing what is
readable or copyable. Qt lays out a text line at worse than linear cost, so a
single 200,000-character message previously blocked the interface for over two
minutes; both guards keep that under a fifth of a second. The preview hands the
renderer a bounded slice of the chosen line. No character count can guarantee
that a slice overflows a width, because zero-width and combining characters
render at no advance at all, so the slice is only a starting estimate: while the
`Text` reports itself not truncated and characters remain, `CompactDetail` grows
the slice and lays it out again until either the renderer truncates it or the
whole line is laid out. `Text.ElideRight` therefore always chooses the visible
cut. Ordinary text stops after the first layout, and growth is geometric, so the
cost stays within a small factor of the shortest sufficient layout; the worst
case, a line of 200,000 zero-advance characters that must be laid out in full
before anything is visible, settles in under a fifth of a second. Growth reads
`truncated` and the laid-out metrics rather than `onLineLaidOut`, because
installing that handler switches `Text` to custom layout and disables eliding.
The expanded editor keeps word wrapping for ordinary text and wraps at any
character once a line exceeds 1000 characters. Neither guard touches
`originalText`, the expanded document, or the copy source.

Live output consumes version 1 of `/api/agent_records`. `DetailView.js` scopes
requests by project, project view revision, invocation, feed session, and request
generation. Only the owning callback can finish a pending request. Returned
cursors are used even at the current end, pages append incrementally with ID
deduplication, and polling continues through idle to collect terminal output.
A new feed session resets live entries. A project change bumps the view
revision and resets both lists, expansion, cursors and reading anchors, so an
earlier visit's response cannot append to the project now shown. Finishing a run
no longer switches away from live output, because the terminal page can arrive
after `busy` clears; opening the panel still chooses the tab from `busy`.
Pre-upgrade legacy output remains one opaque entry; refreshes of its changing
source are deferred while expanded so selection and copying use the inspected
original until collapse.

The history reader adds an opaque `id` to each returned history object without
rewriting the file or changing `event.text`. IDs combine source device, inode,
available birth timestamp, and absolute record byte offset. Identical adjacent
records are distinct; append and the bounded tail moving preserve identities;
file replacement/rotation changes source identity. This follows Forge's
append-only history writer, not external in-place rewriting of old records.
Chat and report response shapes remain unchanged.

`DetailList.qml` retains row delegates and their unchanged source bindings across
polling and filtering. Rows already seen in the current project remain available
when they leave the server's bounded history tail. Filtering hides them without
destroying editors. This intentionally keeps the current view's loaded rows in
memory until its project/session reset. Expansion lives in the retained model,
keyed by scoped identity; it is never attached to a filtered position. Reading
anchors use entry identity plus offset. Inspecting or selecting suspends tail
movement; ordinary output shortcuts and explicit navigation to the tail remain
available. Goal separators, timestamps, kinds, and error coloring are separate
from message previews.

Focus and the viewport stay together. When a control takes focus for any reason
other than a pointer press — Tab, Backtab, or programmatic focus, including the
copy control and editor inside an expanded row — the list scrolls the smallest
amount that shows it and re-anchors reading there, so the following poll keeps
it in place. A control taller than the viewport only has to reach its top edge.
A pointer press is excluded deliberately: focus then lands where the user is
already looking, and moving the view would break a drag selection.

## Isolated verification

```sh
node --test tests/panel_compact.test.mjs
QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software \
  /usr/lib/qt6/bin/qmltestrunner -input tests/qml
python3 tests/run_compact_clipboard.py
```

The clipboard fixture exits and prints
`EXACT_COPY_PASSED (5 originals, CompactDetail and StageProse)` on success. It changes only the isolated offscreen clipboard. The QtTest fixture
exercises actual mouse/keyboard controls, elision, exact copy signal data,
selection retention across reconciled polls/filtering/resizing/appends, reading
anchors, tail behavior, long-line responsiveness, a 400-row history, width-based
elision of zero-width and combining Unicode, and Tab/Backtab traversal crossing
the viewport boundary. That is component evidence, recorded separately from
Panel integration.

Panel integration is exercised in `tests/panel_compact.test.mjs`, which runs
`refreshAgentLog`, `syncHistory` and the project-change handler taken from
`Panel.qml` itself: feed assembly, stale callback rejection, session reset, and
leaving and revisiting a project. Rust history tests cover duplicate records,
tail movement, rotation, absolute byte offsets, and incomplete records. These
fixtures do not load the panel into the user's live shell and do not constitute
full-panel live polling validation.

## Stage details and complete reviews

A read-only plan stage card has exactly one disclosure toggle in its header,
which expands or collapses the whole card. The selected stage's Enter / o / Space
shortcut does the same thing; at most one stage is expanded. A collapsed card
contains only that header and short, wrapping plain-text status lines: stage
status and sha, current activity, review policy and gate (including architect and
independent outcomes), the latest historical review line, elapsed time, model
identity and effort, availability verification, tier provenance, and routing or
block errors. There are no preview lines, per-field Expand controls or editors.
Committed stages keep this read-only card even while the rest of a plan is edited;
the existing editability rule is unchanged.

An expanded card shows Commit, all model-agreement rationale (planner, architect,
trigger evidence and retained routing-history rationales), review-policy rationale,
Instructions, Acceptance criteria and historical reviews as complete, wrapping,
selectable plain text through `StageProse.qml`. There are no per-field disclosure,
load or copy controls. Ctrl+C copies exactly the selected substring of the
original, including whitespace and original CR/CRLF line endings; Ctrl+A selects
all. Qt normalizes line endings in the displayed editor, but the selection maps
back to the original for copying. Escape returns to panel shortcuts; Tab and
Backtab traverse focus, while other keys stay contained in the field. Pointer
focus does not scroll away from a drag selection. The panel viewport scrolls the full prose; ordinary text wraps by word
and unbroken lines longer than 1000 characters wrap anywhere.

The single permitted extra disclosure toggle, **Model agreement and routing
details**, reveals only diagnostics: risk, constraint, cost qualification,
routing price, agreement id and latest invocation. All rationale prose is visible
whenever the card is expanded, regardless of this toggle.

Opening a card automatically requests completion of shortened review previews
when needed, outside plan-edit mode. Already complete records do not reload, and
collapsed cards initiate no automatic load. An already-started request chain may
finish its continuation pages after collapse; collapse does not cancel requests.
Until complete verified records arrive, shortened summaries remain selectable
and explicitly labelled **Summary preview · feedback may be omitted**. Complete
records show the full summary and every change request, legacy note and verified
check, alongside the round, role, decision and UTC timestamp. A shortened record
is never presented as complete.

The review group keeps its **Historical reviews · showing N of M** line and one
wrapping loading/failure status line, red on failure. A failed load or history
that cannot be verified exposes a group-level **Retry reviews** action. Automatic
loading stays suppressed through polling, checkpoint/snapshot publications and
collapse/re-expand until Retry or an applicable project, visit, plan, revision or
stage scope reset. Retry uses the failed range when available, otherwise the
current incomplete range or older-page gap; successful completion clears the
failure state. Failures from Retry and older-page actions also suppress automatic
loading. **Load older reviews (N)** pages backwards eight records at a time.
These are group actions, not disclosure toggles or per-review load buttons.

The card-level snapshot-hold captures stage prose when the card opens. Commit,
instructions, acceptance, policy rationale, model rationale and diagnostics stay
bound to that snapshot while expanded, so polling cannot rebind an open editor
or destroy a selection. Status lines keep updating. Newer prose appears on
collapse/re-expand or when the project, project visit, plan, revision or stage
scope changes. Stage delegates retain their identity across ordinary plan-array
replacement.

Historical review presentation is retained separately from current review
verification. Selected review editors survive polling, `syncReviewViews()`,
checkpoint publications, resizing and older-row insertion. Selected text from a
previous publication is labelled as held from that publication; it cannot make
the current scope's preview complete or bypass verification. If a selected
preview finishes loading, the complete record appears separately while the
selected preview keeps its label and original. Releasing the selection permits
reconciliation; collapse/re-expand or a stage-detail scope reset clears the held
presentation. Current review gates remain independent of historical approvals.

`ReviewView.js` owns retrieval through `/api/architecture/reviews`. Requests carry
`project`, `stage_id`, available `plan_id` and `checkpoint`, `cursor` and `limit`;
revision is a client scope/response guard, not a query parameter. Loading seeks
to absolute record positions and follows `next_cursor` when the byte budget ends
a page early. A failed continuation cannot advance the contiguous older-page
boundary past a gap. The 8-record / 4096-serialized-byte preview bounds and
whole-record page budgets are unchanged, including last-verdict legacy fallback.

Request/cache ownership includes project visit, plan, revision, stage, checkpoint
and history marker. Response `project`, `revision` and `snapshot` identify the
publication; stage state exposes `review_snapshot`. Indexed histories use their
immutable file identifier, while legacy histories use SHA-256 over all complete
records, including unknown fields, or `empty` for no history. Cached legacy state
keeps its original marker without hashing shortened projections; reads rewrite
no records. Durable record identities are checked when available; otherwise
preview positions map only after verification of the same snapshot. Summary text
and shared round numbers do not establish identity. A changed marker rejects the
response and refreshes state; durable stage-scoped suppression prevents that
refresh from silently restarting automatic loading. Without a checkpoint or
snapshot marker, the panel reports unverifiable history. Stale success and
failure callbacks cannot mutate the current view.

Chat, live output, history, reports, providers, catalogue and queue keep the
compact per-field expansion described in the earlier sections. The architecture
card is unchanged, including its grouped disclosures and compact fields.

Verification rerun for this documentation stage:

```sh
node --test tests/*.test.mjs
python3 tests/run_stage_details.py
python3 tests/run_panel_details.py
QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software \
  /usr/lib/qt6/bin/qmltestrunner -input tests/qml
python3 tests/run_compact_clipboard.py
```

All five commands exited successfully: Node reported 57 passed, the stage fixture
17 passed, the panel fixture 27 passed, and the offscreen QML suite 32 passed
(including `tests/qml/tst_stage_prose.qml`), with zero failures or skips. The
isolated Quickshell clipboard run printed
`EXACT_COPY_PASSED (5 originals, CompactDetail and StageProse)`, confirming exact
round trips through both components. It also emitted the offscreen platform's
window-mask warning; the clipboard check still completed successfully.

The stage fixture extracts the production card subtree, `StageProseField`,
request/status helpers, stage model and keyboard shortcut, substituting only
host services such as theme, clipboard and viewport. It proves the populated
collapsed subtree has only its header toggle and wrapping status text, with no
previews or editors; expanded prose and diagnostics obey the disclosure limits;
and header/keyboard activation, exact-selection copying and committed read-only
presentation work. It checks editor identity and native selection retention
through polling, checkpoint/appended-review publications, syncing and resizing,
with live statuses, held-preview/source labels and scope resets. Loading checks
cover automatic completion on opening, no initiation while collapsed, allowed
in-flight continuation after collapse, no reload of complete records, durable
failure suppression, unverifiable responses, Retry recovery, older pagination
to the oldest record and stale callbacks. Node protocol tests additionally cover
byte-limited pages and contiguous older-page gaps. The component suite includes
the new stage-prose test for original CR/CRLF selection mapping, keyboard
containment, mouse selection, empty input and long-line wrapping.

These are isolated component and stage/panel-subtree checks, not full-panel live
polling validation. No live Omarchy shell or Forge engine was started or stopped.
The clipboard check uses a separate offscreen Quickshell process.

## Remaining panel details

The architecture overview is a bordered card with always-visible context,
activity and recovery/error fields. Stage guidance, open risks and decisions
have separate labelled disclosure controls with record counts and start
collapsed, so a long decision history does not push the plan offscreen.
Section contents are created on first opening and retained through collapse;
project/plan scope changes reset them. Shared detail buttons use explicit theme
colors and focus outlines instead of inheriting native desktop button chrome.

`PanelDetails.js` presents architecture activity reasons, each guidance record,
each risk, decision summaries/rationales/alternatives/tradeoffs, provider and
catalogue descriptions/errors/provenance, report commits, lifecycle prose and
usage per tool as separate original fields. Their status rows remain wrapping
plain text: recovery/context, decision status/supersessions, source failures,
model identity/availability, report metadata and archived stage titles and
aggregate/architect/independent outcomes. Quota readings and warnings remain
visible separately from expandable error bodies. Errors carry a red `!` marker.
The latest-output summary uses the complete retained feed, because the state
heartbeat's `agent.last_line` is intentionally bounded to 200 characters.

`DetailFields.qml` reuses `CompactDetail` with a retained flat model. Unchanged
polls do not rebind editors; reordered fields retain their delegates. A changed
open original is held until collapse. An inspected field omitted by a later
snapshot remains labelled `previously shown` until collapse; scope changes still
clear it. Chat keys include plan scope, original record and position (duplicates
remain distinct). Each report collection update serializes each immutable record
once, matching its occurrence among exact duplicates to a cached short identity.
Navigation uses an index lookup without serializing records. The identity cache
releases absent originals on the next update. Lightweight report headers stay
alive while scrolling; lifecycle/detail fields are created only on first report
expansion, then retained so outer collapse and polling preserve inspected editors.
A record/offset anchor restores reading after new reports arrive. Report expansion
uses an explicit button. Keyboard focus reveals controls inside nested viewports.

Goal, queue, local/policy/quota/load/discovery errors and policy explanations also
use the compact wrapper. Input editors, structured diff rows and keyboard help
keep their existing purposes. The compact behavior does not change persisted
strings or storage/API boundaries. Legacy opaque logs cannot recover boundaries
or characters discarded before complete records were introduced.

### Stage-4 verification and scope

`python3 tests/run_panel_details.py` extracts the current Panel local-error,
architecture, provider, catalogue, chat, report and queue subtrees and their real helpers and
adapters. Only theme, host, API actions and clipboard services are substituted.
Its offscreen QtTest matrix uses 280- and 800-pixel widths with LF, CRLF, CR,
whitespace-only, Unicode/combining characters, markup-like text and 5,000-character
unbroken originals. It checks actual `Text.truncated`, full-copy signal data,
keyboard focus/traversal, selection, nested controls, independent statuses and
buttons, unchanged editors during repeated polls, chat expansion, report prepend
anchors, and project-scope resets. It also exercises real mouse drag selection
and retaining an inspected error that disappears from a later snapshot.
The local-error regression places the exact production row inside a Column with
no fixture-supplied row width. At both widths it requires a nonzero error preview,
visible red `! Error` indicator, usable expansion/copy buttons and keyboard access.
The report-cap regression loads 100 full multiline reports, requires zero unopened
detail trees and bounded header object counts, and measures assignment, 1,000
cached navigation lookups and five capped prepend polls. It also preserves an
interior report's selected editor and actual viewport anchor through scrolling,
polling and outer collapse/re-expansion. A Node regression separately counts
exactly one serialization per report per update and none during navigation.
Warnings from the extracted subtrees fail the runner.

The documentation-stage rerun passed all 27 panel runtime checks on Qt 6.11.2
(offscreen, software rendering). At 280/800 pixels respectively, assigning the
100 reports took 18/15 ms, 1,000 cached navigation lookups took 3/2 ms, and the
slowest of five prepend polls took 25/23 ms. Each collapsed list had 2,103 visual
objects and zero report detail trees. These are fixture measurements, not
live-shell benchmarks.

The existing `tests/qml` runtime suite separately validates live/history component
behavior (including 400 loaded rows, filtering, reading anchors and Unicode
elision). `run_compact_clipboard.py` separately verifies exact Quickshell clipboard
round trips for five originals through both `CompactDetail` and `StageProse`
in an offscreen process; the subtree fixtures assert clipboard signal data, not
the desktop clipboard. `run_stage_details.py`
continues to test the stage/review subtree, lazy loading and stale responses.
These are applicable isolated runtime checks, not JavaScript/parsing substitutes.
No live Omarchy shell was restarted or installed into, and no Forge engine was
started or stopped. Full-panel live polling across every concurrent publication,
rotation and project/session transition remains separately unverified; the broader
`compact-risk-view-stability` is not closed by these subtree/component results.
