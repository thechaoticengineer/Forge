# ADR-0002: Use Versioned JSON over a Unix Socket

- Status: Accepted
- Date: 2026-08-29
- Decision owners: Project maintainers

## Context

The Rust engine, Omarchy Quickshell plugin, and Rust CLI need one local boundary for commands, state snapshots, and live events. The engine must remain authoritative and continue safely when the plugin unloads or `omarchy-shell` restarts. A reconnecting frontend must be able to recover missed state without reconstructing it from an event stream.

The installed Quickshell `0.3.1` exposes `Quickshell.Io.Socket`, backed by Qt's local socket implementation, together with `SplitParser` and string writes. This gives the QML plugin a supported Unix-domain socket client without launching helper processes or putting orchestration traffic through Omarchy's shell-control IPC.

## Decision

The engine will listen on an owner-only Unix-domain socket at:

```text
$XDG_RUNTIME_DIR/omarchy-ai-build-orchestrator/engine.sock
```

Clients and the engine will exchange UTF-8 JSON objects separated by newlines. Every message envelope will include a numeric protocol version. Requests will carry a client-generated request ID; direct responses will echo it. Engine events may omit a request ID.

The engine will send its current authoritative snapshot when a client connects. Clients will reconnect after interruption and replace their local view with a fresh snapshot before applying later events. Event sequence numbers will allow clients to detect gaps as live state is added.

The socket directory and socket will be accessible only to the current user. The engine will reject unsupported protocol versions and malformed messages without interpreting them as shell commands. Omarchy's documented `omarchy-shell` IPC remains responsible for plugin lifecycle operations such as summon and hide, not orchestration data.

This decision establishes the transport and framing, not the full command vocabulary, event schema, engine activation model, storage format, authentication beyond local filesystem permissions, or compatibility policy for future protocol versions.

## Consequences

### Positive

- Quickshell/QML and Rust can use native local-socket support.
- The boundary has no listening TCP port and is restricted by runtime-directory permissions.
- JSON messages are inspectable during development and easy to represent in QML.
- Snapshot-on-connect makes plugin reloads and shell restarts routine client reconnections.
- The CLI and graphical plugin use the same engine protocol.

### Negative

- JSON and newline framing require message-size limits and careful malformed-input handling.
- Protocol evolution, compatibility testing, and reconnect behavior become maintained product contracts.
- Unix-domain sockets make the initial implementation intentionally Linux-specific.
- Engine discovery and lifecycle still need a separate decision.

### Follow-up

- Define command, event, error, and acknowledgement schemas as workflow capabilities are added.
- Add bounded message parsing and integration tests for malformed and oversized messages.
- Decide how the engine starts, stops, and reports incompatible clients.
- Specify compatibility rules and snapshot/event reconciliation.
- Define durable state storage and recovery separately.

## Alternatives Considered

### Omarchy shell IPC as the orchestration transport

The shell IPC is appropriate for summoning and hiding plugins, but routing engine state through it would make `omarchy-shell` part of the execution path and weaken engine independence.

### D-Bus

D-Bus provides discovery and typed interfaces, but adds interface-generation and integration complexity before the workflow contract is understood. It remains a possible future replacement if desktop service integration outweighs the simpler socket boundary.

### Loopback HTTP or WebSocket

These transports have broad tooling support but introduce port discovery, a larger protocol surface, and browser-oriented machinery that the local QML and Rust clients do not need.

### Engine as a frontend child process over standard I/O

Standard I/O would be simple initially but ties engine lifetime to one interface. Closing or reloading the plugin could terminate work and the CLI could not attach to the same authoritative process.

## References

- [ADR-0001](0001-separate-engine-from-shell-frontend.md)
- [Product vision and architecture boundaries](../../README.md#architecture-boundaries)
- `$OMARCHY_PATH/shell/README.md`
- Installed `/usr/lib/qt6/qml/Quickshell/Io/quickshell-io.qmltypes`
