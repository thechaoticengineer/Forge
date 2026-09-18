// Stage review presentation for the Plan tab: gate text, verdict fields and
// decisions, round labels, and the retained review preview rows. Pure JS apart
// from the item tree walk; Panel.qml keeps loading and verifying the reviews.
.import "ReviewView.js" as ReviewView
.import "UsageFormat.js" as UsageFormat

function reviewGateText(stage) {
  const gate = stage.review_gate || {}
  const policy = stage.review_policy || {}
  const roles = gate.roles || {}
  function outcome(role) {
    return roles[role] === "deferred" ? "deferred to the plan review"
      : roles[role] === "not_required" ? "review not required" : roles[role] || "pending"
  }
  const status = gate.status === "deferred"
    ? (stage.status === "committed" ? "deferred · committed under a deferred review policy"
      : "deferred · awaiting commit under a deferred review policy") : gate.status || "pending"
  return "Review policy: " + (policy.scope || "pending")
    + " · gate: " + status
    + "\nArchitect: " + outcome("architect")
    + " · Independent: " + outcome("reviewer")
}

function reviewStrings(values) {
  const strings = []
  // Nested ListView data may be a QML sequence rather than a JS Array.
  if (values && typeof values !== "string" && typeof values.length === "number") {
    for (let i = 0; i < values.length; i++) {
      if (typeof values[i] === "string") strings.push(values[i])
    }
  }
  return strings
}

function reviewFields(verdict) {
  const fields = []
  for (const kind of ["issues", "notes", "checks"]) {
    reviewStrings(verdict[kind]).forEach(function(text, i) {
      const label = kind === "issues" ? "Change request" : kind === "checks" ? "Verified check"
        : verdict.approved === true ? "Legacy optional note" : "Legacy note (change request)"
      fields.push({kind: kind, label: label + " " + (i + 1), text: text})
    })
  }
  return fields
}

function stageReviews(stage) {
  const entries = []
  const saved = stage.reviews || []
  for (let i = 0; i < saved.length; i++) {
    if (saved[i] && typeof saved[i] === "object")
      entries.push({ verdict: saved[i], round: saved[i].round })
  }
  if (entries.length === 0 && stage.last_verdict) {
    // A live round describes current work, not the older fallback verdict.
    // Wrap records for display only; never fill in or rewrite saved history.
    entries.push({ verdict: stage.last_verdict, round: stage.last_verdict.round
      || (stage.status !== "in_progress" ? stage.rounds : null) })
  }
  return entries
}

function reviewDecision(verdict) {
  const issues = reviewStrings(verdict.issues)
  const notes = reviewStrings(verdict.notes)
  const requests = issues.concat(notes)
    .filter(function(request, index, all) { return all.indexOf(request) === index })
  const clean = verdict.approved === true && issues.length === 0 && notes.length === 0
  const optionalNotes = verdict.approved === true && issues.length === 0 && notes.length > 0
  const label = clean ? "approved"
    : optionalNotes ? "approved with optional notes"
    : verdict.approved === true ? "legacy approval with change requests" : "changes requested"
  return { clean: clean, optionalNotes: optionalNotes,
    label: label + (clean || optionalNotes ? "" : " · " + requests.length
    + (requests.length === 1 ? " request" : " requests")) }
}

function reviewRoundLabel(entry) {
  const round = UsageFormat.nonNegativeInt(entry.round)
  return round !== null && round > 0 ? "round " + round : "round unknown"
}

function reviewTimestamp(verdict) {
  const unix = UsageFormat.nonNegativeInt(verdict.unix)
  return unix === null ? "" : new Date(unix * 1000).toISOString()
}

function stageReviewHasSelection(item) {
  if (!item) return false
  if (item.stageProseField === true && item.hasSelection) return true
  return Array.from(item.children || []).some(function(child) { return stageReviewHasSelection(child) })
}

function stageReviewHasHeldPreview(presentation, view) {
  // Incomplete rows still to load are not a held preview.
  if (view.rows.some(function(row) { return row.complete === false })) return false
  for (let i = 0; i < presentation.count; i++) {
    if (!presentation.get(i).record.complete) return true
  }
  return false
}

// view is the stage's ReviewView state and detailScope its stage detail scope.
function reconcileStageReviewPresentation(view, detailScope, presentation, repeater) {
  // Presentation is separate from ReviewView's current verification authority.
  // A selected original stays in its existing editor, with its source scope;
  // it never makes a new scope's preview complete or suppresses verification.
  const scope = view.scope.key
  if (presentation.count && presentation.get(0).detailScope !== detailScope) presentation.clear()
  const desired = []
  view.rows.forEach(function(row) {
    const identity = ReviewView.identity(row.verdict)
    let found = -1
    for (let i = 0; i < presentation.count; i++) {
      const entry = presentation.get(i)
      if (desired.indexOf(entry.token) >= 0) continue
      // A selected preview must not hide a newly available complete record.
      if (!entry.record.complete && row.complete && stageReviewHasSelection(repeater.itemAt(i))) continue
      if ((entry.sourceScope === scope && entry.record.position === row.position)
        || (identity && identity === ReviewView.identity(entry.record.verdict))) { found = i; break }
    }
    if (found < 0) {
      const token = JSON.stringify([scope, row.position, row.complete])
      presentation.append({token: token, detailScope: detailScope, sourceScope: scope, record: row})
      desired.push(token)
    } else {
      const entry = presentation.get(found)
      desired.push(entry.token)
      if (!stageReviewHasSelection(repeater.itemAt(found))
        && (entry.sourceScope !== scope || (!entry.record.complete && row.complete))) {
        presentation.setProperty(found, "record", row)
        presentation.setProperty(found, "sourceScope", scope)
      }
    }
  })
  for (let i = presentation.count - 1; i >= 0; i--) {
    if (desired.indexOf(presentation.get(i).token) < 0 && !stageReviewHasSelection(repeater.itemAt(i)))
      presentation.remove(i)
  }
  // Reorder by current history without destroying existing delegates/editors.
  // Unmatched selected originals remain at the end, explicitly source-labelled.
  for (let i = 0; i < desired.length; i++) {
    for (let j = i; j < presentation.count; j++) {
      if (presentation.get(j).token === desired[i]) {
        if (i !== j) presentation.move(j, i, 1)
        break
      }
    }
  }
}
