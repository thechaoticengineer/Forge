// Read-only presentation records. Field text is never trimmed, prefixed or joined.
function field(key, label, text, error) {
    return {key: key, label: label, text: text == null ? "" : String(text), status: "", error: !!error, detail: true}
}
function status(key, text, error) {
    return {key: key, label: "", text: "", status: text || "", error: !!error, detail: false}
}
function decisions(records, prefix) {
    const rows = []
    ;(records || []).forEach(function(d, i) {
        const key = prefix + "/" + (d.id || i)
        rows.push(status(key, "Decision " + d.id + " · " + d.status
            + (d.supersedes ? " · supersedes " + d.supersedes : "")))
        rows.push(field(key + "/summary", "Summary", d.summary))
        if (d.rationale != null) rows.push(field(key + "/rationale", "Rationale", d.rationale))
        ;(d.alternatives || []).forEach(function(a, n) {
            rows.push(field(key + "/alternative/" + n, "Alternative " + (n + 1), a.description))
            rows.push(field(key + "/tradeoffs/" + n, "Tradeoffs " + (n + 1), a.tradeoffs))
        })
    })
    return rows
}
function architecture(cp, activity, persistenceError) {
    cp = cp || {}; activity = activity || {}
    const rows = [status("context", "Architecture · revision " + (cp.revision || "legacy")
        + " · context " + (cp.context_status || "legacy"), cp.context_status === "error"),
        status("activity", "Architect · " + (cp.context_status === "needs_recovery" ? "needs_recovery"
            : activity.status || cp.context_status || "legacy")
            + (cp.session && cp.session.reference ? " · session " + cp.session.reference : ""),
            !!activity.error || cp.context_status === "needs_recovery")]
    for (const item of [["persistence", "Persistence error", persistenceError, true],
        ["error", "Architecture error", cp.error, true], ["activity-error", "Architect error", activity.error, true],
        ["reason", "Activity reason", activity.reason], ["recovery", "Recovery", (cp.recovery || {}).reason]]) {
        if (item[2]) rows.push(field(item[0], item[1], item[2], item[3]))
    }
    Object.keys(cp.guidance || {}).forEach(function(id) {
        const g = cp.guidance[id]
        rows.push(status("guidance-status/" + id, "Stage " + id + " guidance" + (g.valid ? "" : " · needs refresh")))
        rows.push(field("guidance/" + id, "Guidance", g.text))
    })
    ;(cp.unresolved_risks || []).forEach(function(r, i) { rows.push(field("risk/" + (r.id || i), "Risk " + r.id, r.text)) })
    return rows.concat(decisions(cp.recent_decisions, "decision"))
}
function providers(records) {
    const rows = []
    ;(records || []).forEach(function(p) {
        const key = p.provider
        rows.push(status(key, p.provider + ": " + p.status + " · " + p.model_count + " models"
            + (p.cached_stale ? " · cached/stale" : ""), p.status === "unavailable"))
        for (const item of [["blocker", "Provider blocked", (p.blocker || {}).message],
            ["error", "Provider error", (p.error || {}).message], ["cache", "Cache error", p.cache_error]]) {
            if (item[2]) rows.push(field(key + "/" + item[0], item[1], item[2], true))
        }
        if (p.description != null) rows.push(field(key + "/description", "Description", p.description))
    })
    return rows
}
function options(records, format) {
    const rows = []
    ;(records || []).forEach(function(o, i) {
        const key = o.provider + "/" + o.model + "/" + o.effort + "/" + i
        rows.push(status(key, format(Object.assign({}, o, {error: ""}))
            + (o.eligible ? " · eligible" : " · ineligible"), !o.eligible))
        if (o.error) rows.push(field(key + "/error", "Option error", o.error, true))
        if (o.description != null) rows.push(field(key + "/description", "Description", o.description))
    })
    return rows
}
function sources(records, stamp) {
    const rows = []
    ;(records || []).forEach(function(s, i) {
        rows.push(status("source/" + i, s.error ? "Source error · failures " + s.failures
            + " · next attempt " + stamp(s.next_attempt_unix)
            : s.checked_unix ? "Source ok · checked " + stamp(s.checked_unix) : "Source pending", !!s.error))
        rows.push(field("source/" + i + "/url", "Source URL", s.url))
        if (s.error) rows.push(field("source/" + i + "/error", "Source error", s.error, true))
    })
    return rows
}
function report(report, helpers) {
    const rows = []
    ;(report.commits || []).forEach(function(c, i) {
        rows.push(field("commit/" + i, "Commit " + (c.sha || "—"), c.message == null ? c.title : c.message))
    })
    if (report.plan_id) {
        rows.push(status("plan", "Plan " + report.plan_id + " · revision " + report.revision))
        if (report.project) rows.push(status("project", "Project " + report.project))
        const a = report.architecture || {}
        if (a.summary != null) rows.push(field("architecture", "Architecture", a.summary))
        rows.push.apply(rows, decisions(a.recent_decisions, "decision"))
        ;(a.unresolved_risks || []).forEach(function(r, i) { rows.push(field("risk/" + (r.id || i), "Risk " + r.id, r.text)) })
        ;(report.stage_outcomes || []).forEach(function(s) {
            const key = "stage/" + s.id, gate = s.review_gate || {}, roles = gate.roles || {}
            rows.push(status(key, "Stage " + s.id + ": " + s.title + " · " + s.status))
            rows.push(status(key + "/gate", "Recorded aggregate: " + (gate.status || "unavailable")
                + " · independent: " + (roles.reviewer || "unavailable") + " · architect: "
                + (roles.architect === "not_required" ? "not required" : roles.architect || "unavailable")))
            const view = Object.assign({}, s, {model_invocations: s.last_invocation ? [s.last_invocation] : []})
            if (s.model_agreement) {
                rows.push(status(key + "/model", helpers.stageModelStatus(view)))
                const error = helpers.stageModelErrors(view)
                if (error) rows.push(field(key + "/error", "Routing error", error, true))
                helpers.stageModelDetails(view, true).forEach(function(f, i) { rows.push(field(key + "/model/" + i, f.label, f.text)) })
            }
            if ((s.review_policy || {}).rationale != null) rows.push(field(key + "/policy", "Policy rationale", s.review_policy.rationale))
        })
        rows.push(status("roles", helpers.architectUsageText(report)))
        rows.push(field("history", "Archived history", "/api/architecture/history?plan_id="
            + encodeURIComponent(report.plan_id) + (report.project ? "&project=" + encodeURIComponent(report.project) : "")))
    }
    for (const item of [["usage", "Usage", report.usage], ["planner", "Planner usage", report.planner_usage]]) {
        Object.keys(item[2] || {}).forEach(function(tool) {
            const usage = {}; usage[tool] = item[2][tool]
            const text = helpers.usageBreakdown(usage)
            if (text) rows.push(field(item[0] + "/" + tool, item[1] + " · " + tool, text))
        })
    }
    return rows
}

// Reconcile flat fields in place. Open text is an inspected snapshot: status can
// change immediately, but changed prose is published only on collapse. Identical
// polls never rebind the TextArea, and moves preserve the delegate and selection.
function reconcile(model, entries) {
    const wanted = new Set(entries.map(e => e.key))
    entries = entries.slice()
    // A resolved error or rolling collection may omit an inspected field. Keep
    // its snapshot explicitly labelled until collapse rather than destroying
    // the selected editor during a poll. Project/plan scope resets still clear it.
    for (let i = 0; i < model.count; ++i) {
        const row = model.get(i)
        if (!wanted.has(row.key) && row.expanded) {
            entries.splice(Math.min(i, entries.length), 0, {key: row.key, label: row.label,
                text: row.text, status: row.status, error: row.error, detail: row.detail, retired: true})
        }
    }
    const retained = new Set(entries.map(e => e.key))
    for (let i = model.count - 1; i >= 0; --i) if (!retained.has(model.get(i).key)) model.remove(i)
    entries.forEach(function(e, i) {
        e = Object.assign({retired: false}, e)
        let at = i
        while (at < model.count && model.get(at).key !== e.key) ++at
        if (at === model.count) model.insert(i, Object.assign({}, e, {expanded: false, pendingText: e.text}))
        else {
            if (at !== i) model.move(at, i, 1)
            const row = model.get(i)
            for (const key of ["label", "status", "error", "detail", "retired"]) if (row[key] !== e[key]) model.setProperty(i, key, e[key])
            if (row.pendingText !== e.text) model.setProperty(i, "pendingText", e.text)
            if (!row.expanded && row.text !== e.text) model.setProperty(i, "text", e.text)
        }
    })
}
function expand(model, key, value) {
    for (let i = 0; i < model.count; ++i) {
        const row = model.get(i)
        if (row.key !== key) continue
        if (!value && row.retired) { model.remove(i); return }
        model.setProperty(i, "expanded", value)
        if (!value && row.text !== row.pendingText) model.setProperty(i, "text", row.pendingText)
        return
    }
}

function metadata(records, stamp) {
    const rows = []
    ;(records || []).forEach(function(r, i) {
        const key = r.provider + "/" + r.model + "/" + i
        rows.push(status(key, r.provider + "/" + r.model + " · verified " + stamp(r.verified_unix)
            + (r.removed ? " · removed (retained for audit)" : "")
            + (r.conflicts && r.conflicts.length ? " · conflict: discovered native support wins" : "")))
        rows.push(field(key + "/provenance", "Provenance", r.provenance))
        if (r.description != null) rows.push(field(key + "/description", "Description", r.description))
        if (r.pricing) {
            const p = r.pricing
            rows.push(field(key + "/pricing-label", "Pricing explanation", p.label))
            rows.push(status(key + "/pricing", "API list rate: " + p.input + "/" + p.output + " "
                + p.currency + " " + p.unit + " · basis " + p.basis + " · as of " + p.as_of))
        } else rows.push(status(key + "/pricing", "Pricing unknown"))
    })
    return rows
}

// Reports have no durable record IDs. Serialize each immutable record once per
// collection update, reuse a short identity for each exact duplicate occurrence,
// and discard absent originals from the cache. Navigation never serializes text.
function indexReports(reports, cache) {
    const byRecord = new Map(), keys = [], positions = Object.create(null)
    const rows = reports.map(function(report, index) {
        const text = JSON.stringify(report)
        const occurrences = byRecord.get(text) || []
        const previous = cache.byRecord.get(text) || []
        const key = previous[occurrences.length] || "report/" + (++cache.nextId)
        occurrences.push(key)
        byRecord.set(text, occurrences)
        keys.push(key)
        positions[key] = index
        return {key: key, label: "", text: text, status: "", error: false, detail: false}
    })
    cache.byRecord = byRecord
    return {rows: rows, keys: keys, positions: positions}
}
