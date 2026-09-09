// Review positions are absolute within a verified publication, never a summary
// or a role/round pair. State previews may have neither id nor identity.
function identity(record) {
    if (record.id) return 'id:' + record.id
    const i = record.identity
    if (i && i.plan_id && i.attempt_id && i.stage_id !== undefined && i.role && i.round !== undefined)
        return 'identity:' + JSON.stringify([i.plan_id, i.revision, i.stage_id, i.attempt_id, i.role, i.round, i.snapshot])
    return ''
}
function scope(project, visit, plan, stage) {
    plan = plan || {}
    const cp = (plan.architecture || {}).checkpoint || ''
    const previews = stage.reviews && stage.reviews.length ? Array.from(stage.reviews)
        : stage.last_verdict ? [stage.last_verdict] : []
    const count = stage.review_count === undefined ? previews.length : Number(stage.review_count)
    const s = {project: project, visit: visit, plan: plan.plan_id || '', revision: plan.revision === undefined ? null : plan.revision,
        stage: stage.id, checkpoint: cp, snapshot: stage.review_snapshot || '', count: count, previews: previews,
        fallbackRound: (!stage.reviews || !stage.reviews.length) && stage.status !== "in_progress" ? stage.rounds : null}
    // For old servers without a snapshot marker this signature invalidates the
    // view on visible changes, but is deliberately NOT evidence for mapping.
    s.key = JSON.stringify([project, visit, s.plan, s.revision, s.stage, cp, s.snapshot,
        count, s.snapshot || cp ? '' : previews])
    return s
}
function create(s) {
    const start = s.count - s.previews.length
    return {scope: s, pending: null, error: '', retry: null, rows: s.previews.map(function(v, i) {
        return {key: identity(v) || 'position:' + (start + i), position: start + i,
            verdict: v, round: v.round || s.fallbackRound, complete: v.truncated !== true}
    }), older: Math.max(0, start)}
}
function begin(view, cursor, end) {
    if (view.pending) return null
    const request = {scope: view.scope.key, cursor: cursor, end: end}
    view.pending = request
    view.error = ''
    view.retry = {cursor: cursor, end: end}
    return request
}
function path(view, request) {
    const s = view.scope
    let url = '/api/architecture/reviews?project=' + encodeURIComponent(s.project)
        + '&stage_id=' + s.stage + '&cursor=' + request.cursor
        + '&limit=' + Math.min(8, request.end - request.cursor)
    if (s.plan) url += '&plan_id=' + encodeURIComponent(s.plan)
    if (s.checkpoint) url += '&checkpoint=' + encodeURIComponent(s.checkpoint)
    return url
}
function finish(view, request, currentScope, response, status) {
    if (view.pending !== request || currentScope.key !== request.scope) return 'stale'
    view.pending = null
    function fail(message) { view.error = message; return 'failed' }
    if (status !== 200 || !response || response.error)
        return fail(response && response.error ? String(response.error) : 'Unable to load complete reviews')
    const s = view.scope
    if (response.project !== s.project || (s.plan && response.plan_id !== s.plan)
        || response.stage_id !== s.stage || response.revision !== s.revision)
        return fail('Review response belongs to a different project, plan or revision')
    // Checkpoint pins published histories. Legacy histories require the complete
    // content marker supplied by state and endpoint; equal counts/revisions or
    // equal previews alone cannot prove that records have not been replaced.
    if ((s.checkpoint && response.checkpoint !== s.checkpoint)
        || (s.snapshot && response.snapshot !== s.snapshot)
        || (!s.checkpoint && !s.snapshot) || response.count !== s.count) {
        view.error = 'Review history changed or cannot be verified. Refresh and retry.'
        return 'changed'
    }
    const items = response.items
    if (!Array.isArray(items) || !items.length || request.cursor + items.length > request.end
        || items.some(function(v) { return !v || typeof v !== 'object' || v.truncated === true }))
        return fail('Incomplete review response; full text is unavailable')
    const next = request.cursor + items.length
    if ((response.next_cursor !== null && response.next_cursor !== next)
        || (response.next_cursor === null && next !== s.count))
        return fail('Invalid review cursor')
    // Validate the whole page before changing any row.
    for (let i = 0; i < items.length; i++) {
        const old = view.rows.find(function(r) { return r.position === request.cursor + i })
        if (old && identity(old.verdict) && identity(old.verdict) !== identity(items[i]))
            return fail('Review identity does not match the selected snapshot')
    }
    const rows = view.rows.slice()
    items.forEach(function(v, i) {
        const position = request.cursor + i
        const index = rows.findIndex(function(r) { return r.position === position })
        const row = {key: index >= 0 ? rows[index].key : identity(v) || 'position:' + position,
            position: position, verdict: v, round: v.round, complete: true}
        if (index >= 0) rows[index] = row
        else rows.push(row)
    })
    rows.sort(function(a, b) { return a.position - b.position })
    view.rows = rows
    // Advance the older-page boundary only over a contiguous loaded suffix.
    // A byte-limited page followed by a failure must not strand a gap.
    const positions = {}
    rows.forEach(function(row) { positions[row.position] = true })
    let older = s.count
    while (older > 0 && positions[older - 1]) older--
    view.older = older
    view.retry = next < request.end ? {cursor: response.next_cursor, end: request.end} : null
    return view.retry ? 'more' : 'complete'
}
