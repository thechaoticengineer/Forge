// Pure terminal-state decision for an asynchronous discussion message request.
function discussionAction(activity, requestId) {
    if (requestId < 0 || !activity || activity.request_id !== requestId || activity.status === "running")
        return {action: "none"}
    if (activity.status === "ready") return {action: "ready"}
    if (activity.status === "failed") return {action: "error",
        error: activity.error && String(activity.error).trim() ? activity.error : "Could not get a reply. Try again.",
        message: activity.message || ""}
    return {action: "none"}
}

// A discussion is plannable once at least one user message has a reply.
function canPlanFromDiscussion(entries) {
    const list = entries || []
    for (let i = 0; i + 1 < list.length; i++) {
        if (list[i] && list[i].role === "user" && list[i + 1] && list[i + 1].role === "assistant") return true
    }
    return false
}
