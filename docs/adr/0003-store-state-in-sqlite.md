# ADR-0003: Store Durable State in SQLite

- Status: Accepted
- Date: 2026-08-29
- Decision owners: Project maintainers

## Context

The orchestrator must preserve repositories, goals, plans, tasks, agent assignments, commands, verification results, reviews, human decisions, retries, and outcomes across engine restarts and shell reloads. These records are related, updated together, queried in several ways, and expected to evolve through schema changes.

The state belongs to the workshop rather than to any repository it operates on. Storing orchestrator data inside a user project could pollute its working tree or cause private run history to be committed accidentally. The Quickshell plugin and CLI must also observe the same authoritative state instead of maintaining separate files.

Agent transcripts, command output, diffs, and analyzer reports may become much larger than the structured metadata used to find and understand them. Treating every large stream as a database value would make routine queries, backups, and migrations unnecessarily expensive.

## Decision

The Rust engine will be the only component that accesses durable storage directly. It will use one user-scoped SQLite database at:

```text
$XDG_STATE_HOME/omarchy-ai-build-orchestrator/state.db
```

When `XDG_STATE_HOME` is unset, the engine will use `$HOME/.local/state`. The application state directory will be owner-only, and the database and created artifacts will not be accessible to other users.

The project will use `rusqlite` without an ORM. Numbered SQL migrations will be embedded in the storage crate and applied transactionally before the engine begins serving clients. Foreign-key enforcement will be enabled. The database will use write-ahead logging and full synchronous durability unless operational evidence justifies a documented change.

Database work will be serialized through a dedicated storage worker so blocking SQLite calls do not run on Tokio executor threads. Interfaces will request state changes through the engine; QML and CLI code will never open the database themselves.

SQLite will contain normalized current state plus an append-only audit event table. A meaningful state transition will update its current-state records and append the corresponding event in the same transaction. The audit stream supports traceability, but the system will not require full event replay to construct current state.

Large immutable or streaming values, including extensive agent output, command logs, diffs, and analyzer reports, may be stored as files below:

```text
$XDG_STATE_HOME/omarchy-ai-build-orchestrator/artifacts/<run-id>/
```

Artifact files will be written atomically. SQLite will retain their type, relative path, size, checksum, creation time, and owning run. Paths stored in the database will remain relative to the application state directory so the state can be moved or backed up as a unit.

The store must never contain provider credentials or authentication material from Codex CLI or Claude Code CLI. Retention, export, backup, redaction, and secure deletion policies remain separate decisions.

## Consequences

### Positive

- Transactions preserve consistency across related state and audit records.
- SQLite provides durable recovery, indexes, constraints, and migrations without a separate server.
- The engine can efficiently query current work and historical runs.
- Repositories remain free of orchestrator-private state.
- Large artifacts do not bloat ordinary state queries.
- Every interface observes one authoritative store through the engine.

### Negative

- The project must maintain migrations and test upgrades.
- Database corruption, backup, and recovery need explicit operational handling.
- The database and artifact directory must be managed together.
- A dedicated worker adds an internal concurrency boundary.
- Write-ahead logging creates sidecar files that must be considered during backup and shutdown.

### Follow-up

- Implement the storage worker, initial schema, migrations, and permission checks.
- Define artifact thresholds and atomic-write behavior before storing large outputs.
- Add migration, crash-recovery, and engine-restart tests.
- Define retention, export, backup, redaction, and deletion policies.
- Define how a future product rename migrates the state directory safely.

## Alternatives Considered

### One or more JSON files

JSON would be easy to inspect initially but makes coordinated updates, constraints, indexing, schema migration, and crash-safe writes increasingly fragile as the run model grows.

### Full event sourcing

An event log can preserve excellent history, but requiring every current view to be rebuilt from events adds projection, replay, versioning, and repair complexity that the first workflow does not need. Transactional current state plus audit events provides traceability with a simpler recovery model.

### State inside each Git repository

Repository-local state would make discovery simple but risks dirtying user projects, leaking private prompts or logs into commits, and splitting global history across repositories.

### Embedded key-value storage

A key-value store avoids SQL but does not naturally enforce the relationships among runs, plans, tasks, commands, reviews, and artifacts. The product needs relational queries and migrations more than arbitrary key throughput.

### A separate database service

PostgreSQL or another service would add installation, lifecycle, credentials, and operational work without providing useful benefits for a single-user local engine.

## References

- [ADR-0001](0001-separate-engine-from-shell-frontend.md)
- [ADR-0002](0002-use-versioned-json-over-a-unix-socket.md)
- [Product state and traceability vision](../../README.md#state-and-traceability)
