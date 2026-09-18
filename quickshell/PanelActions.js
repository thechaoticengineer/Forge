.pragma library

// Panel actions: one guard per action, shared by its button and its keyboard
// shortcut, the phase-driven Overview action list, the ⋯ menu and the endpoints.
// Pure JS, so node tests compare the guards with the pre-redesign expressions.

// f: {engineOnline, busy, phase, queueActive, editingPlan, revisePending, chatPending,
// planStatus (null without a plan), goalText, feedbackText, questionText,
// goalEnhancePending, hasQueuedGoals}. Each guard is the old enabled expression.
function guards(f) {
  return {
    createPlan: !f.editingPlan && !f.revisePending && !f.busy && f.goalText.trim() !== "",
    enhance: f.engineOnline && !f.busy && !f.editingPlan && !f.revisePending
      && !f.goalEnhancePending && f.goalText.trim() !== "",
    refactor: !f.editingPlan && !f.revisePending && !f.busy && f.engineOnline,
    addToQueue: f.engineOnline && f.goalText.trim() !== "",
    approve: !f.editingPlan && !f.revisePending && !f.busy
      && f.planStatus !== null && f.planStatus === "draft",
    run: !f.editingPlan && !f.revisePending && !f.busy && f.planStatus !== null
      && (f.planStatus === "approved" || f.planStatus === "done"),
    stop: f.phase === "running" || f.queueActive,
    editPlan: !f.editingPlan && !f.revisePending && f.engineOnline && !f.busy && !f.queueActive
      && f.planStatus !== null && ["draft", "approved", "done"].indexOf(f.planStatus) !== -1,
    discard: !f.editingPlan && !f.revisePending && !f.busy && f.planStatus !== null,
    diff: f.engineOnline,
    features: f.engineOnline,
    update: f.engineOnline && !f.busy,
    improve: f.engineOnline && !f.busy && !f.queueActive
      && !f.editingPlan && !f.revisePending && f.planStatus !== null
      && ["draft", "approved", "done"].indexOf(f.planStatus) !== -1
      && f.feedbackText.trim() !== "",
    ask: f.engineOnline && !f.busy && !f.queueActive
      && !f.editingPlan && !f.revisePending && !f.chatPending
      && f.planStatus !== null && f.questionText.trim() !== "",
    startQueue: !f.editingPlan && f.engineOnline && !f.busy && !f.queueActive && f.hasQueuedGoals,
    changeProject: f.engineOnline
  }
}

// The actions Overview shows for the current phase, in order. An action with a
// guard is listed only while that guard is true; discuss has no guard.
function overviewActions(f, g) {
  let ids
  if (f.busy || g.stop) ids = ["stop"]
  else if (f.planStatus === "draft") ids = ["approve", "editPlan"]
  else if (f.planStatus === "approved" || f.planStatus === "done") ids = ["run", "editPlan"]
  else ids = ["createPlan", "discuss", "enhance", "addToQueue"]
  return ids.filter(function(id) { return !(id in g) || g[id] === true })
}

// Rare or destructive actions of the ⋯ menu, enabled by the same guards as before.
function overflowItems(f) {
  const g = guards(f)
  return [
    { id: "update", label: "Update Forge", enabled: g.update },
    { id: "discard", label: "Discard plan", enabled: g.discard },
    { id: "refactor", label: "Refactor plan", enabled: g.refactor },
    { id: "diff", label: "View diff", enabled: g.diff },
    { id: "changeProject", label: "Change project", enabled: g.changeProject },
    { id: "help", label: "Keyboard help", enabled: true }
  ]
}

var endpoints = {
  createPlan: "/api/plan",
  refactor: "/api/plan",
  enhance: "/api/goal/enhance",
  addToQueue: "/api/queue/add",
  approve: "/api/approve",
  run: "/api/run",
  stop: "/api/stop",
  discard: "/api/reset_plan",
  update: "/api/self_update",
  improve: "/api/plan/revise",
  ask: "/api/plan/chat",
  startQueue: "/api/queue/start"
}

// Where every pre-redesign button and indicator lives after the redesign.
var locations = {
  "? Help": ["overflow"],
  "Change project": ["settings", "overflow"],
  "planner": ["settings"],
  "architect": ["settings"],
  "automatic routing": ["settings"],
  "implementer": ["settings"],
  "reviewer": ["settings"],
  "architect review": ["settings"],
  "reviewer review": ["settings"],
  "push at end": ["settings"],
  "auto-approve": ["settings"],
  "Refresh models": ["settings"],
  "Cancel refresh": ["settings"],
  "Refresh Claude limits": ["settings"],
  "Model settings & options": ["settings"],
  "Create plan": ["overview"],
  "Enhance with AI": ["overview"],
  "Apply AI description": ["overview"],
  "Undo enhance": ["overview"],
  "Refactor plan": ["overflow"],
  "Add to queue": ["overview"],
  "Plan is OK — approve": ["overview"],
  "Start implementing": ["overview"],
  "Stop": ["overview"],
  "Edit plan": ["overview", "plan"],
  "Discard plan": ["overflow"],
  "View diff": ["overflow"],
  "Features": ["features"],
  "Update Forge": ["settings", "overflow"],
  "Discuss before planning": ["overview"],
  "Improve with AI": ["plan"],
  "Plan Q&A": ["plan"],
  "Ask": ["plan"],
  "Start queue": ["queue"],
  "↑": ["queue"],
  "↓": ["queue"],
  "×": ["queue"],
  "project session markers": ["header"],
  "phase badge": ["header"],
  "current step": ["header"],
  "background project activity": ["header"],
  "now working card": ["overview"],
  "quota summary": ["settings"],
  "catalogue metadata": ["settings"],
  "model policy error": ["settings"],
  "goal enhancement status": ["overview"],
  "discussion status": ["overview"],
  "architecture card": ["architecture"],
  "role token totals": ["architecture"],
  "plan review status": ["architecture"],
  "stage list": ["plan", "overview"],
  "live output": ["activity"],
  "history": ["activity"],
  "reports": ["activity"],
  "queue list": ["queue"]
}
