# ADR-0004: Run Planners as Constrained CLI Processes

- Status: Accepted
- Date: 2026-08-29
- Decision owners: Project maintainers

## Context

The first planning workflow must let either Codex CLI or Claude Code CLI inspect a selected repository and propose explicit tasks with acceptance criteria. The orchestrator must use the developer's existing authenticated installations, preserve enough evidence to explain a result, and remain in control of process lifetime and durable workflow state.

Planning is an analysis operation. A planner does not need permission to modify the repository, and its prose cannot be treated as authoritative workflow state without validation. The two CLIs expose different non-interactive command shapes and output envelopes, but both installed versions support constrained execution and structured JSON output.

Prompts and results may contain private repository information. Passing prompts as command-line arguments would also expose them through the local process list. Unbounded output, interactive permission requests, or a planner process that outlives its owning run would make the engine unreliable.

## Decision

The Rust engine will launch Codex CLI and Claude Code CLI through separate adapters behind one planner contract. The user will choose the planner in the initial workflow; automatic routing remains future work.

Each adapter will:

- use the existing authenticated CLI installation rather than a direct model API;
- set the selected repository as the working directory;
- send the prompt through standard input instead of a command-line argument;
- select the CLI's non-interactive mode and a read-only or planning permission mode;
- request the same versioned structured plan schema;
- avoid selecting a model so the developer's authenticated CLI configuration remains authoritative;
- capture bounded standard output and standard error separately;
- use the operating-system exit status as the authoritative process result;
- enforce a timeout and terminate the child when the engine cancels or abandons the invocation;
- validate the structured result in Rust before it can become current run state.

The initial plan schema will contain a summary and an ordered list of tasks. Every task will have a title, description, acceptance criteria, and dependencies expressed as earlier task positions. Rust validation will reject empty, oversized, cyclic, forward, or otherwise malformed plans.

SQLite will record the planner identity, exact prompt, invocation status, bounded final output, bounded diagnostic output, exit code, validation failure, and timestamps. A successful proposal, its tasks, acceptance criteria, dependencies, and later human revisions will be stored transactionally with audit events. Human edits and reordering will create new plan revisions instead of overwriting history.

A shell reload cannot interrupt planning because QML never owns the child process. If the engine exits during planning, startup recovery will mark the interrupted attempt and run as failed so the user can inspect and retry it rather than seeing a permanently running state.

This ADR does not select models, automatic routing policy, cross-agent plan challenge, session reuse, usage-limit behavior, or implementation-agent permissions.

## Consequences

### Positive

- Both supported agents participate through one validated workflow model.
- Planning cannot modify the selected repository through the permissions granted by the orchestrator.
- CLI authentication and user-selected defaults remain outside the engine.
- Process failures, malformed output, and interrupted runs become explicit durable evidence.
- Plan revisions preserve what the agent proposed and what the user changed.
- QML remains a thin client and can safely reload during planning.

### Negative

- The adapters must track two evolving CLI command and output contracts.
- Structured output can still fail validation and require a retry.
- Bounded captured output may truncate diagnostics; larger streaming artifacts need a later design.
- A read-only planner can still encounter untrusted repository instructions and sensitive repository content.
- Robust cancellation across abrupt engine termination will eventually need service-level process-group ownership.

### Follow-up

- Add capability checks that explain when either CLI is missing or unauthenticated.
- Add streamed, structured activity events without making raw output the primary UI.
- Define prompt-injection and repository-content trust policy before granting implementation permissions.
- Decide when planner sessions may be resumed instead of using ephemeral invocations.
- Add a second-agent plan challenge after the basic human approval loop is dependable.

## Alternatives Considered

### Direct model APIs

Direct APIs would offer a uniform schema but introduce separate credentials, provider clients, billing paths, and behavior outside the required existing CLI installations.

### Interactive terminal sessions

Driving interactive terminal UIs would be fragile, difficult to observe structurally, and likely to expose prompts or permission requests that the engine cannot resolve safely.

### Let planners edit the repository while planning

This would blur the human approval boundary and could change user files before a plan is inspected. Implementation belongs to a later isolated-worktree stage.

### Store only the accepted plan

Discarding attempts and revisions would hide failures and human decisions that are required for traceability and recovery.

## References

- [ADR-0001](0001-separate-engine-from-shell-frontend.md)
- [ADR-0002](0002-use-versioned-json-over-a-unix-socket.md)
- [ADR-0003](0003-store-state-in-sqlite.md)
- [Product workflow](../../README.md#core-workflow)
- Installed `codex exec --help` from Codex CLI 0.150.1
- Installed `claude --help` from Claude Code 2.1.247
- [Official OpenAI non-interactive mode documentation](https://learn.chatgpt.com/docs/non-interactive-mode)
