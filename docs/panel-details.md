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
