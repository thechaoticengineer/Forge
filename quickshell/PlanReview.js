// Complete plan requests come from the gate or identity-matched history events.
// History is fetched only on disclosure, one bounded page at a time.
function canonical(value) {
    if (Array.isArray(value)) return '[' + value.map(canonical).join(',') + ']'
    if (value && typeof value === 'object') return '{' + Object.keys(value).sort()
        .map(k => JSON.stringify(k) + ':' + canonical(value[k])).join(',') + '}'
    return JSON.stringify(value)
}
function scope(project, visit, plan) {
    const review = (plan || {}).plan_review || {}, gate = review.gate || {}
    return canonical([project, visit, (plan || {}).plan_id, (plan || {}).revision,
        review.attempt_id, review.rounds, gate])
}
function text(requests) {
    return requests.map(r => '[' + r.role + '] ' + r.text).join('\n\n')
}
function create(project, plan) {
    const review = plan.plan_review || {}, gate = review.gate || {}
    const complete = Array.isArray(gate.requests) && gate.requests_truncated !== true
    return {project: project, planId: plan.plan_id, identity: gate.identity,
        identityTruncated: gate.identity_truncated === true, roles: gate.roles || {},
        complete: complete, text: text(Array.isArray(gate.requests) ? gate.requests : []),
        pending: false, error: '', cursor: 0, end: null, records: {}}
}
function path(view) {
    return '/api/architecture/history?project=' + encodeURIComponent(view.project)
        + '&plan_id=' + encodeURIComponent(view.planId) + '&cursor=' + view.cursor + '&limit=100'
}
function begin(view) {
    if (view.pending || view.complete) return false
    const id = view.identity
    if (view.identityTruncated || !id || id.scope !== 'plan' || id.stage_id !== null
        || id.plan_id !== view.planId || !id.attempt_id || !id.snapshot || !id.policy) {
        view.error = 'Complete requests unavailable: the current gate has no complete review identity.'
        return false
    }
    if (view.error) { view.cursor = 0; view.end = null; view.records = {} }
    view.error = ''; view.pending = true
    return true
}
function finish(view, response, status) {
    view.pending = false
    function fail(message) { view.error = message; return false }
    if (status !== 200 || !response || response.plan_id !== view.planId || !Array.isArray(response.items))
        return fail((response || {}).error || 'Unable to load complete plan review requests. Retry full text.')
    // Freeze the published end at the first page, so a growing log cannot keep
    // a disclosure request running forever. New gates get a separate view.
    if (view.end === null) view.end = response.event_end
    for (const event of response.items) {
        if (event.plan_id !== view.planId || (event.payload || {}).kind !== 'plan_review') continue
        for (const record of event.payload.reviews || []) {
            const identity = Object.assign({}, record.identity), role = identity.role
            delete identity.role
            if (canonical(identity) !== canonical(view.identity) || record.truncated === true) continue
            if (!Object.prototype.hasOwnProperty.call(view.records, role)) view.records[role] = record
        }
    }
    const roles = (view.identity.policy.required_roles || []).filter(role =>
        ['approved', 'changes_requested'].indexOf(view.roles[role]) !== -1)
    if (roles.length && roles.every(role => !!view.records[role])) {
        const requests = []
        for (const role of roles) {
            const record = view.records[role], seen = []
            for (const field of ['issues', 'notes']) for (const value of record[field] || []) {
                if (typeof value === 'string' && seen.indexOf(value) === -1) {
                    seen.push(value); requests.push({role: role, text: value})
                }
            }
        }
        view.text = text(requests); view.complete = true; view.records = {}
        return false
    }
    const next = response.next_cursor
    if (typeof next === 'number' && next > view.cursor && next < view.end) {
        view.cursor = next
        return true
    }
    return fail('Complete requests were not found for this plan review gate. Retry full text.')
}
