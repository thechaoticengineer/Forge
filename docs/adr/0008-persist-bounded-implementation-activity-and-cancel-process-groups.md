# ADR-0008: Persist Bounded Implementation Activity and Cancel Process Groups

- Status: Accepted
- Date: 2026-09-01
- Decision owners: Project maintainers

## Context

ADR-0007 established engine-owned implementation processes and durable final output, but clients cannot observe useful progress until an agent exits. The first Omarchy implementation view also needs a safe way to cancel work without making the QML frontend the process owner. Shell reloads and client disconnections must not lose already-observed activity or accidentally stop the agent.

Provider CLIs do not yet expose one stable shared semantic event format. Their standard output and diagnostic output can still be normalized into a small common envelope containing the attempt, task, agent, category, message, ordering sequence, and timestamp.

## Decision

The agent adapters will emit bounded text chunks while continuing to retain their existing bounded final output. The Rust engine will categorize each chunk as normal output or diagnostic output, remove unsafe terminal control characters, split it to a fixed maximum size, and append it to SQLite while the attempt is running.

The authoritative snapshot will contain only the newest bounded activity window for the active run. Every activity record remains durable, so reconnecting clients can replace their view from a fresh snapshot without owning the process or replaying an unbounded transcript. The primary panel will show concise recent activity; complete captured output remains deeper diagnostic evidence.

Cancellation will be an explicit versioned engine request naming the run and running attempt. The engine will signal the registered supervisor, which will terminate the agent's entire process group using the same graceful-then-forced boundary used for timeouts. The store will mark the attempt `cancelled`, retain its partial output and activity, and return the run to `waiting_for_user` so the worktree can be inspected or retried. Cancellation will not delete or roll back files written before termination.

The panel will require a confirmation that identifies the agent and task and explains that partial work remains. A plugin reload, socket disconnect, or abandoned implementation request will not imply cancellation.

This decision does not define pause, redirection, additional context, provider-specific semantic event parsing, changed-file tracking, verification, review, or worktree retirement.

## Consequences

### Positive

- The Omarchy panel can show useful progress without supervising agent processes.
- Activity and cancellation outcomes survive shell and engine client restarts.
- One protocol and engine action serves graphical and command-line clients.
- Cancellation stops descendant processes as well as the direct CLI process while preserving partial work for inspection.

### Negative

- Activity messages are categorized text rather than provider-independent semantic tool events.
- Recent activity is duplicated with final captured output until the artifact pipeline is implemented.
- Persisting each bounded update adds SQLite writes during implementation.
- Cancellation cannot undo filesystem writes already made by the agent.

### Follow-up

- Parse stable provider event formats into richer common activity kinds when their contracts are verified.
- Add redirect and additional-context controls as new attempts or explicit continuation requests.
- Move large immutable raw output to checksummed artifacts.
- Add changed-file summaries and deterministic verification evidence.

## Alternatives Considered

### Stream output only to connected clients

This would lose activity during shell reloads and make reconnect behavior depend on an incomplete transient stream.

### Let QML terminate the agent process

This would violate the engine/frontend boundary and would fail when the panel reloads or disconnects.

### Store the entire transcript in every snapshot

Snapshots are the reconnect and summary contract. Unbounded output would increase IPC memory, parsing cost, and risk inside the long-lived shell process.

### Delete the worktree when cancellation succeeds

Cancellation is a process-lifetime decision, not rejection of partial code. Automatic deletion would destroy inspectable and potentially useful work.

## References

- [ADR-0001](0001-separate-engine-from-shell-frontend.md)
- [ADR-0002](0002-use-versioned-json-over-a-unix-socket.md)
- [ADR-0003](0003-store-state-in-sqlite.md)
- [ADR-0007](0007-supervise-implementation-agents-in-task-worktrees.md)
- [Product state and traceability vision](../../README.md#state-and-traceability)
