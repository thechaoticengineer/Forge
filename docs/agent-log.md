# Agent log record contract (version 1)

`GET /api/agent_records` uses the same optional `project` parameter and repository
validation as `/api/agent_log`. Omitted projects resolve to the active project;
responses identify the canonical project. The existing `/api/agent_log` endpoint
retains its `log`/`size` response and byte-offset behavior.

For a live feed, fetch cursor zero, then send the returned `session` and
`next_cursor` on subsequent requests. `limit` defaults to 50 and is clamped to
1–100 records. Pages have a soft 256 KiB serialized-record budget. One record
larger than that budget is returned intact, by itself. Never split a returned
record into messages using its newlines.

```json
{
  "version": 1,
  "project": "/path/to/project",
  "session": "engine-invocation-id",
  "current_session": "engine-invocation-id",
  "reset": false,
  "entries": [{
    "version": 1,
    "session": "engine-invocation-id",
    "id": "engine-invocation-id:0",
    "sequence": 0,
    "kind": "message",
    "stream": "stdout",
    "label": "",
    "text": "\nFirst line\r\n  Full original text  \t"
  }],
  "next_cursor": 1,
  "more": false,
  "legacy": false
}
```

`sequence` is zero based; `next_cursor` is the next committed sequence, including
when a page is empty. A nonzero live cursor requires `session`. A cursor beyond
the committed boundary is an error. `more` means another committed record is
available now; keep polling even when false. IDs are immutable within an engine
invocation and independent of provider threads, content, and file lengths. Scope
client caches by project and session as well as entry ID.

An invocation replacement returns `reset: true`, the new session, and its first
page, ignoring the old cursor. Discard the old live model before consuming this
page. Restarting the engine alone preserves the on-disk session; starting a new
invocation creates a new session even when it resumes the same provider thread.

`kind` is `message`, `tool_input`, `tool_output`, `result`, `status`, `error`,
`plain`, or `legacy`. `stream` is `stdout`, `stderr`, or `legacy`. Display `label`
separately; select/copy `text` exactly. Tool command/input fields and output fields
have separate records. Object-valued fields retain complete JSON. Plain streams
retain their actual line terminators, trailing whitespace, and final unterminated
fragment. Provider defensive event/result limits and bounded outcome tails still
apply; those limits return errors rather than successful partial capture.

## Historical and legacy sources

`?history=true&cursor=0&limit=50` lists source manifests in ID order using
`sources`, `next_cursor`, and `more`. Each source has `version`, `session`, `count`,
and `legacy`, plus `incomplete` (an unresolved message publication) and
`startup_interrupted` (startup was resumed after an interruption). These flags
describe different conditions; a recovered startup does not certify message
capture in any previous session. Read a selected source with
`?archive=<session>&cursor=0`; subsequent
archive pages keep that same archive parameter. `current_session` still identifies
the live invocation. Source listing is separate from live incremental reads.

Before the first structured invocation, available `agent.log` text is returned as
one opaque `legacy` entry. It has no recoverable message boundaries. The legacy
source may still be changing under an older engine: re-fetch cursor zero to
refresh it. Invocation startup archives all that text as an immutable legacy
source before replacing `agent.log`. Archived legacy text is also one indivisible
entry, regardless of its size. Prior structured invocations remain available.

## Persistence and failures

Files live under `agent-records/` alongside `agent.log`. Each invocation retains
its own `readable.log` inode; the current `agent.log` is a hard link to it. Startup
replaces the compatibility path with a rename, never truncating the old inode.
The initial readable header remains metadata in the readable log, not an agent
message. Plan resets do not remove log sources.

Each session stores immutable numbered JSON records and a `commit.json` manifest.
Readers address the next record directly; they do not scan all previous text.
Stdout/stderr share one mutex. A filesystem lock also serializes readers and
invocation replacement. Publication orders durable pending marker, readable
bytes, complete record, and committed count, then clears the marker. All files
and publication directories are synced. Unpublished trailing/temp files cannot
advance a cursor. Malformed committed records produce an explicit error.

An interrupted or failed **message publication** leaves a session's `pending.json`
and makes that source return an error rather than claim complete capture. A
failed writer cannot retry and a replaced writer cannot publish. Starting a new
invocation leaves that marker, numbered records, readable bytes, and committed
count unchanged. History lists the source with `incomplete: true`; its readable
file remains available on disk for diagnosis. Completed pinned archives remain
readable even while live startup is unresolved.

## Automatic startup recovery

The root `agent-records/pending.json` governs only publication of the
`agent.log`/`current.json` pair. Live record reads return an explicit error while
it exists; the compatibility API continues reading the available `agent.log`.
The next invocation startup automatically recovers under the same exclusive
filesystem lock used by append and retrieval. No service restart, manual marker
deletion, or repair of historical message records is required.

Before replacing either pointer, startup syncs a journal in
`agent-records/.startup-<session>/`. It contains the transaction identity, the
reserved legacy archive ID when needed, and hard links preserving the previous
readable log and current pointer. When superseding an older marker without
provenance, it also preserves that marker's original bytes. Session directories
and their existing commits and records are never replaced. Journals are retained
as diagnostic evidence and are not additional sources in the paginated history.

Recovery completes the recorded startup, then starts the requested invocation
with a fresh session and header. Legacy migration uses the journal's original
readable snapshot and reserved ID, staging the whole archive outside the history
namespace before publishing it. If that archive was already published, recovery
retains its ID, text, and cursor without rewriting it. It never archives a new
header as another legacy source. For markers from the earlier implementation,
startup recognizes a header replacement by its session's readable inode and a
completed legacy copy by its exact opaque record text.

A stale `agent.log.next`, including a surviving hard link from an interrupted
rename or an unrelated collision (even a dangling symlink), is moved to a unique
`stale-*` evidence path inside the journal and synced before a new replacement
link is created. Existing evidence is never overwritten. The replacement link and compatibility rename
are synced, then `current.json` is atomically published and synced. Only after
that consistent pair is durable is the **root** pending marker removed and its
directory synced. No per-session pending marker is cleared by this process.

If recovery fails or is interrupted, the journal and root marker support the
same retry. An underlying IO problem still needs to be corrected (for example,
restoring directory write access); do not delete pending markers or source files
to bypass it. Success is observable as a normal live response identifying the
new current session, `reset: true` for an older supplied session, and advancing
cursors after new appends. The current readable log is a hard link to that
session's `readable.log`; completed historical sources retain their original
identities and text, and incomplete historical sources remain explicitly so.
