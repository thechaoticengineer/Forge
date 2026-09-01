# ADR-0009: Control and Continue Running Implementations

- Status: Accepted
- Date: 2026-09-01
- Decision owners: Project maintainers

## Context

ADR-0008 lets the user observe and cancel one supervised implementation, but a
long-running agent cannot yet be paused or corrected without abandoning its
history. Codex CLI and Claude Code CLI are launched non-interactively and their
standard input is closed after the initial prompt. Treating that input as a
reliable live conversation channel would couple the engine to undocumented
provider behavior and could lose an intervention during a process or frontend
restart.

The engine already owns the implementation process group, durable attempt
state, and local IPC boundary. Intervention controls must preserve those
boundaries, stop descendants together, and make it clear which instructions
produced each filesystem change.

## Decision

The engine will support four explicit implementation controls: pause, resume,
redirect, and add context.

Pause sends `SIGSTOP` to the supervised implementation process group. Resume
sends `SIGCONT`. The engine records the paused state only after the signal has
been accepted, and the implementation timeout excludes time spent paused. A
frontend disconnect or reload does not resume or cancel the process. Because a
paused process is still owned by the live engine, startup recovery treats an
attempt left running or paused as interrupted and failed; it does not claim
that pause survives an engine or machine restart.

Stop requests are arbitrated against the supervisor's terminal transition. An
accepted stop reason owns the durable outcome even when process exit or an
infrastructure failure becomes ready concurrently; a request arriving after
supervision closes is rejected instead of being reported as accepted.

Redirect and add-context requests do not inject text into a running provider
process. The engine first records the continuation kind and exact user
instruction durably on the current attempt, then terminates its process group
through the same graceful-then-forced boundary as cancellation and starts a
linked continuation attempt in the same task worktree with the same agent.
Starting the linked attempt consumes the pending instruction in the same
transaction that creates the child attempt. If that child fails or the engine
stops during the handoff, recovery restores and surfaces the consumed
instruction on its parent for an explicit retry instead of launching work
automatically. A newer continuation already reserved on the child takes
precedence. If an ordinary cancellation wins the stop race, its transaction
clears any losing continuation reservation so cancelled work cannot later be
revived accidentally. The continuation prompt includes the approved task,
current worktree boundary, the user's new instruction, and the fact that
partial changes from the preceding attempt must be inspected before further
editing.

A redirect is an instruction that changes the current approach. Added context
supplements the approved task without changing it. Both are bounded, non-empty
user inputs and become durable attempt metadata. Each continuation names its
parent attempt and kind. The stopped attempt remains `cancelled` with a
specific stop reason; it is not rewritten or deleted. At most one attempt per
run remains active throughout the handoff.

Pause, resume, redirect, and add-context are versioned engine requests shared
by the CLI and QML panel. Consequential controls name the run and attempt they
target. The panel exposes their current state and requires an explicit text
submission for continuation instructions. Raw provider output remains
diagnostic evidence rather than a conversational control channel.

This decision does not define provider session reuse, changed-file tracking,
deterministic verification, independent review, or final approval.

## Consequences

### Positive

- Pausing affects the whole supervised process tree and does not consume the
  configured implementation timeout.
- Redirects and added context work consistently across both supported CLIs.
- Every intervention is attributable to a durable attempt and exact prompt.
- A crash during continuation handoff retains a retryable user instruction.
- Shell reloads and client disconnects do not own process lifetime.
- Partial work remains available in the same isolated worktree.

### Negative

- Redirecting or adding context starts a new provider invocation and may lose
  provider-side conversational context.
- A paused attempt cannot be restored after the engine exits.
- Stopping before continuation introduces a short handoff during which no
  implementer is running.
- The continuation agent must inspect partial filesystem changes to reconstruct
  relevant context.

### Follow-up

- Present linked attempt history, prompts, stop reasons, and retries together.
- Add changed-file summaries at implementation checkpoints.
- Revisit provider-native resumable sessions only after both CLI contracts are
  stable and can preserve the same safety boundary.

## Alternatives Considered

### Keep standard input open and write new instructions to it

The supported non-interactive CLI modes do not define a shared live-input
protocol. Input could be ignored, interpreted inconsistently, or lost while
the frontend believes it was accepted.

### Pause only the direct child process

Descendants could continue modifying the worktree while the interface reports
the implementation as paused. Signalling the process group matches the
existing cancellation boundary.

### Mutate the current attempt's prompt after a redirect

That would erase which instruction launched the process and make later output
and filesystem changes impossible to attribute reliably.

### Resume a paused process automatically after engine restart

The replacement engine cannot safely prove ownership of the original process
group. Startup recovery therefore fails the interrupted attempt and leaves the
worktree for inspection or an explicit continuation.

## References

- [ADR-0001](0001-separate-engine-from-shell-frontend.md)
- [ADR-0002](0002-use-versioned-json-over-a-unix-socket.md)
- [ADR-0003](0003-store-state-in-sqlite.md)
- [ADR-0007](0007-supervise-implementation-agents-in-task-worktrees.md)
- [ADR-0008](0008-persist-bounded-implementation-activity-and-cancel-process-groups.md)
- [Core workflow](../../README.md#core-workflow)
