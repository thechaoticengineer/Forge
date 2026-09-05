# Forge

A minimal wrapper around AI coding agents (Claude Code, Codex CLI) for
building software — including Forge itself — with almost no ceremony.

## The loop

1. Point Forge at a git repository and describe a goal.
2. A planner agent (Claude Code or Codex) writes a staged plan to
   `.forge/plan.json` — each stage has instructions, acceptance criteria,
   and a proposed commit message.
3. You mark the plan OK.
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
verdict, and git action is visible in the UI and kept in
`.forge/history.jsonl`.

## Run

```
python3 forge.py
```

Open <http://localhost:8734>. No dependencies — Python 3 stdlib only.
Requires `claude` and/or `codex` CLIs on PATH, logged in.

Pick planner/implementer/checker tools (and optional model), set the
project path, type the goal, create the plan, approve, start.
