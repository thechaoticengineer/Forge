// Mutable request bookkeeping is separate from QML delegates and engine snapshots.
function newFeed() {
    return { generation: 0, pending: null, session: "", cursor: 0, legacy: false }
}

function invalidate(feed) {
    ++feed.generation
    feed.pending = null
}

function begin(feed, project, revision, invocation) {
    if (feed.pending || !project) return null
    const request = { generation: ++feed.generation, project: project,
        revision: revision, invocation: invocation, session: feed.session,
        cursor: feed.legacy ? 0 : feed.cursor }
    feed.pending = request
    return request
}

function finish(feed, request, project, revision, invocation, response) {
    if (feed.pending !== request || request.generation !== feed.generation
        || request.project !== project || request.revision !== revision
        || request.invocation !== invocation || request.session !== feed.session)
        return null
    // A stale callback must never clear a newer request's pending state.
    feed.pending = null
    if (!response || response.project !== project || response.version !== 1
        || !Array.isArray(response.entries) || typeof response.session !== "string"
        || typeof response.next_cursor !== "number") return null
    const reset = response.reset || response.session !== feed.session
    feed.session = response.session
    feed.cursor = response.next_cursor // advances even on an empty terminal page
    feed.legacy = response.legacy === true
    return { reset: reset, entries: response.entries, more: response.more === true }
}

function key(project, session, id) {
    return JSON.stringify([project, session, id])
}

// Keep retained rows and original string bindings untouched on identical polls.
// Seen history rows remain available until leaving the project, including rows
// which have rolled out of the server's bounded tail. Filtering only hides rows.
function reconcile(model, entries, project, session) {
    const indexes = Object.create(null)
    for (let i = 0; i < model.count; ++i) indexes[model.get(i).entryKey] = i
    entries.forEach(function(entry) {
        if (!entry || typeof entry.id !== "string" || typeof entry.text !== "string") return
        const identity = key(project, session, entry.id)
        if (indexes[identity] !== undefined) {
            const index = indexes[identity], row = model.get(index)
            // Pre-upgrade legacy text is one changing opaque source. Do not
            // disturb selection while it is open; publish its refresh on collapse.
            if (entry.kind === "legacy") {
                if (row.expanded) {
                    if (row.deferredText !== entry.text) model.setProperty(index, "deferredText", entry.text)
                    const deferred = row.originalText !== entry.text
                    if (row.deferred !== deferred) model.setProperty(index, "deferred", deferred)
                } else if (row.originalText !== entry.text) {
                    model.setProperty(index, "originalText", entry.text)
                }
            }
            return
        }
        indexes[identity] = model.count
        model.append({ entryKey: identity, originalText: entry.text,
            kind: entry.kind || "message", stream: entry.stream || "",
            label: entry.label || "", time: entry.t || "", unix: entry.unix || 0,
            goal: entry.goal || "", separator: "", shown: true,
            expanded: false, deferredText: "", deferred: false })
    })
}

function matches(kind, filter) {
    return filter === "all"
        || (filter === "runs" && ["run", "stage", "plan", "queue", "update"].indexOf(kind) !== -1)
        || (filter === "git" && kind === "git")
        || (filter === "reviews" && (kind === "review" || kind === "check"))
        || (filter === "errors" && kind === "error")
}

function filterRows(model, filter) {
    let previousGoal = ""
    for (let i = 0; i < model.count; ++i) {
        const row = model.get(i), shown = matches(row.kind, filter)
        const separator = shown && row.goal !== previousGoal ? row.goal : ""
        if (row.shown !== shown) model.setProperty(i, "shown", shown)
        if (row.separator !== separator) model.setProperty(i, "separator", separator)
        if (shown) previousGoal = row.goal
    }
}

function expand(model, identity, expanded) {
    for (let i = 0; i < model.count; ++i) {
        const row = model.get(i)
        if (row.entryKey !== identity) continue
        model.setProperty(i, "expanded", expanded)
        if (!expanded && row.deferred) {
            model.setProperty(i, "originalText", row.deferredText)
            model.setProperty(i, "deferredText", "")
            model.setProperty(i, "deferred", false)
        }
        return
    }
}
