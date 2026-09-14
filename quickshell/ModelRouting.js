// Pure model-agreement and reassessment presentation.
function appendField(fields, label, text) {
    if (text !== undefined && text !== null && text !== "") fields.push({label: label, text: String(text)})
}

function stageModelText(stage, detail) {
    return stageModelStatus(stage) + "\n" + stageModelDetails(stage, detail).map(function(field) {
        return field.label + ": " + field.text
    }).join("\n") + (stageModelErrors(stage) ? "\n" + stageModelErrors(stage) : "")
}

function stageModelStatus(stage) {
    const a = stage.model_agreement || {}
    const e = a.effective || {}, p = a.policy_inputs || {}, proposed = a.validated_proposal || {}
    let text = !stage.model_agreement ? "Model agreement pending — reconcile before approval"
        : (a.valid === false ? "Needs reconciliation · " : "Agreed · ")
        + e.provider + "/" + e.model + " · " + e.native_effort
        + " · " + (a.verification_state || a.availability || "unverified")
        + " · " + (p.tier || "unclassified") + " (" + (p.tier_provenance || "unknown provenance") + ")"
    if (a.version === 2) {
        text = (a.valid === false ? "Needs reconciliation · " : "Agreed · ")
            + "Tier: " + (p.tier || proposed.tier || "pending")
            + " · model selected at implementation start"
        const selected = stage.model_selection || {}, actual = selected.effective || {}
        if (actual.provider && actual.model)
            text += "\nSelected: " + actual.provider + "/" + actual.model + " · " + actual.native_effort
    }
    if (proposed.provider && proposed.model)
        text += "\nProposed: " + proposed.provider + "/" + proposed.model + " · " + proposed.native_effort
    const calls = stage.model_invocations || []
    if (calls.length) {
        const last = calls[calls.length - 1], actual = last.effective || last.requested || {}
        text += "\nExecution: " + actual.provider + "/" + actual.model + " · "
            + (actual.native_effort || (last.requested || {}).native_effort || "provider_default")
            + " · " + (last.verification_state || last.status)
    }
    const routing = stage.reassessment || {}, history = routing.history || []
    if (routing.status) text += "\nRouting: " + routing.status + " · reassessments " + (routing.count || 0)
        + "/" + ((routing.limits || {}).max_reassessments ?? 3)
        + " · operational retries " + (routing.operational_retries || 0)
        + "/" + ((routing.limits || {}).max_operational_retries ?? 2)
    const trigger = routing.pending || (history.length ? history[history.length - 1] : null)
    if (trigger) text += "\nTrigger: " + trigger.kind
    return text
}

function stageModelErrors(stage) {
    return [(stage.reassessment || {}).error, stage.model_block].filter(Boolean).join("\n")
}

function stageModelDetails(stage, detail) {
    return detail ? stageModelRationale(stage).concat(stageModelDiagnostics(stage)) : stageModelReasons(stage)
}

function stageModelReasons(stage) {
    const a = stage.model_agreement || {}, fields = []
    appendField(fields, "Planner", a.planner_reason)
    appendField(fields, "Architect", a.architect_reason)
    const routing = stage.reassessment || {}, history = routing.history || []
    const trigger = routing.pending || (history.length ? history[history.length - 1] : null)
    if (trigger) appendField(fields, "Trigger evidence", typeof trigger.evidence === "string" ? trigger.evidence
        : JSON.stringify(trigger.evidence || trigger.error || ""))
    return fields
}

function stageModelRoutingHistory(stage) {
    const routing = stage.reassessment || {}, history = routing.history || [], fields = []
    history.slice(-4).forEach(function(h, i) {
        appendField(fields, h.kind + " · Planner " + (i + 1), h.planner_reason)
        appendField(fields, h.kind + " · Architect " + (i + 1), h.architect_reason)
    })
    return fields
}

function stageModelDiagnostics(stage) {
    const a = stage.model_agreement || {}, p = (stage.model_selection || a).policy_inputs || {}, fields = []
    appendField(fields, "Risk", ((a.validated_proposal || {}).risk || "pending") + " · complexity: " + ((a.validated_proposal || {}).complexity || "pending"))
    appendField(fields, "Constraint", JSON.stringify(p.constraint || {}))
    appendField(fields, "Cost", p.relative_cost_preference !== null && p.relative_cost_preference !== undefined
        ? "configured relative preference " + p.relative_cost_preference + " (not a price)"
        : "unknown / no comparable billing data used")
    appendField(fields, "Routing price", p.pricing ? "API list rate (not CLI spend): " + JSON.stringify(p.pricing)
        : "unavailable / no comparable rate used")
    appendField(fields, "Agreement", (a.id || "pending") + " · policy: " + (p.policy || "pending"))
    const calls = stage.model_invocations || []
    if (calls.length) appendField(fields, "Latest invocation", JSON.stringify(calls[calls.length - 1]))
    return fields
}

function stageModelRationale(stage) {
    return stageModelReasons(stage).concat(stageModelRoutingHistory(stage))
}
