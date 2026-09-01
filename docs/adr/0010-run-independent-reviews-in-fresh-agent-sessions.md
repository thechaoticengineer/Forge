# ADR-0010: Run Independent Reviews in Fresh Agent Sessions

- Status: Accepted
- Date: 2026-09-01
- Decision owners: Project maintainers

## Context

An implementation process cannot establish that its own change is correct. The
workflow needs a separate reviewer with explicit evidence, durable findings,
and no inherited conversational assumptions. Codex CLI and Claude Code CLI are
the supported agent boundaries, but either command may be unavailable at review
time. A same-provider retry is weaker than cross-provider review, yet a fresh,
non-persistent process still provides more independence than self-review inside
the implementing session.

Review approval must also remain distinct from deterministic verification and
from permission to commit, merge, push, deploy, or perform destructive actions.

## Decision

Every review will run as a new, read-only, non-persistent CLI process. The
implementing process can never review or approve its own output.

The engine supports two explicit policies:

- `cross_provider_required` routes Codex implementations to Claude and Claude
  implementations to Codex. If the other provider cannot launch, review fails.
- `cross_provider_or_fresh_session` tries the other provider first. Only when
  that CLI cannot launch may the engine fall back to a fresh process from the
  implementer's provider. A real verdict, timeout, nonzero exit, malformed
  response, or review finding never triggers fallback.

The fallback is stored as `fresh_session_fallback`; it is never represented as
cross-provider review. Both policies persist the implementation attempt,
implementer, reviewer, policy, independence level, exact prompt, bounded output,
structured verdict, findings, failure evidence, and timestamps.

The reviewer receives the approved goal, task, acceptance criteria, recorded
base revision and branch, and bounded Git status and patch evidence. It does not
receive the implementer's conclusions as authority. The schema permits three
verdicts: `approved`, `changes_requested`, and `blocked`. Approval cannot contain
a critical or major finding. `changes_requested` requires at least one concrete
finding. Product, architecture, security, destructive-operation, or insufficient
evidence questions must be blocked.

An approved verdict is technical task acceptance only. It does not imply that
deterministic checks passed and does not authorize a commit, merge, push,
deployment, worktree retirement, or destructive action. Those boundaries require
their own recorded policy and evidence. Until deterministic verification exists,
the prompt explicitly tells the reviewer that verification evidence is absent.

## Consequences

### Positive

- Implementers cannot silently approve their own work.
- Cross-provider review is the normal and strongest available reasoning path.
- Same-provider fallback is explicit, fresh, durable, and policy-controlled.
- Structured findings can drive later correction loops without parsing prose.
- A review verdict cannot cross Git or deployment boundaries by implication.

### Negative

- Provider identity is the enforceable boundary; the engine still does not
  select or attest the provider's configured model version.
- A fresh same-provider session may repeat model-family assumptions.
- Review is useful but incomplete until deterministic verification evidence is
  available.
- Large patches are bounded, so the reviewer may block and request more evidence.

### Follow-up

- Feed deterministic command evidence into the review prompt.
- Present verdicts and findings in the Omarchy Review section.
- Route accepted findings into an explicit implementation continuation.
- Add an opt-in approval policy for local commits only after verification and
  final-diff inspection exist.

## Alternatives Considered

### Let the implementing session review its own output

This preserves the same context and assumptions and violates the independent
review boundary.

### Always use the same provider in a new session

This is operationally simple but discards the stronger cross-provider path when
both authenticated CLIs are available.

### Fall back after any unfavorable review outcome

Retrying another reviewer after concrete findings would enable verdict shopping.
Fallback is therefore limited to inability to launch the preferred reviewer.

### Treat review approval as permission to commit or push

Review is reasoning evidence, not deterministic verification or authorization
for consequential Git and external operations.

## References

- [ADR-0004](0004-run-planners-as-constrained-cli-processes.md)
- [ADR-0007](0007-supervise-implementation-agents-in-task-worktrees.md)
- [ADR-0009](0009-control-and-continue-running-implementations.md)
- [Core workflow](../../README.md#core-workflow)
