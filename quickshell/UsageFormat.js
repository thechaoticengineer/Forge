// Pure quota and token-usage presentation.
function nonNegativeInt(value) {
    return typeof value === "number" && isFinite(value) && value >= 0 ? Math.floor(value) : null
}

function formatTokens(value) {
    const count = nonNegativeInt(value)
    if (count === null) return "—"
    if (count >= 1000000) return (count / 1000000).toFixed(1) + "M"
    if (count >= 1000) return (count / 1000).toFixed(1) + "k"
    return String(count)
}

function reportDuration(seconds) {
    const count = nonNegativeInt(seconds)
    return count === null ? "—" : Math.floor(count / 60) + "m " + (count % 60) + "s"
}

function usageTools(usage) {
    return usage && typeof usage === "object" ? Object.keys(usage).sort().filter(function(tool) {
        return usage[tool] && typeof usage[tool] === "object"
    }) : []
}

function quotaSummary(quota) {
    if (!quota || quota.status === "pending") return "Claude limits: checking…"
    const windows = quota.windows || []
    if (quota.status === "unavailable") return "Claude limits: unavailable" + (quota.error ? " · " + quota.error : "")
    const rows = windows.map(function(window) {
        const expired = window.resets_unix && window.resets_unix * 1000 <= Date.now()
        const remaining = expired ? "awaiting refresh" : typeof window.used_percent === "number"
            ? Math.max(0, 100 - window.used_percent).toFixed(0) + "% remaining" : "remaining unknown"
        const reset = window.resets_at ? " · reset " + new Date(window.resets_at).toLocaleString() : ""
        return window.name + ": " + remaining + reset
    })
    if (!rows.length) rows.push("Claude limits: no usage windows reported")
    if (quota.status === "stale") rows.push("Previous reading · " + (quota.error || "refresh pending"))
    if (quota.extra_usage_enabled === true) rows.push("Usage credits enabled")
    if (quota.refreshing) rows.push("Refreshing…")
    return rows.join("\n")
}

function usageSummary(usage) {
    return usageTools(usage).map(function(tool) {
        return tool + " " + formatTokens(usage[tool].total_tokens) + " tok"
    }).join(" · ")
}

function modelSummary(models) {
    return models && typeof models === "object" ? Object.keys(models).sort().map(function(model) {
        return model + " " + formatTokens(models[model])
    }).join(" · ") : ""
}

function exactTokens(value) {
    // Keep exact counts in details; only the summary and model list are compact.
    const count = nonNegativeInt(value)
    return count === null ? "—" : String(count)
}

function usageBreakdown(usage) {
    return usageTools(usage).map(function(tool) {
        const item = usage[tool], models = modelSummary(item.models)
        return tool + ": input " + exactTokens(item.input_tokens) + " · output " + exactTokens(item.output_tokens)
            + " · total " + exactTokens(item.total_tokens) + " tok · calls " + exactTokens(item.calls)
            + (models ? "\nmodels: " + models : "")
    }).join("\n")
}
