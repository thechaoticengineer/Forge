.pragma library

// Shared fixtures and item-tree helpers for the panel redesign M1 QML tests
// (tst_panel_shell, tst_panel_overview, tst_panel_settings, tst_panel_queue).
// Engine-state shapes mirror a real GET /api/state response.

var palette = {
    foreground: "#dddddd",
    mutedForeground: "#aaaaaa",
    background: "#202020",
    surface: "#282828",
    accent: "#6699ff",
    urgent: "#ff6666",
    success: "#4faf72",
    working: "#e0a030",
    fontFamily: "monospace",
    fontSize10: 10,
    fontSize11: 11,
    fontSize12: 12
}

// Every object below `root` (items, popups, delegates) whose objectName matches.
function walk(root, name) {
    return collect(root, function (o) { return o.objectName === name })
}

function walkPrefix(root, prefix) {
    return collect(root, function (o) {
        return typeof o.objectName === "string" && o.objectName.indexOf(prefix) === 0
    })
}

function collect(root, match) {
    var found = []
    var seen = []
    function rec(o) {
        if (!o || seen.indexOf(o) >= 0) return
        seen.push(o)
        if (match(o)) found.push(o)
        var lists = [o.data, o.children, o.contentData, o.contentChildren]
        for (var l = 0; l < lists.length; ++l) {
            var list = lists[l]
            if (!list) continue
            for (var i = 0; i < list.length; ++i) rec(list[i])
        }
        if (o.contentItem) rec(o.contentItem)
    }
    rec(root)
    return found
}

// All text shown by `root` and everything below it, one string per visible text-bearing object.
function texts(root) {
    return collect(root, function (o) {
        return typeof o.text === "string" && o.text !== "" && o.visible !== false
    }).map(function (o) { return o.text })
}

function allText(root) {
    return texts(root).join("\n")
}

// True when `item` is visible and lies completely inside `area` (no scrolling needed).
function insideArea(item, area) {
    if (!item.visible || item.height <= 0 || item.width <= 0) return false
    var p = item.mapToItem(area, 0, 0)
    return p.x >= -0.5 && p.y >= -0.5
        && p.x + item.width <= area.width + 0.5 && p.y + item.height <= area.height + 0.5
}

function stage(id, title, status, extra) {
    var s = {
        id: id, title: title, status: status,
        instructions: "Implement " + title + " and keep every existing behaviour.",
        acceptance: "The tests for " + title + " pass.",
        rounds: status === "committed" ? 1 : 0,
        review_gate: { status: status === "committed" ? "approved" : "pending",
            roles: { architect: "not_required", reviewer: status === "committed" ? "approved" : "pending" } },
        model_agreement: { agreed: true, availability: "unverified", binding: "at_implementation_start",
            planner_reason: "Standard risk", architect_reason: "Preserve interfaces",
            effective: { provider: "claude", model: "claude-sonnet-5", native_effort: "provider_default" } },
        commit: status === "committed" ? "test(panel): " + title.toLowerCase() : null
    }
    for (var key in (extra || {})) s[key] = extra[key]
    return s
}

function fiveStagePlan(status) {
    return {
        plan_id: "plan-1", revision: 1, status: status || "approved",
        goal: "Split the panel into per-view files",
        stages: [
            stage(1, "Executable M1 scenario tests", "committed", { commit_sha: "a1b2c3d", duration_seconds: 312 }),
            stage(2, "Navigation shell and view keys", "committed", { commit_sha: "b2c3d4e", duration_seconds: 604 }),
            stage(3, "Activity, Architecture, Features and Queue tabs", "in_progress"),
            stage(4, "Settings tab and Plan tab", "pending"),
            stage(5, "Overview view and final Panel.qml reduction", "pending")
        ]
    }
}

function settings() {
    return {
        planner: "claude", architect: "codex", implementer: "claude",
        reviewer: "codex", reviewer_provider_mode: "configured", reviewer_model: "",
        automatic_routing: true, auto_push: true, queue_auto_approve: false,
        review_cadence: { architect: "per_plan", reviewer: "per_stage" }
    }
}

function engineState(phase, plan, extra) {
    var s = {
        project: "/home/user/Projects/forge", active_project: "/home/user/Projects/forge",
        phase: phase, busy: phase === "running" || phase === "planning",
        current_stage: phase === "running" ? 3 : null,
        current_step: phase === "running" ? "implementing" : "",
        goal: "Split the panel into per-view files",
        run_started_unix: phase === "running" ? 1789751906 : 0,
        plan: plan, settings: settings(), queue: [], queue_active: false,
        agent: null, sessions: [], model_catalogue: catalogue(), claude_quota: quota(),
        discussion: [], chat: []
    }
    for (var key in (extra || {})) s[key] = extra[key]
    return s
}

function runningAgent() {
    return { role: "implementer", tool: "claude", model: "claude-sonnet-5", lines: 42,
        started_unix: 1789751908, last_line: "« tool: go test ./... ok" }
}

// A poll far enough in the future that quota windows never read as expired.
function quota() {
    var soon = Math.floor(Date.now() / 1000) + 3600
    return {
        status: "available", error: null, refreshing: false, extra_usage_enabled: false,
        checked_unix: soon - 60,
        windows: [
            { name: "Claude · 5h", model: null, used_percent: 14.0, resets_unix: soon,
                resets_at: new Date(soon * 1000).toISOString() },
            { name: "Claude · weekly", model: null, used_percent: 33.0, resets_unix: soon + 86400,
                resets_at: new Date((soon + 86400) * 1000).toISOString() }
        ]
    }
}

function catalogue(extra) {
    var c = {
        configured_count: 8, policy_revision: "ai-1789285303642989665-1", policy_error: null,
        refreshing: false, provenance: "configured", details_url: "/api/models",
        metadata: { last_refresh_unix: 1789750220, last_requests: 2, negative: 8, provenance: "official",
            records: 11, refreshing: false, source_errors: 2, store_error: null, unknown_pricing: 11 },
        providers: [
            { provider: "codex", status: "discovered", model_count: 11, cached_stale: false,
                blocker: null, error: null, cache_error: null },
            { provider: "claude", status: "discovered", model_count: 5, cached_stale: false,
                blocker: null, error: null, cache_error: null }
        ]
    }
    for (var key in (extra || {})) c[key] = extra[key]
    return c
}

// Two busy/blocked sessions as the engine reports them, plus one of each other marker.
function sessions() {
    return [
        { name: "forge", project: "/home/user/Projects/forge", phase: "running", busy: true,
            queue_active: false, queued: 0, current_step: "implementing" },
        { name: "site", project: "/home/user/Projects/site", phase: "blocked", busy: false,
            queue_active: false, queued: 2, current_step: "" }
    ]
}

function allMarkerSessions() {
    return [
        { name: "busy", project: "/p/busy", phase: "running", busy: true, queue_active: false, queued: 0 },
        { name: "queue", project: "/p/queue", phase: "idle", busy: false, queue_active: true, queued: 0 },
        { name: "failed", project: "/p/failed", phase: "failed", busy: false, queue_active: false, queued: 0 },
        { name: "done", project: "/p/done", phase: "done", busy: false, queue_active: false, queued: 0 },
        { name: "idle", project: "/p/idle", phase: "idle", busy: false, queue_active: false, queued: 3 }
    ]
}
