# Forge

A minimal wrapper around AI coding agents (Claude Code, Codex CLI) for
building software — including Forge itself — with almost no ceremony.

Rust engine + Quickshell (Omarchy) panel.

## The loop

1. Point Forge at a git repository and describe a goal.
2. A planner agent (Claude Code or Codex) writes a staged plan to
   `.forge/plan.json` — each stage has instructions, acceptance criteria,
   and a proposed commit message.
3. You mark the plan OK in the panel.
4. Forge runs each stage automatically:
   - the **implementer** (one tool) implements the stage,
   - an independent **checker** (the other tool, always a fresh session)
     reviews the uncommitted diff and writes a verdict,
   - rejections loop back to the implementer with the checker's issues,
     up to a bounded number of fix rounds,
   - an approved stage is committed with the proposed message.
5. After the last stage, Forge pushes to `origin`.

If the checker still rejects after the fix rounds, the stage is marked
blocked and Forge stops for you. Full history of every agent session,
verdict, and git action is visible in the panel and kept in
`.forge/history.jsonl`.

## Run

```
cargo run [/path/to/project]
```

The engine serves a JSON API on `http://127.0.0.1:8734` for the panel
(also usable with `curl`). Requires `claude` and/or `codex` CLIs on
PATH, logged in.

The Omarchy plugin (`manifest.json`, `quickshell/`) provides the bar
widget and the Forge panel: pick planner/implementer/checker, set the
project path, type the goal, create the plan, approve, start.
