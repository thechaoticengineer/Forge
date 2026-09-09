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

The clipboard fixture exits and prints `EXACT_COPY_PASSED (5 originals)` on
success. It changes only the isolated offscreen clipboard. The QtTest fixture
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

Stage cards reuse `CompactDetail` separately for commit text, instructions,
acceptance, review-policy rationale, both model-selection rationales and model
explanations. Current aggregate gate, separate architect/independent outcomes,
activity, model identity/availability, routing failures and historical labels
remain independent, wrapping status text. Clicking the stage header or using the
existing stage shortcut reveals acceptance and review history; each prose field
has its own expansion. Only the header owns the stage pointer handler, so text
selection and nested controls do not collapse the card. Committed editors stay
locked, including while editing the rest of a plan.

Prose expansion uses project, project visit, plan identity, revision, stage and
field identity. Checkpoint publications and appended reviews therefore preserve
open commit, instructions, acceptance, policy and model details even when stage
delegates are recreated. Review rows retain their separate snapshot scope so an
open review cannot transfer to a different publication.

`ReviewView.js` owns lazy review retrieval through the existing
`/api/architecture/reviews` endpoint. Requests include explicit `project`,
`stage_id`, available `plan_id` and `checkpoint`, `cursor`, and `limit`. Revision
is a client scope/response guard, not an endpoint query parameter. Loading a
shortened review seeks to its absolute position; older reviews load eight at a
time, following `next_cursor` if the byte budget ends a page early. A failed
continuation does not advance the contiguous older-page boundary past a gap.

The additive response fields `project`, `revision`, and `snapshot` identify the
returned publication. The read-only state projection adds `review_snapshot` to
each stage. Indexed histories use their immutable file identifier. Legacy
histories use SHA-256 over all complete records (including unknown fields); an
empty history has the marker `empty`. Both readers use the existing SHA-256
utility. Cached legacy state retains its original marker without hashing the
shortened projection again. No records are rewritten by reads. The existing
8-record / 4096-serialized-byte preview bounds and whole-record page budgets
remain unchanged. Last-verdict-only legacy histories are also readable through
the endpoint.

Cache and request ownership include project visit, plan, revision, stage,
checkpoint and history marker. Durable record identities are checked when
available; otherwise the recent preview at index `i` maps to
`review_count - preview_count + i` only after the endpoint verifies the same
snapshot. Summary strings and shared round numbers never establish identity.
With no checkpoint, a changed full-history marker rejects the response and
refreshes state; assembly restarts in the new scope. An old server without either
a checkpoint or a snapshot marker cannot authorize positional mapping. The panel
reports that it cannot verify the history rather than attaching full text to an
unverified preview. Stale success and failure callbacks cannot modify the view.

A shortened preview explicitly says it is a preview. Expansion displays loading,
failure and retry controls, with no editor or full-copy action until a complete,
verified record arrives. Each full summary, individual request, legacy note and
check then has a separate selectable editor and an exact original copy source.
Current gates remain independent of historical review approvals.

Additional verification:

```sh
node --test tests/panel_review_details.test.mjs tests/panel_review.test.mjs \
  tests/panel_routing.test.mjs tests/panel_lifecycle.test.mjs
python3 tests/run_stage_details.py
```

The stage fixture extracts the current Panel stage-content subtree, compact
wrapper and request/status helpers, substituting only theme, clipboard and
viewport services. It executes actual delegates and controls offscreen: narrow
widths, independent expansion, selection/copy, lazy loading, failure/retry,
complete long reviews and individual requests, older pagination, stale callbacks,
and committed read-only presentation. Stage delegates are recreated on plan
replacement; regression checks preserve expanded prose across checkpoint-only
publication and appended reviews, and reset it for project, visit, plan, revision
or stage changes. Clipboard signal data is asserted there;
`run_compact_clipboard.py` separately verifies the actual Quickshell clipboard.
This is isolated stage-subtree evidence, not a running Omarchy-shell interaction
or validation of the remaining stage-4 panel surfaces. Rust endpoint tests pin
same-revision publications, reconstruct large complete histories, detect hidden
legacy changes with identical previews, preserve cached markers, and confirm
that reads leave saved records unchanged.

## Remaining panel details

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

The re-review run passed all 24 panel runtime checks on Qt 6.11.2 (offscreen,
software rendering). At 280/800 pixels respectively, assigning the 100 reports
took 16/15 ms, 1,000 cached navigation lookups took 2/2 ms, and the slowest of five
prepend polls took 23/22 ms. Each collapsed list had 2,203 visual objects and zero
report detail trees. These are fixture measurements, not live-shell benchmarks.

The existing `tests/qml` runtime suite separately validates live/history component
behavior (including 400 loaded rows, filtering, reading anchors and Unicode
elision). `run_compact_clipboard.py` separately verifies exact Quickshell clipboard
round trips for five originals in an offscreen process; the subtree fixtures
assert clipboard signal data, not the desktop clipboard. `run_stage_details.py`
continues to test the stage/review subtree, lazy loading and stale responses.
These are applicable isolated runtime checks, not JavaScript/parsing substitutes.
No live Omarchy shell was restarted or installed into, and no Forge engine was
started or stopped. Full-panel live polling across every concurrent publication,
rotation and project/session transition remains separately unverified; the broader
`compact-risk-view-stability` is not closed by these subtree/component results.
