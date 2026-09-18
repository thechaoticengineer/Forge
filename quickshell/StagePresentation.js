.pragma library

// Stage status wording shared by the plan list, the stage detail page and Overview.
// Pure JS, so node tests load it too.

function gateStatus(stage) {
  return stage && stage.review_gate ? stage.review_gate.status : undefined
}

// The status line of a stage: the status (with the review-gate reason when blocked), then the commit hash.
function statusText(stage, editingPlan) {
  const gate = gateStatus(stage)
  return (editingPlan && stage.status === "committed"
    ? "committed — locked" : stage.status === "blocked"
      ? (gate === "scope_blocked" ? "blocked · stage cannot be built as written"
        : gate === "design_blocked" ? "blocked · pen.dev export unavailable"
          : gate === "exhausted" ? "blocked · fix rounds exhausted" : "blocked") : stage.status)
    + (stage.sha ? " " + stage.sha : "")
}

function oneLine(value) {
  return typeof value === "string" ? value.replace(/\s+/g, " ").trim() : ""
}

// Why a blocked or failed stage needs attention; "" for every other status.
function attentionReason(stage) {
  if (!stage) return ""
  const gate = stage.review_gate || {}
  if (stage.status === "blocked") {
    const detail = oneLine(gate.reason)
    return gate.status === "scope_blocked" ? detail || "stage cannot be built as written"
      : gate.status === "design_blocked" ? detail || "pen.dev export unavailable"
        : gate.status === "exhausted" ? "fix rounds exhausted"
          : gate.status === "history_blocked" ? detail || "git history changed"
            : detail || oneLine(gate.error) || (gate.status ? "review gate " + gate.status : "blocked")
  }
  if (stage.status === "failed") {
    return oneLine(stage.error) || oneLine(gate.error) || oneLine(gate.reason)
      || oneLine((stage.reassessment || {}).error) || "failed"
  }
  return ""
}
