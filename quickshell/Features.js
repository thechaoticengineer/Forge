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

// ---- M5: the feature page --------------------------------------------------

var FEATURE_PAGE_TABS = ["README", "Scenarios", "Decisions", "Milestones", "Design"]

function featurePageTabs() {
    return FEATURE_PAGE_TABS.slice()
}

// The tab shown after stepping from `tab` by `step` (wrapping); an unknown tab
// counts as the first.
function stepFeaturePageTab(tab, step) {
    var count = FEATURE_PAGE_TABS.length
    var at = Math.max(0, FEATURE_PAGE_TABS.indexOf(tab))
    return FEATURE_PAGE_TABS[((at + step) % count + count) % count]
}

// Splits Markdown into [{kind: "markdown", text} | {kind: "mermaid", text}].
// Fenced ```mermaid blocks become Mermaid segments holding the block body;
// every other line, including other code fences, stays Markdown unchanged.
// Whitespace-only Markdown between two blocks is dropped.
function markdownSegments(text) {
    var lines = typeof text === "string" ? (text.match(/[^\n]*\n|[^\n]+/g) || []) : []
    var segments = []
    var current = []
    var fence = null
    var mermaid = false

    function flush(kind) {
        var joined = current.join("")
        current = []
        if (kind === "markdown" && joined.trim() === "") return
        segments.push({ kind: kind, text: joined })
    }

    lines.forEach(function(line) {
        var opening = /^ {0,3}(`{3,}|~{3,})\s*(\S*)/.exec(line)
        if (fence === null) {
            if (opening && opening[2].toLowerCase() === "mermaid") {
                flush("markdown")
                fence = opening[1]
                mermaid = true
            } else {
                if (opening) {
                    fence = opening[1]
                    mermaid = false
                }
                current.push(line)
            }
            return
        }
        var closing = /^ {0,3}(`{3,}|~{3,})\s*$/.exec(line)
        var closes = !!closing && closing[1].charAt(0) === fence.charAt(0) && closing[1].length >= fence.length
        if (closes) {
            if (mermaid) flush("mermaid")
            else current.push(line)
            fence = null
            mermaid = false
        } else {
            current.push(line)
        }
    })
    if (fence !== null) flush(mermaid ? "mermaid" : "markdown")
    else flush("markdown")
    return segments
}

// "2027-01-15 08:00" in the local time zone, or "" for a missing time.
function formatResultTime(unix) {
    if (typeof unix !== "number" || !isFinite(unix)) return ""
    var date = new Date(unix * 1000)
    function pad(n) { return n < 10 ? "0" + n : "" + n }
    return date.getFullYear() + "-" + pad(date.getMonth() + 1) + "-" + pad(date.getDate())
        + " " + pad(date.getHours()) + ":" + pad(date.getMinutes())
}

// What a scenario row shows for its latest recorded review result. Evidence is
// shown only for failed results.
function scenarioResultView(scenario) {
    var result = scenario && scenario.result && typeof scenario.result === "object" ? scenario.result : null
    if (!result || (result.status !== "passed" && result.status !== "failed"))
        return { label: "not recorded", evidence: "", when: "", plan: "", outOfDate: false }
    return {
        label: result.status,
        evidence: result.status === "failed" && typeof result.evidence === "string" ? result.evidence : "",
        when: formatResultTime(result.unix),
        plan: typeof result.plan_id === "string" ? result.plan_id : "",
        outOfDate: result.out_of_date === true
    }
}

// Normalises the design entries of GET /api/features/content into the items
// the Design tab shows, in order: each .pen paired with its exported PNG (or
// a note when none exists), each Mermaid file with its source, and any PNG
// that no .pen claims. Other files are not listed.
function designItems(design) {
    var entries = isList(design) ? Array.prototype.slice.call(design) : []
    var pngs = {}
    var claimed = {}
    entries.forEach(function(entry) {
        if (entry && entry.kind === "png") pngs[entry.path] = entry
    })
    var items = []
    entries.forEach(function(entry) {
        if (!entry) return
        if (entry.kind === "pen") {
            var png = entry.png ? pngs[entry.png] : null
            if (png) claimed[png.path] = true
            items.push({ kind: "pen", label: entry.path, image: png && png.absolute_path ? png.absolute_path : null,
                note: png && png.absolute_path ? "" : "no export exists", text: "", error: "" })
        } else if (entry.kind === "mermaid") {
            items.push({ kind: "mermaid", label: entry.path, image: null, note: "",
                text: typeof entry.text === "string" ? entry.text : "",
                error: typeof entry.error === "string" ? entry.error : "" })
        }
    })
    entries.forEach(function(entry) {
        if (entry && entry.kind === "png" && !claimed[entry.path] && entry.absolute_path)
            items.push({ kind: "png", label: entry.path, image: entry.absolute_path, note: "", text: "", error: "" })
    })
    return items
}

// A file:// URL for an absolute path, escaping characters a URL treats specially.
function fileUrl(path) {
    return "file://" + String(path).split("/").map(encodeURIComponent).join("/")
}

// The status header of the feature page: validation, spec status, milestone
// progress, the latest spec review verdict and every milestone plan status.
// `state` is the GET /api/features/state response (or any {valid, reasons}).
function featurePageHeader(spec, state) {
    var reasons = state && Array.isArray(state.reasons) ? state.reasons : []
    var valid = spec ? spec.valid !== false && spec.status !== "invalid" : !!(state && state.valid)
    if (state && state.valid === false) valid = false
    var milestones = spec && Array.isArray(spec.milestones) ? spec.milestones : []
    var done = milestones.filter(function(m) { return m.status === "implemented" }).length
    var review = spec && spec.latest_review ? spec.latest_review : null
    return {
        validation: valid ? "valid" : "invalid",
        reasons: reasons,
        specStatus: spec && spec.spec_status ? spec.spec_status : "draft",
        progress: done + "/" + milestones.length + " implemented",
        review: review ? { verdict: review.approved ? "approved" : "changes requested", current: !!spec.review_current } : null,
        milestonePlans: milestones.filter(function(m) { return milestonePlanStatus(m) !== "" })
            .map(function(m) { return { id: m.id, status: milestonePlanStatus(m) } })
    }
}

// The actions of the feature page, enabled under the same rules as the
// Features tab: Request review, Approve spec, Approve scenarios and one Plan
// milestone entry per milestone the tab offers one for.
function featurePageActions(spec, activity) {
    var valid = !!spec && spec.valid !== false && spec.status !== "invalid"
    var running = !!activity && activity.status === "running" && (!spec || !spec.slug || activity.slug === spec.slug)
    var status = spec && spec.spec_status ? spec.spec_status : "draft"
    var review = { id: "review", label: "Request review", enabled: valid && !running, reason: "" }
    if (!valid) review.reason = "Fix the feature validation errors first"
    else if (running) review.reason = "A feature activity is already running"
    var approveSpec = { id: "approveSpec", label: "Approve spec", enabled: canApproveSpec(spec), reason: "" }
    if (!approveSpec.enabled) {
        approveSpec.reason = !valid ? "Fix the feature validation errors first"
            : status !== "draft" ? "The spec is already approved"
            : "Request a review first: the latest review must approve the current content"
    }
    var approveScenarios = { id: "approveScenarios", label: "Approve scenarios", enabled: canApproveScenarios(spec), reason: "" }
    if (!approveScenarios.enabled) {
        approveScenarios.reason = status === "scenarios approved" ? "The scenarios are already approved"
            : "Approve the spec first (status must be spec approved)"
    }
    var actions = [review, approveSpec, approveScenarios]
    var milestones = spec && Array.isArray(spec.milestones) ? spec.milestones : []
    milestones.forEach(function(milestone) {
        var plan = milestonePlanAction(spec, milestone)
        if (!plan.visible) return
        actions.push({ id: "plan", label: "Plan milestone " + milestone.id, milestone: milestone.id,
            enabled: plan.enabled, reason: plan.reason })
    })
    return actions
}
