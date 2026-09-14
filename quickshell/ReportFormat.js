// Pure architecture and archived-report presentation.
function architectActivityText(activity, architecture) {
    const a = activity || {}, cp = architecture || {}
    let text = "Architect · " + (cp.context_status === "needs_recovery" ? "needs_recovery" : a.status || cp.context_status || "legacy")
    if (a.error) text += " · " + a.error
    else if (a.reason) text += " · " + a.reason
    else if (cp.recovery && cp.recovery.reason) text += " · recovered: " + cp.recovery.reason
    if (cp.session && cp.session.reference) text += " · session " + cp.session.reference
    return text
}

function architectGuidanceText(architecture) {
    const cp = architecture || {}, guidance = cp.guidance || {}
    return Object.keys(guidance).map(function(id) {
        const g = guidance[id]
        return "Stage " + id + (g.valid ? "" : " (needs refresh)") + ": " + (g.text || "")
    }).join("\n")
}

function architectDecisionText(d) {
    let text = d.id + " · " + d.status + " · " + d.summary
    if (d.rationale) text += "\nWhy: " + d.rationale
    if (d.supersedes) text += "\nSupersedes: " + d.supersedes
    ;(d.alternatives || []).forEach(function(a) { text += "\nAlternative: " + a.description + " — " + a.tradeoffs })
    return text
}

function architectUsageText(plan) {
    const usage = plan && plan.role_usage ? plan.role_usage : {}
    return Object.keys(usage).map(function(role) {
        const tools = usage[role] || {}
        let total = 0
        Object.keys(tools).forEach(function(tool) { total += tools[tool].total_tokens || 0 })
        return role + ": " + total + " tokens"
    }).join(" · ")
}

function reportLifecycleText(report, stageModelText) {
    if (!report || !report.plan_id) return ""
    const architecture = report.architecture || {}
    let text = "Plan " + report.plan_id + " · revision " + report.revision
    if (architecture.summary) text += "\nArchitecture: " + architecture.summary
    ;(architecture.recent_decisions || []).forEach(function(d) { text += "\nDecision: " + architectDecisionText(d) })
    ;(report.stage_outcomes || []).forEach(function(s) {
        text += "\nStage " + s.id + ": " + s.title + " · " + s.status
        if (s.model_agreement) {
            const view = Object.assign({}, s, {model_invocations: s.last_invocation ? [s.last_invocation] : []})
            text += "\n" + stageModelText(view, true)
        }
        const gate = s.review_gate || {}, roles = gate.roles || {}, policy = s.review_policy || {}
        text += "\nRecorded aggregate: " + (gate.status || "unavailable")
            + " · independent: " + (roles.reviewer || "unavailable")
            + " · architect: " + (roles.architect === "not_required" ? "not required" : roles.architect || "unavailable")
        if (policy.rationale) text += "\nPolicy: " + policy.rationale
    })
    const usage = architectUsageText(report)
    if (usage) text += "\nRole usage: " + usage
    text += "\nArchived decisions, supersessions and agreements: /api/architecture/history?plan_id=" + encodeURIComponent(report.plan_id)
    if (report.project) text += "&project=" + encodeURIComponent(report.project)
    return text
}

function reportTime(report, now) {
    if (typeof report.unix !== "number" || !isFinite(report.unix)) return "—"
    const seconds = Math.max(0, Math.floor(now - report.unix))
    if (seconds < 60) return "just now"
    if (seconds < 3600) return Math.floor(seconds / 60) + "m ago"
    if (seconds < 86400) return Math.floor(seconds / 3600) + "h ago"
    return Math.floor(seconds / 86400) + "d ago"
}

function reportDuration(seconds) {
    const count = typeof seconds === "number" && isFinite(seconds) && seconds >= 0 ? Math.floor(seconds) : null
    return count === null ? "—" : Math.floor(count / 60) + "m " + (count % 60) + "s"
}

function reportCommits(commits) {
    const lines = []
    if (commits && typeof commits.length === "number") {
        for (let i = 0; i < commits.length; i++) {
            const commit = commits[i]
            if (!commit || !(commit.sha || commit.message || commit.title)) continue
            lines.push(((commit.sha || "—") + " " + (commit.message || commit.title || "")).trim())
        }
    }
    return lines.join("\n")
}
