// Pure helpers for the panel's feature-spec list (M1: format and discovery;
// M2: spec-phase status, review summaries and new-feature validation).

function featureRows(response) {
    var features = response && Array.isArray(response.features) ? response.features : []
    return features.map(function(feature) {
        var reasons = Array.isArray(feature.reasons) ? feature.reasons : []
        var valid = feature.status === "valid"
        return {
            slug: feature.slug,
            title: feature.title,
            path: feature.path,
            status: feature.status,
            statusLabel: valid ? "valid" : "invalid: " + reasons.join("; "),
            valid: valid,
            reasons: reasons
        }
    })
}

function editorCommand(feature) {
    return ["omarchy-launch-editor", feature.path]
}

// Indexes the M2 fields (spec_status, content_hash, latest_review,
// review_current) of a GET /api/features response by slug, so the view can
// look them up without changing featureRows' shape.
function featureSpecsBySlug(response) {
    var features = response && Array.isArray(response.features) ? response.features : []
    var bySlug = {}
    features.forEach(function(feature) {
        bySlug[feature.slug] = {
            slug: feature.slug,
            status: feature.status,
            valid: feature.status === "valid",
            spec_status: feature.spec_status || "draft",
            content_hash: feature.content_hash,
            latest_review: feature.latest_review || null,
            review_current: !!feature.review_current,
            progress: feature.progress || "planned",
            milestones: Array.isArray(feature.milestones) ? feature.milestones.map(milestoneEntry) : []
        }
    })
    return bySlug
}

// M3: keeps every milestone key and defaults the covered scenario IDs, the
// registered business tests and the latest plan link.
function milestoneEntry(milestone) {
    var entry = {}
    Object.keys(milestone || {}).forEach(function(key) { entry[key] = milestone[key] })
    entry.covers = Array.isArray(entry.covers) ? entry.covers : []
    entry.business_tests = Array.isArray(entry.business_tests) ? entry.business_tests : []
    entry.plan = entry.plan && typeof entry.plan === "object" ? entry.plan : null
    return entry
}

// Delegate model data wraps arrays, so Array.isArray() is false there: accept
// any list-like value.
function isList(value) {
    return !!value && typeof value !== "string" && typeof value.length === "number"
}

function milestoneCovers(milestone) {
    if (!milestone) return []
    if (isList(milestone.covers)) return milestone.covers
    return isList(milestone.scenario_ids) ? milestone.scenario_ids : []
}

// "planning", "planned", "completed", "failed" or "" from the milestone's
// latest plan link.
function milestonePlanStatus(milestone) {
    if (!milestone) return ""
    var status = milestone.plan && typeof milestone.plan.status === "string"
        ? milestone.plan.status : milestone.plan_status
    return ["planning", "planned", "completed", "failed"].indexOf(status) !== -1 ? status : ""
}

// The "Plan milestone" action of a milestone row: visible for planned
// milestones that cover scenarios, enabled only for a valid feature whose
// scenarios are approved, with the reason otherwise.
function milestonePlanAction(spec, milestone) {
    if (!milestone || milestone.status === "implemented" || milestoneCovers(milestone).length === 0)
        return { visible: false, enabled: false, reason: "" }
    if (!spec || spec.valid === false || spec.status === "invalid")
        return { visible: true, enabled: false, reason: "Fix the feature validation errors first" }
    if (spec.spec_status !== "scenarios approved")
        return { visible: true, enabled: false, reason: "Approve the scenarios first (status must be scenarios approved)" }
    return { visible: true, enabled: true, reason: "" }
}

function featurePlanRequest(project, slug, milestone) {
    return { project: project, slug: slug, milestone: milestone }
}

function isImplemented(spec) {
    return !!(spec && spec.progress === "implemented")
}

// Implemented features are hidden from the list unless showImplemented is set.
// specs is featureSpecsBySlug's index, which carries each feature's progress.
function visibleRows(rows, specs, showImplemented) {
    var all = Array.isArray(rows) ? rows : []
    if (showImplemented) return all
    return all.filter(function(row) { return !isImplemented(specs && specs[row.slug]) })
}

function implementedCount(rows, specs) {
    return (Array.isArray(rows) ? rows : []).filter(function(row) {
        return isImplemented(specs && specs[row.slug])
    }).length
}

// "implemented", "N/M implemented" while some milestones are done, or "" when
// none are, so the row falls back to its spec status.
function progressLabel(spec) {
    if (!spec) return ""
    if (spec.progress === "implemented") return "implemented"
    var milestones = Array.isArray(spec.milestones) ? spec.milestones : []
    var done = milestones.filter(function(m) { return m.status === "implemented" }).length
    return done > 0 ? done + "/" + milestones.length + " implemented" : ""
}

function specStatusLabel(feature) {
    return (feature && feature.spec_status) ? feature.spec_status : "draft"
}

var SLUG_PATTERN = /^[a-z0-9]+(-[a-z0-9]+)*$/

function validateNewFeature(slug, title) {
    if (typeof slug !== "string" || !SLUG_PATTERN.test(slug))
        return "slug must be lowercase letters, digits and single hyphens"
    if (slug.length > 64)
        return "slug must be at most 64 bytes"
    var titleText = typeof title === "string" ? title : ""
    if (titleText.indexOf("\n") !== -1)
        return "title must be a single line"
    var trimmedTitle = titleText.trim()
    if (trimmedTitle === "")
        return "title is required"
    if (trimmedTitle.length > 200)
        return "title must be at most 200 characters"
    return ""
}

function reviewSummary(review) {
    if (!review) return null
    return {
        label: review.approved ? "approved" : "changes requested",
        summary: review.summary,
        issues: Array.isArray(review.issues) ? review.issues : [],
        questions: Array.isArray(review.questions) ? review.questions : []
    }
}

function canApproveSpec(feature) {
    if (!feature) return false
    var valid = feature.valid === true || feature.status === "valid"
    return !!(valid && feature.latest_review && feature.latest_review.approved === true
        && feature.review_current === true && feature.spec_status === "draft")
}

function canApproveScenarios(feature) {
    return !!(feature && feature.spec_status === "spec approved")
}

// Formats an engine error response, falling back to joined reasons when
// resp.error is missing.
function errorMessage(resp) {
    if (resp && typeof resp.error === "string" && resp.error !== "") return resp.error
    if (resp && Array.isArray(resp.reasons) && resp.reasons.length > 0) return resp.reasons.join("; ")
    return "Request failed"
}
