#!/usr/bin/env python3
"""Forge — a minimal wrapper around AI coding agents.

Loop: plan (codex/claude) -> human approves -> per stage:
implement (tool A) -> independent check (tool B, fresh session) ->
bounded fix loop -> commit proposed message -> next stage -> push.

Zero dependencies. Run: python3 forge.py  ->  http://localhost:8734
"""

import json
import os
import shlex
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

PORT = 8734
ROOT = Path(__file__).resolve().parent
FORGE_DIR = ".forge"
PLAN_FILE = "plan.json"
VERDICT_FILE = "verdict.json"
HISTORY_FILE = "history.jsonl"

# ---------------------------------------------------------------- state

LOCK = threading.RLock()
STATE = {
    "project": str(ROOT),
    "settings": {
        "planner": "claude",
        "implementer": "codex",
        "checker": "claude",
        "planner_model": "",
        "implementer_model": "",
        "checker_model": "",
        "max_fix_rounds": 3,
        "auto_push": True,
    },
    "phase": "idle",  # idle|planning|plan_ready|running|blocked|done|failed
    "goal": "",
    "current_stage": None,
    "current_step": "",
    "stop_requested": False,
}
WORKER = None


def forge_path(name):
    d = Path(STATE["project"]) / FORGE_DIR
    d.mkdir(exist_ok=True)
    return d / name


def log_event(kind, text):
    entry = {"t": time.strftime("%H:%M:%S"), "kind": kind, "text": text}
    with LOCK:
        with open(forge_path(HISTORY_FILE), "a") as f:
            f.write(json.dumps(entry) + "\n")


def read_history(limit=400):
    p = forge_path(HISTORY_FILE)
    if not p.exists():
        return []
    lines = p.read_text().splitlines()[-limit:]
    return [json.loads(x) for x in lines if x.strip()]


def load_plan():
    p = forge_path(PLAN_FILE)
    if not p.exists():
        return None
    try:
        return json.loads(p.read_text())
    except (json.JSONDecodeError, OSError):
        return None


def save_plan(plan):
    forge_path(PLAN_FILE).write_text(json.dumps(plan, indent=2))


# ---------------------------------------------------------------- agents

def run_agent(role, tool, prompt, model=""):
    """Run one non-interactive agent session in the project directory."""
    cwd = STATE["project"]
    if tool == "mock":
        return mock_agent(role, cwd)
    if tool == "claude":
        cmd = ["claude", "-p", prompt, "--dangerously-skip-permissions"]
        if model:
            cmd += ["--model", model]
    elif tool == "codex":
        cmd = ["codex", "exec", "--dangerously-bypass-approvals-and-sandbox",
               "--skip-git-repo-check", "-C", cwd]
        if model:
            cmd += ["-m", model]
        cmd += [prompt]
    else:
        raise ValueError(f"unknown tool {tool}")
    log_event("agent", f"[{role}] starting {tool} session")
    proc = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True,
                          timeout=3600)
    tail = (proc.stdout or "").strip()[-2000:]
    if proc.returncode != 0:
        err = (proc.stderr or "").strip()[-2000:]
        log_event("error", f"[{role}] {tool} exited {proc.returncode}: {err}")
        raise RuntimeError(f"{tool} failed with code {proc.returncode}")
    log_event("agent", f"[{role}] {tool} finished: {tail[-600:]}")
    return tail


def mock_agent(role, cwd):
    """Fake agent used by the self-test. Never invoked from normal settings."""
    if role == "planner":
        save_plan({
            "goal": STATE["goal"], "status": "draft",
            "stages": [
                {"id": 1, "title": "first", "instructions": "append line one",
                 "acceptance": "file has line one", "commit": "feat: line one",
                 "status": "pending", "rounds": 0},
                {"id": 2, "title": "second", "instructions": "append line two",
                 "acceptance": "file has line two", "commit": "feat: line two",
                 "status": "pending", "rounds": 0},
            ],
        })
    elif role in ("implementer", "fixer"):
        with open(Path(cwd) / "mock.txt", "a") as f:
            f.write(f"work at {time.time()}\n")
    elif role == "checker":
        forge_path(VERDICT_FILE).write_text(
            json.dumps({"approved": True, "issues": []}))
    return "mock done"


PLANNER_PROMPT = """You are the planning agent of Forge, an AI build orchestrator.
Explore this repository, then produce an implementation plan for the goal below.

GOAL:
{goal}

Write the plan as JSON to the file {plan_path} (create the directory if needed) with exactly this schema:
{{"goal": "...", "status": "draft", "stages": [
  {{"id": 1, "title": "short title",
    "instructions": "complete, self-contained instructions for an implementing agent that has NOT seen this conversation",
    "acceptance": "concrete acceptance criteria",
    "commit": "proposed conventional commit message",
    "status": "pending", "rounds": 0}}
]}}

Rules: 2 to 8 stages, each independently committable, ordered by dependency.
Do NOT implement anything, do not modify any other file. Only write {plan_path}."""

IMPLEMENT_PROMPT = """You are the implementing agent of Forge for exactly one stage of an approved plan.

OVERALL GOAL:
{goal}

FULL PLAN (context only — do NOT work on other stages):
{plan_overview}

YOUR STAGE {sid}: {title}
INSTRUCTIONS:
{instructions}
ACCEPTANCE CRITERIA:
{acceptance}

Implement this stage completely. Verify your work runs (build/tests/quick manual check as appropriate).
Do NOT commit, do NOT push, do NOT touch the {forge_dir}/ directory."""

FIX_PROMPT = IMPLEMENT_PROMPT + """

An independent reviewer looked at your changes and requests fixes:
{issues}

Address every issue (or make the code obviously correct where the reviewer was wrong)."""

CHECK_PROMPT = """You are an independent reviewer in a fresh session. Another agent implemented one stage of a plan in this repository. Judge only whether the current uncommitted changes correctly implement the stage.

STAGE: {title}
INSTRUCTIONS GIVEN TO THE IMPLEMENTER:
{instructions}
ACCEPTANCE CRITERIA:
{acceptance}

Inspect with `git status` and `git diff` (all uncommitted changes belong to this stage), read files, and run checks if useful.
Then write your verdict as JSON to the file {verdict_path}:
{{"approved": true/false, "issues": ["specific, actionable issue", ...]}}

approved=true only if the acceptance criteria are met and you found no real defect.
Do NOT fix anything yourself; do NOT modify any file except {verdict_path}."""


# ------------------------------------------------------------- git helpers

def git(*args, check=True):
    proc = subprocess.run(["git", *args], cwd=STATE["project"],
                          capture_output=True, text=True)
    if check and proc.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)}: {proc.stderr.strip()}")
    return proc.stdout.strip()


def commit_stage(message):
    git("add", "-A", "--", ".", f":(exclude){FORGE_DIR}")
    if not git("status", "--porcelain", "--", ".", f":(exclude){FORGE_DIR}"):
        log_event("git", "nothing to commit for this stage")
        return None
    git("commit", "-m", message)
    sha = git("rev-parse", "--short", "HEAD")
    log_event("git", f"committed {sha}: {message}")
    return sha


# ------------------------------------------------------------- orchestrator

def plan_worker(goal):
    s = STATE["settings"]
    try:
        prompt = PLANNER_PROMPT.format(goal=goal,
                                       plan_path=f"{FORGE_DIR}/{PLAN_FILE}")
        run_agent("planner", s["planner"], prompt, s["planner_model"])
        plan = load_plan()
        if not plan or not plan.get("stages"):
            raise RuntimeError("planner did not produce a valid plan file")
        for st in plan["stages"]:
            st.setdefault("status", "pending")
            st.setdefault("rounds", 0)
        plan["goal"] = goal
        plan["status"] = "draft"
        save_plan(plan)
        with LOCK:
            STATE["phase"] = "plan_ready"
        log_event("plan", f"plan ready with {len(plan['stages'])} stages")
    except Exception as e:  # noqa: BLE001 — surface anything to the UI
        with LOCK:
            STATE["phase"] = "failed"
        log_event("error", f"planning failed: {e}")


def plan_overview(plan):
    return "\n".join(f"  {s['id']}. {s['title']} -> {s['commit']}"
                     for s in plan["stages"])


def run_stage(plan, stage):
    s = STATE["settings"]
    common = dict(goal=plan["goal"], plan_overview=plan_overview(plan),
                  sid=stage["id"], title=stage["title"],
                  instructions=stage["instructions"],
                  acceptance=stage.get("acceptance", ""),
                  forge_dir=FORGE_DIR)
    issues = None
    for round_no in range(int(s["max_fix_rounds"]) + 1):
        if STATE["stop_requested"]:
            return "stopped"
        stage["rounds"] = round_no + 1
        with LOCK:
            STATE["current_step"] = ("implementing" if issues is None
                                     else f"fixing (round {round_no})")
        save_plan(plan)
        if issues is None:
            run_agent("implementer", s["implementer"],
                      IMPLEMENT_PROMPT.format(**common), s["implementer_model"])
        else:
            run_agent("fixer", s["implementer"],
                      FIX_PROMPT.format(**common, issues="\n".join(
                          f"- {i}" for i in issues)), s["implementer_model"])
        if STATE["stop_requested"]:
            return "stopped"

        with LOCK:
            STATE["current_step"] = "checking"
        vp = forge_path(VERDICT_FILE)
        if vp.exists():
            vp.unlink()
        run_agent("checker", s["checker"],
                  CHECK_PROMPT.format(**common,
                                      verdict_path=f"{FORGE_DIR}/{VERDICT_FILE}"),
                  s["checker_model"])
        try:
            verdict = json.loads(vp.read_text())
        except (OSError, json.JSONDecodeError):
            log_event("error", "checker produced no readable verdict; retrying stage")
            issues = ["The previous review session failed to produce a verdict. "
                      "Re-verify the implementation end to end."]
            continue
        if verdict.get("approved"):
            log_event("check", f"stage {stage['id']} approved by checker")
            return "approved"
        issues = verdict.get("issues") or ["reviewer rejected without details"]
        log_event("check", f"stage {stage['id']} rejected: " +
                  "; ".join(issues)[:1500])
    return "exhausted"


def run_worker():
    plan = load_plan()
    try:
        for stage in plan["stages"]:
            if stage["status"] == "committed":
                continue
            if STATE["stop_requested"]:
                raise InterruptedError
            with LOCK:
                STATE["current_stage"] = stage["id"]
            stage["status"] = "in_progress"
            save_plan(plan)
            log_event("stage", f"stage {stage['id']} started: {stage['title']}")
            result = run_stage(plan, stage)
            if result == "stopped":
                raise InterruptedError
            if result == "exhausted":
                stage["status"] = "blocked"
                save_plan(plan)
                with LOCK:
                    STATE["phase"] = "blocked"
                log_event("stage", f"stage {stage['id']} blocked: checker still "
                          "rejecting after max fix rounds — needs a human")
                return
            sha = commit_stage(stage["commit"])
            stage["status"] = "committed"
            stage["sha"] = sha
            save_plan(plan)

        plan["status"] = "done"
        save_plan(plan)
        if STATE["settings"]["auto_push"]:
            with LOCK:
                STATE["current_step"] = "pushing"
            try:
                out = git("push", "-u", "origin", "HEAD")
                log_event("git", f"pushed to origin: {out or 'ok'}")
            except RuntimeError as e:
                log_event("error", f"push failed (commits are safe locally): {e}")
        with LOCK:
            STATE["phase"] = "done"
        log_event("run", "all stages committed — run complete")
    except InterruptedError:
        with LOCK:
            STATE["phase"] = "plan_ready"
        log_event("run", "stopped by user; progress is saved, run again to continue")
    except Exception as e:  # noqa: BLE001
        with LOCK:
            STATE["phase"] = "failed"
        log_event("error", f"run failed: {e}")
    finally:
        with LOCK:
            STATE["current_stage"] = None
            STATE["current_step"] = ""
            STATE["stop_requested"] = False


def start_thread(fn, *args):
    global WORKER
    WORKER = threading.Thread(target=fn, args=args, daemon=True)
    WORKER.start()


# ---------------------------------------------------------------- http

class Handler(BaseHTTPRequestHandler):
    def _send(self, obj, code=200, ctype="application/json"):
        body = obj if isinstance(obj, bytes) else json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):  # silence request spam
        pass

    def do_GET(self):
        if self.path in ("/", "/index.html"):
            self._send((ROOT / "index.html").read_bytes(),
                       ctype="text/html; charset=utf-8")
        elif self.path == "/api/state":
            with LOCK:
                snap = {**{k: v for k, v in STATE.items()},
                        "plan": load_plan(), "history": read_history()}
            try:
                snap["git_log"] = git("log", "--oneline", "-12", check=False)
            except Exception:  # noqa: BLE001
                snap["git_log"] = ""
            self._send(snap)
        elif self.path == "/api/diff":
            try:
                d = git("diff", "HEAD", check=False) or git("diff", check=False)
            except Exception:  # noqa: BLE001
                d = ""
            self._send({"diff": d[-40000:]})
        else:
            self._send({"error": "not found"}, 404)

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        route = self.path
        busy = STATE["phase"] in ("planning", "running")

        if route == "/api/settings":
            with LOCK:
                STATE["settings"].update(
                    {k: v for k, v in body.items() if k in STATE["settings"]})
            self._send({"ok": True})
        elif route == "/api/project":
            path = os.path.expanduser(body.get("path", "").strip())
            if busy:
                self._send({"error": "busy"}, 409)
            elif not (Path(path) / ".git").exists():
                self._send({"error": f"{path} is not a git repository"}, 400)
            else:
                with LOCK:
                    STATE["project"] = path
                    STATE["phase"] = "plan_ready" if load_plan() else "idle"
                self._send({"ok": True})
        elif route == "/api/plan":
            if busy:
                self._send({"error": "busy"}, 409)
            else:
                goal = body.get("goal", "").strip()
                with LOCK:
                    STATE["goal"] = goal
                    STATE["phase"] = "planning"
                log_event("plan", f"planning started for goal: {goal[:300]}")
                start_thread(plan_worker, goal)
                self._send({"ok": True})
        elif route == "/api/approve":
            plan = load_plan()
            if not plan:
                self._send({"error": "no plan"}, 400)
            else:
                plan["status"] = "approved"
                save_plan(plan)
                log_event("plan", "plan approved by user")
                self._send({"ok": True})
        elif route == "/api/run":
            plan = load_plan()
            if busy:
                self._send({"error": "busy"}, 409)
            elif not plan or plan.get("status") not in ("approved", "done"):
                self._send({"error": "plan is not approved"}, 400)
            else:
                with LOCK:
                    STATE["phase"] = "running"
                    STATE["stop_requested"] = False
                    STATE["goal"] = plan.get("goal", STATE["goal"])
                log_event("run", "run started")
                start_thread(run_worker)
                self._send({"ok": True})
        elif route == "/api/stop":
            with LOCK:
                STATE["stop_requested"] = True
            log_event("run", "stop requested; finishing current agent session")
            self._send({"ok": True})
        elif route == "/api/reset_plan":
            if busy:
                self._send({"error": "busy"}, 409)
            else:
                p = forge_path(PLAN_FILE)
                if p.exists():
                    p.unlink()
                with LOCK:
                    STATE["phase"] = "idle"
                log_event("plan", "plan discarded")
                self._send({"ok": True})
        else:
            self._send({"error": "not found"}, 404)


def main():
    server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
    print(f"Forge running on http://localhost:{PORT}  (project: {STATE['project']})")
    server.serve_forever()


if __name__ == "__main__":
    main()
