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

// Build chat rows (oldest first) from the transcript plus the in-flight and error
// state. Entries never include the pending message: the engine only appends the
// user/assistant pair once a reply succeeds, so a sending message and a failed
// reply are synthesized here from Panel.qml's discussionSent/discussionError.
function chatMessages(entries, pending, sentMessage, error) {
    const list = Array.isArray(entries) ? entries : []
    const occurrences = new Map()
    const rows = []
    for (const entry of list) {
        if (!entry) continue
        let kind
        if (entry.role === "user") kind = "user"
        else if (entry.role === "assistant") kind = "assistant"
        else continue
        const text = entry.text == null ? "" : String(entry.text)
        const identity = JSON.stringify([entry.role, entry.unix, text])
        const n = occurrences.get(identity) || 0
        occurrences.set(identity, n + 1)
        rows.push({key: "entry/" + identity + "/" + n, kind: kind, outgoing: kind === "user",
            author: kind === "user" ? "You" : "Forge", status: "", text: text})
    }
    const sent = sentMessage == null ? "" : String(sentMessage)
    if (pending && sent.trim() !== "")
        rows.push({key: "pending/user", kind: "user", outgoing: true, author: "You", status: "Sending…", text: sent})
    if (pending)
        rows.push({key: "pending/reply", kind: "pending", outgoing: false, author: "Forge", status: "", text: "Forge is replying…"})
    if (!pending) {
        const errorText = error == null ? "" : String(error)
        if (errorText.trim() !== "")
            rows.push({key: "error", kind: "error", outgoing: false, author: "Forge", status: "Reply failed", text: errorText})
    }
    return rows
}

// Reconcile chat rows in place, mirroring reconcile() in PanelDetails.js: remove
// keys that are gone, move retained keys into position and update only the
// fields that changed, so an identical call never touches a delegate.
function reconcileChat(model, rows) {
    rows = rows || []
    const wanted = new Set(rows.map(r => r.key))
    for (let i = model.count - 1; i >= 0; --i) if (!wanted.has(model.get(i).key)) model.remove(i)
    rows.forEach(function(r, i) {
        let at = i
        while (at < model.count && model.get(at).key !== r.key) ++at
        if (at === model.count) {
            model.insert(i, {key: r.key, kind: r.kind, outgoing: r.outgoing, author: r.author, status: r.status, text: r.text})
        } else {
            if (at !== i) model.move(at, i, 1)
            const row = model.get(i)
            for (const field of ["kind", "outgoing", "author", "status", "text"])
                if (row[field] !== r[field]) model.setProperty(i, field, r[field])
        }
    })
}
