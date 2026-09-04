# ADR-0017: Read Project Verification Policy from the Task Base Revision

- Status: Accepted
- Date: 2026-09-04
- Decision owners: Project maintainers

## Context

Deterministic verification is the gate that keeps agent claims out of the
approval decision. Until now the engine inferred that gate: it looked for
`Cargo.toml` and `manifest.json` in the task worktree and ran a fixed list of
Cargo and Omarchy commands. That is a reasonable default for this repository
and wrong for almost every other project. A project with a different build
system, an integration suite, a schema check, or a documentation build has no
way to say so, and a project whose checks are slower or faster than the fixed
fifteen-minute bound has no way to say that either.

The obvious fix is a configuration file. That introduces a second problem the
initial scope has to answer: the implementing agent has write access to the
task worktree. If the engine read verification policy from the working tree, an
agent that cannot make the tests pass could instead edit the file that decides
what "pass" means, and the run would look green. Deterministic verification
would stop being independent of the agent it verifies.

Both parts of that answer are architectural: what a project is allowed to
declare, and which copy of that declaration the engine trusts.

## Decision

A project declares its deterministic checks in `.orchestrator/verification.json`
at the repository root. The document is versioned JSON, consistent with the
plugin manifest and with ADR-0002:

```json
{
  "schemaVersion": 1,
  "commands": [
    {
      "label": "Rust tests",
      "program": "cargo",
      "arguments": ["test", "--workspace"],
      "workingDirectory": "crates",
      "timeoutSeconds": 900,
      "required": true
    }
  ]
}
```

`label` names the evidence and must be unique. `program` and `arguments` are
executed directly without a shell. `workingDirectory` defaults to the worktree
root and must stay inside it. `timeoutSeconds` defaults to 900 and is bounded
at 3600. `required` defaults to true; a check declared `required: false` is
advisory — it runs, its result is recorded and shown, but its failure does not
fail the attempt. At least one check must be required: an entirely advisory
policy would pass every task, and no path through the engine may reach approval
without a real gate.

**The engine reads this file as it was committed at the task worktree's base
revision, not as it exists in the worktree.** An implementing agent may edit
the file, and that edit will be part of the change a human reviews and
approves, but it does not take effect for the run that produced it. Policy
changes are reviewed like any other change before they gate anything.

Resolution has exactly three outcomes:

- the base revision contains a valid policy — those checks run;
- the base revision has no policy — the previous detected Cargo and Omarchy
  commands run, so existing projects keep working;
- the base revision contains a policy the engine cannot use — the attempt is
  an infrastructure error naming the field and the reason.

The third case never falls back to detection. A project that declared its gates
must not be silently verified by a weaker set of checks it did not ask for.

An infrastructure error also stops the automatic correction loop instead of
feeding it to the implementer. An unusable policy or a missing tool is not
something the implementing agent can correct, and asking it to try invites it
to remove the checks. That is a decision for the user.

## Consequences

### Positive

- Any project can define its own gates without the engine guessing a toolchain.
- Per-check timeouts replace one fixed bound for every command.
- Advisory checks make a slow or flaky signal visible without blocking a task.
- An agent cannot widen, weaken, or delete the gates that judge its own work.
- An unusable policy is a visible stop, not a quiet downgrade.

### Negative

- A project that adds or fixes its policy needs one committed change before it
  takes effect; the task that writes the file is verified by the old policy.
- Reading policy from Git makes verification depend on a valid base revision.
- Declared commands still run with the developer's full user permissions. The
  policy file is committed, human-reviewed configuration, not a sandbox.
- Detection remains as a fallback, so two paths must be maintained until every
  project declares its own checks.

### Follow-up

- Show raw per-command output and the policy source in the panel's contextual
  drill-down.
- Decide whether campaign-level policy can add checks a single task cannot
  remove.
- Support verification cancellation, which is still planned in Phase 3.
- Reconsider the shape of this file if ADR-0016 removes linked worktrees; the
  base-revision rule survives that change, since it only needs a committed
  revision to read from.

## Alternatives Considered

### Read the policy from the task worktree

This is simpler and lets a task fix its own broken policy immediately. It also
lets the implementing agent decide what counts as success, which contradicts
the rule that an agent is never the sole judge of its own work.

### Ask the planning or reviewing agent to infer the checks

An agent can usually guess a project's build and test commands. It can also
guess wrong, quietly, and the failure mode is a green run. Deterministic gates
must not depend on inference.

### Use TOML instead of JSON

TOML is idiomatic for Rust projects, but the repository already exchanges
versioned JSON over the socket and ships a JSON plugin manifest. Keeping one
document format avoids a dependency and a second parser.

### Let a project define policy in engine configuration instead of the repository

Engine-side configuration would not need a commit to change, but it would not
travel with the project, would not be reviewable in the project's history, and
would let one developer's local settings decide whether a change is verified.

## References

- [ADR-0002](0002-use-versioned-json-over-a-unix-socket.md)
- [ADR-0011](0011-gate-local-task-commits-on-verification-and-independent-review.md)
- [ADR-0016](0016-simplify-the-single-task-workflow.md)
- [Roadmap](../../ROADMAP.md)
