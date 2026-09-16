pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
// qs.Ui exports its own Button, which shadows the Controls one and carries a
// different API. Controls-specific buttons name it explicitly.
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "DetailView.js" as DetailView
import "ReviewView.js" as ReviewView
import "PlanReview.js" as PlanReview
import "PanelDetails.js" as PanelDetails
import "ModelRouting.js" as ModelRouting
import "UsageFormat.js" as UsageFormat
import "GoalEnhancement.js" as GoalEnhancement
import "Discussion.js" as Discussion
import "PlanEdit.js" as PlanEdit
import "CataloguePresentation.js" as CataloguePresentation

Item {
  id: root

  property var shell: null
  property var manifest: null
  property bool closingFromHost: false

  // Engine snapshot, refreshed by polling http://127.0.0.1:8734/api/state.
  property var engineState: null
  property bool engineOnline: false
  property string apiBase: "http://127.0.0.1:8734"
  property string localError: ""
  property int expandedStageId: -1
  property bool stageRoutingExpanded: false
  property var stageSnapshot: null
  onExpandedStageIdChanged: {
    stageRoutingExpanded = false
    if (expandedStageId === -1) stageSnapshot = null
    else syncStageSnapshot()
  }
  onDisplayedStagesChanged: syncStageSnapshot()
  readonly property string expandedStageScope: {
    const stage = (displayedStages || []).find(s => s.id === expandedStageId)
    return stage ? stageDetailScope(stage) : ""
  }
  onExpandedStageScopeChanged: syncStageSnapshot()
  property int selectedStageIndex: -1
  property string lastProject: ""
  property int projectViewRevision: 0
  property var goalDrafts: ({})
  property bool revisePending: false
  property bool chatPending: false
  property bool goalEnhancePending: false
  property int goalEnhanceRequest: -1
  property string goalEnhanceSent: ""
  property string goalEnhanceReady: ""
  property string goalEnhanceUndo: ""
  property string goalEnhanceError: ""
  property bool chatExpanded: false
  property bool discussionPending: false
  property int discussionRequest: -1
  property string discussionSent: ""
  property string discussionError: ""
  // The chat page being current is the single source of truth for "open".
  readonly property bool discussionOpen: panelStack.currentItem === discussionView
  property bool editingPlan: false
  property var editStages: []
  property string editGoal: ""
  property string editProject: ""
  property int editRevision: 0
  property int editSession: 0
  property bool editPending: false
  property var editFocusedField: null
  property int stateRequestSerial: 0
  property int stateResponseSerial: 0
  readonly property var displayedStages: editingPlan ? editStages
    : plan && plan.stages ? plan.stages : []
  readonly property bool editValid: {
    // Field changes must update validation without replacing the ListView model.
    const revision = editRevision
    return editStages.length > 0 && editStages.every(function(stage) {
      return stage.status === "committed"
        || (stage.title.trim() !== "" && stage.instructions.trim() !== "")
    })
  }

  readonly property string pluginId: manifest && manifest.id
    ? manifest.id : "dev.omarchy-ai-build-orchestrator"
  readonly property color foreground: Color.foreground
  readonly property color mutedForeground: Qt.darker(Color.foreground, 1.45)
  readonly property color background: Color.background
  readonly property color surface: Color.popups.background
  readonly property color accent: Color.accent
  readonly property color urgent: Color.urgent
  readonly property string fontFamily: Style.font.family

  // Semantic status colors from the active theme's full palette; the shell's
  // Color singleton only exposes five roles, so read colors.toml directly.
  property color success: "#4faf72"
  property color working: "#d5a542"
  property color info: "#56a8c7"

  function loadPalette(raw) {
    function grab(key, fallback) {
      const m = String(raw).match(new RegExp('^' + key + '\\s*=\\s*"([^"]+)"', "m"))
      return m ? m[1] : fallback
    }
    success = grab("green", success)
    working = grab("yellow", working)
    info = grab("cyan", info)
  }

  FileView {
    path: Quickshell.env("HOME") + "/.local/state/omarchy/current/theme/colors.toml"
    watchChanges: true
    printErrors: false
    onLoaded: root.loadPalette(text())
    onFileChanged: reload()
  }

  // Font size in design pixels; Style.fontPx takes a multiplier of the 12px base.
  function fs(px) { return Style.fontPx(px / 12) }

  readonly property var plan: engineState ? engineState.plan : null
  readonly property var architecture: engineState && engineState.architecture ? engineState.architecture : null
  readonly property var sessions: engineState && engineState.sessions ? engineState.sessions : []
  readonly property string activeProject: engineState
    ? engineState.active_project || engineState.project : ""
  readonly property int activeProjectCount: sessions.filter(function(session) {
    return session.busy || session.queue_active
  }).length
  readonly property bool backgroundBusy: sessions.some(function(session) {
    return session.project !== root.activeProject && (session.busy || session.queue_active)
  })
  readonly property var queue: engineState && engineState.queue ? engineState.queue : []
  readonly property var chat: engineState && engineState.chat ? engineState.chat : []
  readonly property var discussion: engineState && engineState.discussion ? engineState.discussion : []
  readonly property bool queueActive: engineState !== null && engineState.queue_active === true
  readonly property var queueHead: queue.find(function(item) { return item.status !== "done" }) || null
  readonly property bool hasQueuedGoals: queueHead !== null && ["queued", "blocked", "failed", "planning", "awaiting_approval", "running"].includes(queueHead.status)
  function canMoveQueueGoal(index, step) {
    for (let i = index + step; i >= 0 && i < queue.length; i += step) {
      if (queue[i].status !== "done") return queue[i].status === "queued"
    }
    return false
  }
  readonly property string phase: engineState ? engineState.phase : "offline"
  readonly property string currentActivity: {
    const stage = plan && plan.stages ? plan.stages.find(function(stage) {
      return engineState && stage.id === engineState.current_stage
    }) : null
    return stage ? stageActivity(stage) : ""
  }
  // Only the displayed project's work gates its controls, never background sessions.
  readonly property bool busy: phase === "planning" || phase === "running"
    || (engineState !== null && engineState.busy === true)
    || (engineState !== null && engineState.current_step.indexOf("cloning") === 0)
  readonly property string projectName: engineState
    ? engineState.project.split("/").filter(function(p) { return p !== "" }).pop() || "?"
    : "?"

  readonly property bool discussionCanSend: engineOnline && !busy && !queueActive
    && !editingPlan && !revisePending && !discussionPending
  readonly property bool discussionCanPlan: discussionCanSend && Discussion.canPlanFromDiscussion(discussion)
  readonly property bool discussionCanClear: engineOnline && !busy && !queueActive && discussion.length > 0

  readonly property var planReview: plan && plan.plan_review ? plan.plan_review : null
  readonly property string planReviewScope: PlanReview.scope(lastProject, projectViewRevision, plan)
  property var planReviewView: null
  property int planReviewVersion: 0
  property bool planReviewExpanded: false
  onPlanReviewScopeChanged: {
    planReviewView = plan && plan.plan_review ? PlanReview.create(lastProject, plan) : null
    planReviewExpanded = false
    planReviewVersion++
  }

  function loadPlanReviewRequests() {
    const view = planReviewView, scope = planReviewScope
    if (!view) return
    const started = PlanReview.begin(view)
    planReviewVersion++
    if (!started) return
    api("GET", PlanReview.path(view), null, function(resp, status) {
      if (root.planReviewScope !== scope || root.planReviewView !== view) return
      const more = PlanReview.finish(view, resp, status)
      root.planReviewVersion++
      if (more) root.loadPlanReviewRequests()
    }, true)
  }

  function reviewCadenceLabel(state, role) {
    const cadence = state && state.settings ? state.settings.review_cadence : null
    return !cadence || cadence[role] === undefined ? "… (unavailable)"
      : cadence[role] === "per_plan" ? "per plan" : "per stage"
  }

  function toggleReviewCadence(role) {
    const settings = engineState && engineState.settings ? engineState.settings : {}
    const current = settings.review_cadence || {}
    const cadence = {architect: current.architect === "per_plan" ? "per_plan" : "per_stage",
      reviewer: current.reviewer === "per_plan" ? "per_plan" : "per_stage"}
    cadence[role] = cadence[role] === "per_plan" ? "per_stage" : "per_plan"
    act("/api/settings", {review_cadence: cadence})
  }

  function planReviewStatusText(review) {
    if (!review) return ""
    const gate = review.gate || {}, roles = gate.roles || {}
    function outcome(role) {
      const value = roles[role]
      return value === "not_required" ? "review not required"
        : value === "deferred" ? "deferred to the plan review" : value || "unavailable"
    }
    const round = typeof review.rounds === "number" ? review.rounds : "—"
    const maximum = typeof review.budget === "number" ? review.budget + 1 : "—"
    return "Plan review: " + (review.status || "unavailable")
      + " · round " + round + " of " + maximum
      + "\nCurrent gate: " + (gate.status || "unavailable")
      + "\nArchitect: " + outcome("architect") + " · Independent: " + outcome("reviewer")
      + (review.fix_sha ? "\nFix commit: " + review.fix_sha : "")
  }

  property bool chooserOpen: false
  property var projectsData: null
  property bool manualEntry: false

  property bool diffOpen: false
  property string diffText: ""
  property bool diffPending: false
  property string diffError: ""

  property var catalogueDetails: null
  property bool catalogueOpen: false
  property string catalogueDraft: ""
  property bool catalogueAiPending: false
  property int catalogueAiRequest: -1
  property string catalogueAiSent: ""
  property string catalogueAiReady: ""
  property string catalogueAiUndo: ""
  property string catalogueAiMessage: ""
  property bool catalogueWasRefreshing: false
  readonly property var catalogue: engineState && engineState.model_catalogue ? engineState.model_catalogue : null
  onCatalogueChanged: {
    const refreshing = !!catalogue && catalogue.refreshing
    if (catalogueWasRefreshing && !refreshing && catalogueOpen) {
      api("GET", "/api/models", null, function(resp, status) {
        if (status === 200 && resp) root.catalogueDetails = resp
      })
    }
    catalogueWasRefreshing = refreshing
  }

  function changeModelConstraint(index, key, value) {
    const stage = editStages[index]
    changeStageField(index, "model_constraint", PlanEdit.modelConstraint(stage, key, value))
  }

  function openCatalogue() {
    if (root.catalogueAiPending || root.catalogueAiUndo || root.catalogueAiReady) {
      root.catalogueOpen = true
      return
    }
    api("GET", "/api/models", null, function(resp, status) {
      if (status !== 200 || !resp) return
      root.catalogueDetails = resp
      root.catalogueDraft = JSON.stringify(resp.policy, null, 2)
      root.catalogueOpen = true
    })
  }
  function saveCatalogue() {
    let policy
    try { policy = JSON.parse(catalogueEditorView.editor.text) }
    catch (e) { root.localError = "Model policy must be valid JSON: " + e; return }
    act("/api/settings", { model_catalogue: policy,
      expected_model_policy: root.catalogueDetails ? root.catalogueDetails.policy : undefined }, function(resp, status) {
      if (status === 200) {
        root.catalogueAiUndo = ""
        root.catalogueAiReady = ""
        root.catalogueAiMessage = ""
        root.openCatalogue()
      } else {
        root.catalogueAiMessage = resp && resp.error ? resp.error : "Could not save model policy."
      }
    })
  }

  function reloadCatalogue() {
    catalogueAiUndo = ""
    catalogueAiReady = ""
    catalogueAiMessage = ""
    root.openCatalogue()
  }

  function suggestCatalogue() {
    if (catalogueAiPending) return
    let policy
    try { policy = JSON.parse(catalogueEditorView.editor.text) }
    catch (e) { root.catalogueAiMessage = "Model policy must be valid JSON: " + e; return }
    const revision = projectViewRevision
    catalogueAiSent = catalogueEditorView.editor.text
    catalogueAiPending = true
    catalogueAiRequest = -1
    catalogueAiReady = ""
    catalogueAiMessage = "AI is checking official sources and selecting up to 4 models per provider…"
    api("POST", "/api/models/suggest", { project: lastProject, policy: policy }, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (status === 202 && resp) {
        root.catalogueAiRequest = resp.request_id
        root.refresh()
        root.syncCatalogueSuggestion()
      } else {
        root.catalogueAiPending = false
        root.catalogueAiMessage = resp && resp.error ? resp.error : "Could not request AI tiers. Check the engine connection."
      }
    }, true)
  }

  function syncCatalogueSuggestion() {
    const snapshot = engineState ? engineState.model_policy_suggestion : null
    if (catalogueAiRequest < 0 || !snapshot || snapshot.request_id !== catalogueAiRequest
        || snapshot.status === "running") return
    const requestId = catalogueAiRequest
    if (snapshot.status === "failed") {
      catalogueAiRequest = -1
      catalogueAiPending = false
      catalogueAiMessage = snapshot.error || "Could not assign model tiers."
      return
    }
    if (snapshot.status !== "ready") return
    catalogueAiRequest = -1
    const revision = projectViewRevision
    api("GET", "/api/models/suggestion?project=" + encodeURIComponent(lastProject), null, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      root.catalogueAiPending = false
      if (status !== 200 || !resp || resp.request_id !== requestId || resp.status !== "ready" || !resp.policy) {
        root.catalogueAiMessage = "Could not load AI tiers. Try again."
        return
      }
      root.catalogueAiReady = JSON.stringify(resp.policy, null, 2)
      root.catalogueAiMessage = "AI tier suggestions — review and save to use them.\n" + resp.summary
        + (resp.sources && resp.sources.length ? "\n" + resp.sources.map(function(s) {
          return s.provider + ": " + s.status + " · " + s.url
        }).join("\n") : "")
        + (resp.warnings && resp.warnings.length ? "\n" + resp.warnings.join("\n") : "")
        + (resp.cost_evidence && resp.cost_evidence.length ? "\n" + resp.cost_evidence.map(function(c) {
          return c.provider + "/" + c.model + ": cost preference "
            + (c.relative_cost_preference === null ? "unknown" : c.relative_cost_preference)
            + (c.source_url ? " · standard API USD/1M tokens input " + c.input_per_million
              + ", output " + c.output_per_million + " · " + c.source_url : "")
        }).join("\n") : "")
        + (resp.reasons && resp.reasons.length ? "\n" + resp.reasons.map(function(r) {
          return r.provider + "/" + r.model + ": " + r.rationale
        }).join("\n") : "")
      if (catalogueEditorView.editor.text === root.catalogueAiSent) root.applyCatalogueSuggestion()
    }, true)
  }

  function applyCatalogueSuggestion() {
    if (catalogueAiReady === "") return
    catalogueAiUndo = catalogueEditorView.editor.text
    catalogueEditorView.editor.text = catalogueAiReady
    catalogueDraft = catalogueAiReady
    catalogueAiReady = ""
  }

  function undoCatalogueSuggestion() {
    catalogueEditorView.editor.text = catalogueAiUndo
    catalogueDraft = catalogueAiUndo
    catalogueAiUndo = ""
    catalogueAiMessage = "AI changes undone."
  }

  property bool helpOpen: false
  readonly property bool insertMode: goalField.activeFocus
    || feedbackField.activeFocus || questionField.activeFocus || discussionView.input.activeFocus
    || projectChooser.filterField.activeFocus || projectChooser.manualField.activeFocus || catalogueEditorView.editor.activeFocus
    || editFocusedField !== null

  onHelpOpenChanged: {
    keyHandler.pendingKey = ""
    keyHandler.forceActiveFocus()
  }

  onDiffOpenChanged: {
    keyHandler.pendingKey = ""
    keyHandler.forceActiveFocus()
  }

  onChooserOpenChanged: {
    keyHandler.pendingKey = ""
    if (chooserOpen) projectChooser.chooserList.resetSelection()
    keyHandler.forceActiveFocus()
  }

  readonly property var agent: engineState ? engineState.agent : null
  readonly property bool agentActive: agent !== null && agent !== undefined
    && agent.role !== ""
  readonly property string agentSession: agentActive
    ? JSON.stringify([engineState.project, engineState.run_started_unix,
                      agent.started_unix, agent.role, agent.tool, agent.model]) : ""
  property double agentNow: Date.now() / 1000
  property bool liveTab: false
  property string historyFilter: "all"
  readonly property var reports: engineState && Array.isArray(engineState.reports)
    ? engineState.reports.filter(function(report) { return report && typeof report === "object" })
      .sort(function(a, b) { return (b.unix || 0) - (a.unix || 0) }) : []
  readonly property bool hasReports: reports.length > 0
  readonly property bool reportsVisible: !liveTab && historyFilter === "reports"
  property string selectedReportKey: ""
  property string expandedReportKey: ""
  onReportsChanged: {
    if (reports.length === 0 && historyFilter === "reports") historyFilter = "all"
  }
  property var logFeed: DetailView.newFeed()
  property string logError: ""
  ListModel { id: liveEntries }
  ListModel { id: historyEntries }

  function syncHistory() {
    agentOutput.historyList.beginUpdate()
    DetailView.reconcile(historyEntries, engineState ? engineState.history || [] : [], lastProject, "history")
    DetailView.filterRows(historyEntries, historyFilter)
    agentOutput.historyList.endUpdate()
  }
  onHistoryFilterChanged: {
    agentOutput.historyList.beginUpdate()
    DetailView.filterRows(historyEntries, historyFilter)
    agentOutput.historyList.endUpdate()
  }

  onEngineStateChanged: {
    const project = engineState ? engineState.project : ""
    if (project === lastProject) { syncHistory(); syncGoalEnhancement(); syncCatalogueSuggestion(); syncReviewViews(); syncDiscussion(); return }
    if (lastProject !== "") goalDrafts[lastProject] = goalField.text
    lastProject = project
    // Ignore log/diff responses from an earlier visit, even after switching back.
    projectViewRevision++
    catalogueAiPending = false
    catalogueAiRequest = -1
    catalogueAiMessage = ""
    reviewViews = ({})
    stageReviewBlocks = ({})
    stageSnapshot = null
    stageRoutingExpanded = false
    cancelPlanEdit()
    goalField.text = goalDrafts[project] || ""
    feedbackField.text = ""
    revisePending = false
    questionField.text = ""
    chatPending = false
    goalEnhancePending = false
    goalEnhanceRequest = -1
    goalEnhanceSent = ""
    goalEnhanceReady = ""
    goalEnhanceUndo = ""
    goalEnhanceError = ""
    chatExpanded = false
    chatList.followTail = true
    chatList.readingY = 0
    discussionPending = false
    discussionRequest = -1
    discussionSent = ""
    discussionError = ""
    closeDiscussion()
    discussionView.input.text = ""
    goalFlick.contentY = 0
    expandedStageId = -1
    selectedStageIndex = -1
    diffOpen = false
    diffText = ""
    diffError = ""
    diffPending = false
    localError = ""
    DetailView.invalidate(logFeed)
    logFeed = DetailView.newFeed()
    logError = ""
    liveEntries.clear()
    historyEntries.clear()
    agentOutput.liveOutput.resetView()
    agentOutput.historyList.resetView()
    liveTab = busy
    historyFilter = "all"
    agentOutput.historyList.positionViewAtBeginning()
    selectedReportKey = ""
    expandedReportKey = ""
    agentOutput.reportList.readingY = 0
    agentOutput.reportList.positionViewAtBeginning()
    syncHistory()
    Qt.callLater(refreshAgentLog)
  }

  onBusyChanged: {
    // Keep an inspected live entry visible when the agent finishes.
    if (busy) liveTab = true
    if (!busy) chatPending = false
    Qt.callLater(refreshAgentLog)
  }
  onAgentSessionChanged: {
    DetailView.invalidate(logFeed)
    agentNow = Date.now() / 1000
    Qt.callLater(refreshAgentLog)
  }

  function agentElapsed() {
    const seconds = agentActive && agent.started_unix > 0
      ? Math.max(0, Math.floor(agentNow - agent.started_unix)) : 0
    return Math.floor(seconds / 60) + ":" + (seconds % 60 < 10 ? "0" : "")
      + (seconds % 60)
  }

  function open(payloadJson) {
    closingFromHost = false
    liveTab = busy
    window.visible = true
    keyHandler.forceActiveFocus()
    refresh()
  }

  function close() {
    closingFromHost = true
    window.visible = false
    closingFromHost = false
  }

  function api(method, path, body, done, scopedErrors) {
    const xhr = new XMLHttpRequest()
    xhr.open(method, apiBase + path)
    xhr.setRequestHeader("Content-Type", "application/json")
    xhr.onreadystatechange = function() {
      if (xhr.readyState !== XMLHttpRequest.DONE) return
      if (xhr.status === 0) {
        if (!scopedErrors) root.engineOnline = false
        if (done) done(null, xhr.status)
        return
      }
      if (!scopedErrors) root.engineOnline = true
      let parsed = null
      try { parsed = JSON.parse(xhr.responseText) } catch (e) {}
      if (parsed && parsed.error && !scopedErrors) root.localError = parsed.error
      if (done) done(parsed, xhr.status)
    }
    xhr.send(body ? JSON.stringify(body) : null)
  }

  function refresh(done) {
    const serial = ++stateRequestSerial
    api("GET", "/api/state", null, function(resp) {
      // A poll started before a save must not overwrite its refreshed snapshot.
      if (resp && serial > root.stateResponseSerial) {
        root.stateResponseSerial = serial
        // Preserve view models across unchanged polls; replacing ListView and
        // Repeater models can rebuild delegates and reset the reading position.
        if (root.engineState && resp.project === root.engineState.project) {
          for (const key of ["plan", "architecture", "chat", "discussion", "reports", "queue", "model_catalogue"]) {
            if (JSON.stringify(resp[key]) === JSON.stringify(root.engineState[key]))
              resp[key] = root.engineState[key]
          }
        }
        root.engineState = resp
      }
      if (done) done()
    })
  }

  function refreshAgentLog() {
    if (!window.visible) return
    const request = DetailView.begin(logFeed, lastProject, projectViewRevision, agentSession)
    if (!request) return
    let path = "/api/agent_records?cursor=" + request.cursor
      + "&project=" + encodeURIComponent(request.project)
    if (request.session) path += "&session=" + encodeURIComponent(request.session)
    // Handle errors here only after validating the request's view identity.
    api("GET", path, null, function(resp) {
      const ownsRequest = root.logFeed.pending === request
      const page = DetailView.finish(root.logFeed, request, root.lastProject,
        root.projectViewRevision, root.agentSession, resp)
      if (!page) {
        if (ownsRequest && root.logFeed.pending === null)
          root.logError = resp && resp.error ? resp.error : "Could not load agent output"
        return
      }
      root.logError = ""
      agentOutput.liveOutput.beginUpdate()
      if (page.reset) {
        liveEntries.clear()
        agentOutput.liveOutput.resetView()
      }
      DetailView.reconcile(liveEntries, page.entries, request.project, root.logFeed.session)
      agentOutput.liveOutput.endUpdate()
      if (page.more) Qt.callLater(root.refreshAgentLog)
    }, true)
  }

  function act(path, body, done) {
    localError = ""
    api("POST", path, body || {}, function(resp, status) {
      root.refresh(function() { if (done) done(resp, status) })
    })
  }

  function syncGoalEnhancement() {
    const outcome = GoalEnhancement.goalEnhancementAction(engineState ? engineState.goal_enhancement : null,
      goalEnhanceRequest, goalField.text, goalEnhanceSent)
    if (outcome.action === "none") return
    // Consume terminal results once; later polls must preserve Apply and Undo.
    goalEnhanceRequest = -1
    if (outcome.action === "apply") {
      goalEnhanceUndo = goalField.text
      goalField.text = outcome.text
      goalEnhanceReady = ""
      goalEnhanceError = ""
    } else if (outcome.action === "offer") {
      goalEnhanceReady = outcome.text
    } else if (outcome.action === "error") {
      goalEnhanceError = outcome.error
    }
    goalEnhancePending = false
  }

  function enhanceGoal() {
    if (!enhanceGoalButton.enabled) return
    const text = goalField.text
    const revision = projectViewRevision
    goalEnhanceRequest = -1
    goalEnhancePending = true
    goalEnhanceSent = text
    goalEnhanceReady = ""
    goalEnhanceError = ""
    act("/api/goal/enhance", { goal: text }, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (status === 200) {
        root.goalEnhanceRequest = resp.request_id
        // act refreshes before its callback, so the result may already be ready.
        root.syncGoalEnhancement()
      } else {
        root.goalEnhancePending = false
        if (resp && resp.error) root.goalEnhanceError = resp.error
        else root.localError = "Could not enhance the description. Check the engine connection and try again."
      }
    })
  }

  function applyGoalEnhancement() {
    goalEnhanceUndo = goalField.text
    goalField.text = goalEnhanceReady
    goalEnhanceReady = ""
  }

  function undoGoalEnhancement() {
    goalField.text = goalEnhanceUndo
    goalEnhanceUndo = ""
  }

  function revisePlan() {
    if (!improvePlanButton.enabled) return
    const feedback = feedbackField.text
    const revision = projectViewRevision
    revisePending = true
    act("/api/plan/revise", { feedback: feedback }, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      root.revisePending = false
      if (status === 200) {
        if (feedbackField.text === feedback) feedbackField.text = ""
      } else if (!resp || !resp.error) {
        root.localError = "Could not revise plan. Check the engine connection and try again."
      }
    })
  }

  function askPlanQuestion() {
    if (!askPlanButton.enabled) return
    const question = questionField.text
    const revision = projectViewRevision
    chatPending = true
    chatExpanded = true
    chatList.followTail = true
    act("/api/plan/chat", { question: question }, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      // act refreshes state first; keep pending until the answer finishes.
      root.chatPending = status === 200 && root.busy
      if (status === 200) {
        if (questionField.text === question) questionField.text = ""
      } else if (!resp || !resp.error) {
        root.localError = "Could not ask about the plan. Check the engine connection and try again."
      }
    })
  }

  function sendDiscussionMessage(text) {
    if (!discussionCanSend || text.trim() === "") return
    const revision = projectViewRevision
    discussionPending = true
    discussionSent = text
    discussionError = ""
    act("/api/discussion/message", { message: text }, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (status === 200) {
        root.discussionRequest = resp.request_id
        if (discussionView.input.text === text) discussionView.input.text = ""
        // act refreshes state first; the reply may already be ready.
        root.syncDiscussion()
      } else {
        root.discussionPending = false
        root.discussionError = resp && resp.error ? resp.error
          : "Could not send the message. Check the engine connection and try again."
      }
    })
  }

  function syncDiscussion() {
    const outcome = Discussion.discussionAction(engineState ? engineState.discussion_activity : null, discussionRequest)
    if (outcome.action === "none") return
    // Consume terminal results once; later polls must not repeat them.
    discussionRequest = -1
    discussionPending = false
    if (outcome.action === "error") {
      discussionError = outcome.error
      if (discussionView.input.text === "") discussionView.input.text = outcome.message
    }
  }

  function planFromDiscussion() {
    if (!discussionCanPlan) return
    act("/api/plan", { discussion: true, goal: goalField.text })
  }

  function clearDiscussion() {
    if (!discussionCanClear) return
    act("/api/discussion/reset")
  }

  function openDiscussion() {
    if (!discussionOpen) panelStack.push(discussionView, StackView.Immediate)
    keyHandler.pendingKey = ""
    discussionView.input.forceActiveFocus()
  }

  function closeDiscussion() {
    if (discussionOpen) panelStack.pop(panelPage, StackView.Immediate)
    keyHandler.pendingKey = ""
    keyHandler.forceActiveFocus()
  }

  function beginPlanEdit() {
    if (!editPlanButton.enabled) return
    editStages = PlanEdit.cloneStages(plan)
    editGoal = plan.goal || ""
    editProject = lastProject
    editSession++
    editingPlan = true
    localError = ""
    keyHandler.forceActiveFocus()
    keyHandler.selectStage(selectedStageIndex < 0 ? 0 : selectedStageIndex)
  }

  function cancelPlanEdit() {
    if (editingPlan) keyHandler.forceActiveFocus()
    editingPlan = false
    editPending = false
    editFocusedField = null
    editStages = []
    editGoal = ""
    editProject = ""
    editSession++
    selectedStageIndex = Math.min(selectedStageIndex, displayedStages.length - 1)
  }

  function changeStageField(index, field, value) {
    if (!editingPlan || editPending || !editStages[index]
        || editStages[index].status === "committed") return
    editStages[index][field] = value
    editRevision++
  }

  function moveEditStage(index, direction) {
    if (editPending || editStages[index].status === "committed") return
    const target = PlanEdit.editableNeighbor(editStages, index, direction)
    if (target < 0) return
    keyHandler.forceActiveFocus()
    editStages = PlanEdit.moveStages(editStages, index, target)
    keyHandler.selectStage(target)
  }

  function deleteEditStage(index) {
    if (editPending || editStages[index].status === "committed") return
    keyHandler.forceActiveFocus()
    const stages = PlanEdit.deleteStage(editStages, index)
    editStages = stages
    selectedStageIndex = -1
    keyHandler.selectStage(Math.min(index, stages.length - 1))
  }

  function addEditStage() {
    if (editPending) return
    keyHandler.forceActiveFocus()
    editStages = PlanEdit.addStage(editStages)
    keyHandler.selectStage(editStages.length - 1)
  }

  function savePlanEdit() {
    if (!planEditor.saveButton.enabled) return
    keyHandler.forceActiveFocus()
    const session = editSession
    const content = PlanEdit.payload(editGoal, editStages)
    editPending = true
    act("/api/plan/edit", { project: editProject, plan: content }, function(resp) {
      if (session !== root.editSession) return
      root.editPending = false
      if (resp && resp.ok) root.cancelPlanEdit()
      else if (!resp || !resp.error) root.localError = "Could not save plan. Check the engine connection and try again."
    })
  }

  function openChooser() {
    manualEntry = false
    api("GET", "/api/projects", null, function(resp) {
      if (!resp) return
      root.projectsData = resp
      root.chooserOpen = true
    })
  }

  function openDiff() {
    diffText = ""
    diffError = ""
    diffOpen = true
    diffView.listView.positionViewAtBeginning()
    refreshDiff()
  }

  function refreshDiff() {
    if (diffPending) return
    diffPending = true
    const revision = projectViewRevision
    api("GET", "/api/diff?project=" + encodeURIComponent(lastProject), null, function(resp) {
      if (revision !== root.projectViewRevision) return
      root.diffPending = false
      if (resp && typeof resp.diff === "string") {
        root.diffError = ""
        root.diffText = resp.diff
      } else {
        root.diffError = "Unable to load diff"
      }
    })
  }

  property var reportIdentityCache: ({nextId: 0, byRecord: new Map()})
  readonly property var reportIndex: PanelDetails.indexReports(reports, reportIdentityCache)

  function reportKey(report, index) {
    return reportIndex.keys[index] || ""
  }

  function selectedReportIndex() {
    const index = reportIndex.positions[selectedReportKey]
    return index === undefined ? -1 : index
  }

  property var reviewViews: ({})
  property int reviewViewVersion: 0
  property var stageReviewBlocks: ({})
  readonly property string stageReviewBlockScope: stageDetailScope({id: null})
  onStageReviewBlockScopeChanged: stageReviewBlocks = ({})

  function revealDetail(control) {
    // Reveal inside every enclosing scroller, from the inner viewport outward.
    for (let item = control.parent; item; item = item.parent) {
      if (item.contentY === undefined || !item.contentItem || !item.height) continue
      const top = control.mapToItem(item.contentItem, 0, 0).y
      const bottom = top + control.height
      if (top < item.contentY) item.contentY = Math.max(item.originY || 0, top)
      else if (bottom > item.contentY + item.height)
        item.contentY = Math.max(item.originY || 0, Math.min(top, bottom - item.height))
      if (item.followTail !== undefined) item.followTail = false
      if (typeof item.captureReading === "function") item.captureReading()
      else if (item.readingY !== undefined) item.readingY = item.contentY
    }
  }

  function inspectDetail(control) {
    for (let item = control.parent; item; item = item.parent) {
      if (item.followTail !== undefined) item.followTail = false
      if (typeof item.captureReading === "function") item.captureReading()
      else if (item.readingY !== undefined) item.readingY = item.contentY
    }
  }

  component PanelFields: DetailFields {
    id: panelFields
    scope: JSON.stringify([root.lastProject, root.projectViewRevision, (root.plan || {}).plan_id || ""])
    foreground: root.mutedForeground
    mutedForeground: root.mutedForeground
    background: root.background
    urgent: root.urgent
    fontFamily: root.fontFamily
    fontSize: root.fs(11)
    onCopyRequested: original => Quickshell.clipboardText = original
    onLeaveRequested: keyHandler.forceActiveFocus()
    onFocusRevealed: control => root.revealDetail(control)
    onInspecting: root.inspectDetail(panelFields)
  }

  component PanelDetail: PanelFields {
    property string originalText: ""
    property string metadata: ""
    property bool error: false
    entries: [PanelDetails.field("text", metadata, originalText, error)]
  }

  function reviewScope(stage) {
    return ReviewView.scope(lastProject, projectViewRevision, plan, stage)
  }
  function stageDetailScope(stage) {
    // Execution publications change review snapshots without revising prose.
    // Hold open prose through polling; a new plan revision refreshes it.
    const p = plan || {}
    return JSON.stringify([lastProject, projectViewRevision, p.plan_id || "",
      p.revision === undefined ? null : p.revision, stage.id])
  }
  function captureStageSnapshot(stage) {
    stageSnapshot = {key: stageDetailScope(stage), commit: stage.commit || "",
      instructions: stage.instructions || "", acceptance: stage.acceptance || "",
      policyRationale: (stage.review_policy || {}).rationale || "",
      rationale: ModelRouting.stageModelRationale(stage), diagnostics: ModelRouting.stageModelDiagnostics(stage)}
  }
  function syncStageSnapshot() {
    const stage = (displayedStages || []).find(s => s.id === expandedStageId)
    if (!stage) { stageSnapshot = null; return }
    if (!stageSnapshot || stageSnapshot.key !== stageDetailScope(stage)) captureStageSnapshot(stage)
  }
  function reviewView(stage) {
    const version = reviewViewVersion
    const scope = reviewScope(stage)
    // Retain even already-complete state records, so unchanged polls do not
    // replace their delegates and interrupt selection before any network load.
    if (!reviewViews[scope.key]) reviewViews[scope.key] = ReviewView.create(scope)
    return Object.assign({}, reviewViews[scope.key])
  }
  function syncReviewViews() {
    const kept = {}
    const stages = plan && plan.stages ? plan.stages : []
    stages.forEach(function(stage) {
      const scope = root.reviewScope(stage)
      if (root.reviewViews[scope.key]) kept[scope.key] = root.reviewViews[scope.key]
    })
    reviewViews = kept
  }
  function loadStageReviews(stageId, cursor, end, chain) {
    const stage = plan && plan.stages ? plan.stages.find(function(s) { return s.id === stageId }) : null
    if (!stage || !lastProject) return
    const scope = reviewScope(stage)
    chain = chain || {detailScope: stageDetailScope(stage), automatic: false}
    const view = reviewViews[scope.key] || ReviewView.create(scope)
    reviewViews[scope.key] = view
    const request = ReviewView.begin(view, cursor, end)
    if (!request) return
    reviewViewVersion++
    api("GET", ReviewView.path(view, request), null, function(resp, status) {
      const current = root.plan && root.plan.stages ? root.plan.stages.find(function(s) { return s.id === stageId }) : null
      if (!current || root.reviewViews[scope.key] !== view) return
      const result = ReviewView.finish(view, request, root.reviewScope(current), resp, status)
      if (result === "stale") return
      // Observe terminal outcomes before bindings or changed-triggered refresh
      // can replace this verified view. Manual retries/older pages also latch:
      // their failure must not restart automatic completion on the next poll.
      if (chain.detailScope === root.stageDetailScope(current)) {
        if (result === "failed" || result === "changed") root.setStageReviewBlock(current, view.error)
        else if (result === "complete") root.setStageReviewBlock(current, "")
      }
      root.reviewViewVersion++
      if (result === "more") root.loadStageReviews(stageId, view.retry.cursor, view.retry.end, chain)
      if (result === "changed") root.refresh()
    }, true)
  }

  function setStageReviewBlock(stage, message) {
    // Prune scopes retired by a project/visit/plan/revision change. Execution
    // snapshots and checkpoints deliberately do not participate in this key.
    const next = {}
    const stages = plan && plan.stages ? plan.stages : []
    stages.forEach(function(s) {
      const key = root.stageDetailScope(s)
      if (root.stageReviewBlocks[key]) next[key] = root.stageReviewBlocks[key]
    })
    const key = stageDetailScope(stage)
    if (message) next[key] = message
    else delete next[key]
    stageReviewBlocks = next
  }
  function stageReviewIncompleteRange(view) {
    const rows = view.rows.filter(function(row) { return row.complete === false })
    return rows.length ? {cursor: rows[0].position, end: rows[rows.length - 1].position + 1} : null
  }
  function ensureStageReviewsLoaded(stage) {
    // This is also called by deferred checks: resolve the current stage instead
    // of trusting a captured publication or a delegate that has been destroyed.
    const current = plan && plan.stages ? plan.stages.find(function(s) { return s.id === stage.id }) : null
    if (!lastProject || !current || expandedStageId !== current.id || editingPlan) return
    const view = reviewView(current)
    const range = stageReviewIncompleteRange(view)
    if (!range || view.pending || view.error !== "" || stageReviewBlocks[stageDetailScope(current)]) return
    loadStageReviews(current.id, range.cursor, range.end,
      {detailScope: stageDetailScope(current), automatic: true})
  }
  function retryStageReviews(stage) {
    const current = plan && plan.stages ? plan.stages.find(function(s) { return s.id === stage.id }) : null
    if (!current || !lastProject || expandedStageId !== current.id) return
    const view = reviewView(current)
    if (view.pending) return
    // Keep older-page gaps recoverable even when every displayed row is full.
    const range = view.retry || stageReviewIncompleteRange(view)
      || (view.older > 0 ? {cursor: Math.max(0, view.older - 8), end: view.older} : null)
    setStageReviewBlock(current, "")
    if (range) loadStageReviews(current.id, range.cursor, range.end)
  }
  function stageReviewHasSelection(item) {
    if (!item) return false
    if (item.stageProseField === true && item.hasSelection) return true
    return Array.from(item.children || []).some(function(child) { return root.stageReviewHasSelection(child) })
  }
  function stageReviewHasHeldPreview(presentation, view) {
    if (stageReviewIncompleteRange(view)) return false
    for (let i = 0; i < presentation.count; i++) {
      if (!presentation.get(i).record.complete) return true
    }
    return false
  }
  function reconcileStageReviewPresentation(stage, presentation, repeater) {
    // Presentation is separate from ReviewView's current verification authority.
    // A selected original stays in its existing editor, with its source scope;
    // it never makes a new scope's preview complete or suppresses verification.
    const view = reviewView(stage), scope = view.scope.key, detailScope = stageDetailScope(stage)
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

  function stageActivity(stage) {
    if (!engineState || !busy || stage.status !== "in_progress"
        || stage.id !== engineState.current_stage) return ""
    const step = engineState.current_step || ""
    if (!step) return ""
    const activity = step.indexOf("fixing") === 0 ? "fixing for review" : step
    return "now: " + activity + " · " + reviewRoundLabel({ round: stage.rounds })
  }

  function chooserRows(data, filter) {
    if (!data) return []
    const f = filter.toLowerCase()
    const rows = []
    const local = (data.local || []).filter(function(p) {
      return f === "" || p.name.toLowerCase().indexOf(f) !== -1
    })
    if (local.length > 0) rows.push({ kind: "header", label: "Local" })
    local.forEach(function(p) { rows.push({ kind: "local", name: p.name, path: p.path }) })
    const remote = (data.remote || []).filter(function(r) {
      return f === "" || r.full_name.toLowerCase().indexOf(f) !== -1
    })
    if (remote.length > 0 || data.remote_error) rows.push({ kind: "header", label: "GitHub" })
    if (data.remote_error) rows.push({ kind: "note", label: data.remote_error })
    remote.forEach(function(r) {
      rows.push({ kind: "remote", name: r.full_name, cloned: r.cloned,
                  isPrivate: r.private })
    })
    rows.push({ kind: "path", label: "path…" })
    return rows
  }

  function chooseRow(row) {
    if (row.kind === "local") {
      act("/api/project/select", { path: row.path })
      chooserOpen = false
    } else if (row.kind === "remote") {
      act("/api/project/select", { repo: row.name })
      chooserOpen = false
    } else if (row.kind === "path") {
      manualEntry = !manualEntry
      if (manualEntry) projectChooser.manualField.forceActiveFocus()
      else keyHandler.forceActiveFocus()
    }
  }

  function reviewerLabel() {
    const s = engineState ? engineState.settings : {}
    return s.reviewer_provider_mode === "configured" || s.reviewer_model || s.automatic_routing === false
      ? "reviewer: " + (s.reviewer || "…")
      : "reviewer: auto (other provider)"
  }

  function cycleReviewer() {
    const s = engineState ? engineState.settings : {}
    if (s.reviewer_provider_mode !== "configured")
      act("/api/settings", {reviewer:"codex", reviewer_provider_mode:"configured", reviewer_model:""})
    else if (s.reviewer === "codex")
      act("/api/settings", {reviewer:"claude", reviewer_provider_mode:"configured", reviewer_model:""})
    else
      act("/api/settings", {reviewer_provider_mode:"other_provider", automatic_routing:true, reviewer_model:""})
  }

  function cycleTool(key) {
    const current = engineState ? engineState.settings[key] : "claude"
    const next = current === "claude" ? "codex" : "claude"
    const patch = {}
    patch[key] = next
    act("/api/settings", patch)
  }

  Timer {
    interval: root.busy ? 1000 : 2000
    repeat: true
    running: window.visible
    onTriggered: root.refresh()
  }

  Timer {
    interval: 3000
    repeat: true
    running: window.visible && root.diffOpen && root.busy
    onTriggered: root.refreshDiff()
  }

  Timer {
    interval: 1000
    repeat: true
    // Poll through idle as well: the terminal page can arrive after busy clears.
    running: window.visible
    triggeredOnStart: true
    onTriggered: root.refreshAgentLog()
  }

  Timer {
    interval: 1000
    repeat: true
    running: window.visible
    triggeredOnStart: true
    onTriggered: root.agentNow = Date.now() / 1000
  }

  FloatingWindow {
    id: window
    visible: false
    title: "Forge"
    color: root.background
    implicitWidth: 760
    implicitHeight: 760
    minimumSize: Qt.size(560, 680)

    onVisibleChanged: {
      if (visible) keyHandler.forceActiveFocus()
      if (!visible && !root.closingFromHost && root.shell
          && typeof root.shell.hide === "function")
        root.shell.hide(root.pluginId)
    }

    Rectangle {
      anchors.fill: parent
      color: root.background

      Item {
        id: keyHandler
        anchors.fill: parent
        focus: true
        property string pendingKey: ""
        onActiveFocusChanged: pendingKey = ""

        function selectStage(index) {
          const stages = root.displayedStages
          if (stages.length === 0) return
          root.selectedStageIndex = Math.max(0, Math.min(index, stages.length - 1))
          if (root.editingPlan) {
            planEditor.stageList.positionViewAtIndex(root.selectedStageIndex, ListView.Contain)
            panelScroll.reveal(planEditor.stageFrame)
          } else {
            planEditor.stageList.forceLayout()
            const row = planEditor.stageList.itemAtIndex(root.selectedStageIndex)
            if (row) panelScroll.reveal(row)
          }
        }

        function selectReport(index) {
          if (root.reports.length === 0) return
          const selected = Math.max(0, Math.min(index, root.reports.length - 1))
          root.selectedReportKey = root.reportKey(root.reports[selected], selected)
          agentOutput.reportList.positionViewAtIndex(selected, ListView.Contain)
          agentOutput.reportList.captureReading()
          panelScroll.reveal(agentOutput.outputFrame)
        }

        function scrollOutput(direction) {
          const view = root.liveTab ? agentOutput.liveOutput : root.reportsVisible ? agentOutput.reportList : agentOutput.historyList
          view.cancelFlick()
          if (view !== agentOutput.reportList) view.followTail = false
          const top = view.originY
          const bottom = top + Math.max(0, view.contentHeight - view.height)
          view.contentY = Math.max(top, Math.min(bottom,
            view.contentY + direction * view.height / 2))
          if (view !== agentOutput.reportList && direction > 0 && view.contentY >= bottom) {
            view.followTail = true
            view.scrollToTail()
          }
          if (view !== agentOutput.reportList) view.captureReading()
          if (view === agentOutput.reportList) agentOutput.reportList.captureReading()
          panelScroll.reveal(agentOutput.outputFrame)
        }

        function scrollDiff(amount) {
          diffView.listView.cancelFlick()
          const top = diffView.listView.originY
          const bottom = top + Math.max(0, diffView.listView.contentHeight - diffView.listView.height)
          diffView.listView.contentY = Math.max(top, Math.min(bottom, diffView.listView.contentY + amount))
        }

        Keys.onPressed: event => {
          const prefix = pendingKey
          pendingKey = ""
          const question = (event.key === Qt.Key_Question
            && (event.modifiers === Qt.NoModifier || event.modifiers === Qt.ShiftModifier))
            || (event.key === Qt.Key_Slash && event.modifiers === Qt.ShiftModifier)
            || (event.key === Qt.Key_F1 && event.modifiers === Qt.NoModifier)
          // Modal normal mode owns every key; focused text fields handle insert mode.
          if (root.catalogueOpen) {
            event.accepted = true
            if (event.key === Qt.Key_Escape) root.catalogueOpen = false
            return
          }
          if (root.helpOpen) {
            event.accepted = true
            if (question || event.key === Qt.Key_Escape
                || (event.key === Qt.Key_Q && event.modifiers === Qt.NoModifier))
              root.helpOpen = false
          } else if (root.diffOpen) {
            event.accepted = true
            if (event.key === Qt.Key_Escape) {
              root.diffOpen = false
            } else if (event.modifiers === Qt.ShiftModifier) {
              if (event.key === Qt.Key_G) {
                diffView.listView.cancelFlick()
                diffView.listView.positionViewAtEnd()
              } else if (event.key === Qt.Key_R && !root.diffPending) {
                root.refreshDiff()
              }
            } else if (event.modifiers === Qt.ControlModifier) {
              if (event.key === Qt.Key_D || event.key === Qt.Key_U)
                scrollDiff((event.key === Qt.Key_D ? 1 : -1) * diffView.listView.height / 2)
            } else if (event.modifiers === Qt.NoModifier) {
              if (event.key === Qt.Key_Q) root.diffOpen = false
              else if (event.key === Qt.Key_J || event.key === Qt.Key_K)
                scrollDiff(event.key === Qt.Key_J ? 40 : -40)
              else if (event.key === Qt.Key_G) {
                if (prefix === "g") {
                  diffView.listView.cancelFlick()
                  diffView.listView.positionViewAtBeginning()
                } else pendingKey = "g"
              }
            }
          } else if (root.chooserOpen) {
            event.accepted = true
            if (event.key === Qt.Key_Escape) {
              root.chooserOpen = false
            } else if (event.modifiers === Qt.NoModifier) {
              if (event.key === Qt.Key_Q) root.chooserOpen = false
              else if (event.key === Qt.Key_J || event.key === Qt.Key_K)
                projectChooser.chooserList.moveSelection(event.key === Qt.Key_J ? 1 : -1)
              else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter)
                projectChooser.chooserList.activateSelection()
              else if (event.key === Qt.Key_Slash || event.key === Qt.Key_I)
                projectChooser.filterField.forceActiveFocus()
            }
          } else if (question) {
            root.helpOpen = true
            event.accepted = true
          } else if (event.key === Qt.Key_I && event.modifiers === Qt.NoModifier) {
            goalField.forceActiveFocus()
            panelScroll.reveal(goalFlick.parent)
            event.accepted = true
          } else if (event.key === Qt.Key_I && event.modifiers === Qt.ShiftModifier) {
            feedbackField.forceActiveFocus()
            panelScroll.reveal(feedbackField.parent)
            event.accepted = true
          } else if (event.key === Qt.Key_E && event.modifiers === Qt.ShiftModifier) {
            root.enhanceGoal()
            event.accepted = true
          } else if (event.key === Qt.Key_T && event.modifiers === Qt.NoModifier) {
            root.openDiscussion()
            event.accepted = true
          } else if (event.key === Qt.Key_Escape) {
            if (root.editingPlan && !root.editPending) root.cancelPlanEdit()
            event.accepted = true
          } else {
            const stages = root.displayedStages
            if (event.key === Qt.Key_G && event.modifiers === Qt.ShiftModifier) {
              if (root.reportsVisible) selectReport(root.reports.length - 1)
              else selectStage(stages.length - 1)
              event.accepted = true
            } else if (event.key === Qt.Key_P && event.modifiers === Qt.ShiftModifier) {
              if (root.discussionCanPlan) root.planFromDiscussion()
              event.accepted = true
            } else if (event.modifiers === Qt.ControlModifier
                       && (event.key === Qt.Key_D || event.key === Qt.Key_U)) {
              scrollOutput(event.key === Qt.Key_D ? 1 : -1)
              event.accepted = true
            } else if (event.modifiers === Qt.NoModifier) {
              // Keep action guards identical to their PanelButton.enabled bindings.
              if (event.key === Qt.Key_PageDown || event.key === Qt.Key_PageUp) {
                panelScroll.scrollPage(event.key === Qt.Key_PageDown ? 1 : -1)
                event.accepted = true
              } else if (event.key === Qt.Key_Home || event.key === Qt.Key_End) {
                panelScroll.cancelFlick()
                panelScroll.contentY = event.key === Qt.Key_Home ? 0 : panelScroll.maximumY
                event.accepted = true
              } else if (event.key === Qt.Key_P) {
                if (createPlanButton.enabled)
                  root.act("/api/plan", { goal: goalField.text })
                event.accepted = true
              } else if (event.key === Qt.Key_A) {
                if (approvePlanButton.enabled)
                  root.act("/api/approve")
                event.accepted = true
              } else if (event.key === Qt.Key_R) {
                if (runPlanButton.enabled)
                  root.act("/api/run")
                event.accepted = true
              } else if (event.key === Qt.Key_E) {
                if (editPlanButton.enabled) root.beginPlanEdit()
                event.accepted = true
              } else if (event.key === Qt.Key_X) {
                if (root.phase === "running" || root.queueActive)
                  root.act("/api/stop")
                event.accepted = true
              } else if (event.key === Qt.Key_D) {
                if (root.engineOnline) root.openDiff()
                event.accepted = true
              } else if (event.key === Qt.Key_C) {
                if (root.engineOnline) root.openChooser()
                event.accepted = true
              } else if (event.key === Qt.Key_Tab) {
                root.liveTab = !root.liveTab
                event.accepted = true
              } else if (event.key === Qt.Key_H || event.key === Qt.Key_L) {
                root.liveTab = event.key === Qt.Key_H
                event.accepted = true
              } else if (event.key >= Qt.Key_1 && event.key <= Qt.Key_6) {
                root.liveTab = false
                if (event.key !== Qt.Key_6 || root.hasReports)
                  root.historyFilter = ["all", "runs", "git", "reviews", "errors", "reports"][event.key - Qt.Key_1]
                event.accepted = true
              } else if (event.key === Qt.Key_J || event.key === Qt.Key_K) {
                if (root.reportsVisible) {
                  const selected = root.selectedReportIndex()
                  selectReport(selected < 0 ? 0 : selected + (event.key === Qt.Key_J ? 1 : -1))
                } else selectStage(root.selectedStageIndex < 0 ? 0
                    : root.selectedStageIndex + (event.key === Qt.Key_J ? 1 : -1))
                event.accepted = true
              } else if (event.key === Qt.Key_G) {
                if (prefix === "g") {
                  if (root.reportsVisible) selectReport(0)
                  else selectStage(0)
                }
                else pendingKey = "g"
                event.accepted = true
              } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter
                         || event.key === Qt.Key_O || event.key === Qt.Key_Space) {
                if (root.reportsVisible) {
                  if (root.selectedReportIndex() < 0) selectReport(0)
                  root.expandedReportKey = root.expandedReportKey === root.selectedReportKey
                    ? "" : root.selectedReportKey
                } else if (root.selectedStageIndex >= 0 && root.selectedStageIndex < stages.length) {
                  const row = planEditor.stageList.itemAtIndex(root.selectedStageIndex)
                  if (root.editingPlan && stages[root.selectedStageIndex].status !== "committed") {
                    if (row) row.focusEditor()
                  } else {
                    const stageId = stages[root.selectedStageIndex].id
                    root.expandedStageId = root.expandedStageId === stageId ? -1 : stageId
                  }
                }
                event.accepted = true
              }
            }
          }
        }

        StackView {
          id: panelStack

          anchors.fill: parent
          initialItem: panelPage
          // Instant page swaps: the panel is a navigation stack, not an animated app.
          pushEnter: Transition {}
          pushExit: Transition {}
          popEnter: Transition {}
          popExit: Transition {}
          replaceEnter: Transition {}
          replaceExit: Transition {}
        }

        Item {
          id: panelPage

          Flickable {
            id: panelScroll
            anchors.fill: parent
            anchors.margins: Style.space(16)
            anchors.bottomMargin: Style.space(16) + keyboardHint.height + Style.space(10)
            clip: true
            contentWidth: width
            contentHeight: panelColumn.implicitHeight
            flickableDirection: Flickable.VerticalFlick
            boundsBehavior: Flickable.StopAtBounds
            readonly property real maximumY: Math.max(0, contentHeight - height)
            function scrollPage(direction) {
              cancelFlick()
              contentY = Math.max(0, Math.min(maximumY, contentY + direction * height * 0.8))
            }
            function reveal(item) {
              const top = item.mapToItem(contentItem, 0, 0).y
              if (top < contentY || top + item.height > contentY + height)
                contentY = Math.max(0, Math.min(maximumY, top))
            }
            ScrollBar.vertical: ScrollBar {
              policy: ScrollBar.AsNeeded
            }

          Column {
            id: panelColumn
            width: panelScroll.width - Style.space(16)
            spacing: Style.space(10)

            // ---------------------------------------------------- header
            Row {
              width: parent.width
              spacing: Style.space(10)

              Text {
                id: forgeTitle
                text: "FORGE"
                color: root.accent
                font.family: root.fontFamily
                font.pixelSize: root.fs(18)
                font.bold: true
              }
              Text {
                id: projectActivity
                visible: root.backgroundBusy
                text: root.activeProjectCount + " project"
                  + (root.activeProjectCount === 1 ? "" : "s") + " active"
                color: root.mutedForeground
                font.family: root.fontFamily
                font.pixelSize: root.fs(10)
                anchors.verticalCenter: parent.verticalCenter
              }
              Rectangle {
                id: phaseBadge
                width: phaseText.implicitWidth + Style.space(16)
                height: phaseText.implicitHeight + Style.space(6)
                radius: height / 2
                color: "transparent"
                border.width: 1
                border.color: root.phase === "failed" || root.phase === "blocked"
                  ? root.urgent
                  : root.busy ? root.working
                  : root.phase === "done" ? root.success
                  : root.mutedForeground
                Text {
                  id: phaseText
                  anchors.centerIn: parent
                  text: root.engineOnline ? root.phase : "engine offline"
                  color: root.phase === "failed" || root.phase === "blocked"
                    ? root.urgent
                    : root.busy ? root.working
                    : root.phase === "done" ? root.success
                    : root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(11)
                }
              }
              Text {
                visible: root.engineState !== null && root.engineState.current_step !== ""
                width: Math.max(0, parent.width - forgeTitle.width - phaseBadge.width
                  - (projectActivity.visible ? projectActivity.width + parent.spacing : 0)
                  - helpButton.width - parent.spacing
                  - 2 * parent.spacing)
                elide: Text.ElideRight
                text: root.engineState && root.engineState.current_stage !== null
                  ? "stage " + root.engineState.current_stage + ": "
                    + (root.currentActivity || root.engineState.current_step)
                  : (root.engineState ? root.engineState.current_step : "")
                color: root.mutedForeground
                font.family: root.fontFamily
                font.pixelSize: root.fs(12)
                anchors.verticalCenter: parent.verticalCenter
              }
              PanelButton {
                id: helpButton
                label: "? Help"
                onClicked: root.helpOpen = true
              }
            }

            // ------------------------------------------------ now working
            Rectangle {
              id: agentCard
              readonly property var stages: root.plan && root.plan.stages ? root.plan.stages : []
              readonly property var currentStage: stages.find(function(stage) {
                return root.engineState && stage.id === root.engineState.current_stage
              })
              readonly property int committedStages: stages.filter(function(stage) {
                return stage.status === "committed"
              }).length
              readonly property int runSeconds: root.engineState && root.engineState.run_started_unix > 0
                ? Math.max(0, Math.floor(root.agentNow - root.engineState.run_started_unix)) : 0

              visible: root.busy
              width: parent.width
              height: visible ? workingSummary.implicitHeight + Style.space(16) : 0
              color: root.surface
              radius: 4

              Column {
                id: workingSummary
                anchors.top: parent.top
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.margins: Style.space(8)
                spacing: Style.space(4)
                PanelDetail {
                  width: parent.width
                  originalText: root.engineState ? root.engineState.goal : ""
                  metadata: "Goal"
                }
                Row {
                  width: parent.width
                  spacing: Style.space(8)
                  Text {
                    width: parent.width - (workingStep.visible ? workingStep.width + parent.spacing : 0)
                    text: root.phase === "planning" ? "planning…"
                      : root.engineState && root.engineState.current_stage !== null
                        ? "stage " + root.engineState.current_stage
                          + (agentCard.currentStage ? " · " + agentCard.currentStage.title : "")
                        : "now working"
                    textFormat: Text.PlainText
                    color: root.working
                    elide: Text.ElideRight
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(11)
                  }
                  Text {
                    id: workingStep
                    visible: root.phase !== "planning" && text !== ""
                    width: Math.min(implicitWidth, parent.width * 0.4)
                    text: root.currentActivity || (root.engineState ? root.engineState.current_step : "")
                    textFormat: Text.PlainText
                    color: root.working
                    elide: Text.ElideRight
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(11)
                  }
                }
                Text {
                  readonly property string planUsage: UsageFormat.usageSummary(root.plan ? root.plan.usage : null)
                  visible: root.phase !== "planning"
                    || (root.engineState !== null && root.engineState.run_started_unix > 0)
                  width: parent.width
                  text: (root.phase !== "planning"
                      ? agentCard.committedStages + "/" + agentCard.stages.length + " stages committed" : "")
                    + (root.engineState && root.engineState.run_started_unix > 0
                      ? (root.phase !== "planning" ? " · " : "")
                        + "run " + Math.floor(agentCard.runSeconds / 60) + "m "
                        + (agentCard.runSeconds % 60) + "s" : "")
                    + (planUsage ? " · " + planUsage : "")
                  textFormat: Text.PlainText
                  color: root.mutedForeground
                  wrapMode: planUsage ? Text.Wrap : Text.NoWrap
                  elide: planUsage ? Text.ElideNone : Text.ElideRight
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(11)
                }
                Row {
                  visible: root.agentActive
                  width: parent.width
                  spacing: Style.space(10)
                  Text {
                    id: agentSummary
                    width: Math.max(0, parent.width - agentTime.width - parent.spacing)
                    text: root.agentActive ? root.agent.role + " · " + root.agent.tool
                      + (root.agent.model ? " · " + root.agent.model : "") : ""
                    textFormat: Text.PlainText
                    color: root.working
                    elide: Text.ElideRight
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(11)
                  }
                  Text {
                    id: agentTime
                    text: root.agentActive ? root.agentElapsed() + " · " + root.agent.lines + " lines" : ""
                    textFormat: Text.PlainText
                    color: root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(11)
                  }
                }
                PanelDetail {
                  // The state heartbeat's last_line is bounded to 200 characters.
                  // Full-text actions must use the complete retained feed instead.
                  visible: root.agentActive && liveEntries.count > 0
                  scope: JSON.stringify([root.lastProject, root.projectViewRevision, root.agentSession])
                  width: parent.width
                  originalText: liveEntries.count > 0 ? liveEntries.get(liveEntries.count - 1).originalText : ""
                  metadata: "Latest retained output"
                  error: liveEntries.count > 0 && (liveEntries.get(liveEntries.count - 1).kind === "error"
                    || liveEntries.get(liveEntries.count - 1).stream === "stderr")
                }
              }
            }

            // ----------------------------------------------- project tabs
            Flow {
              id: projectTabs
              visible: root.sessions.length > 1
              width: parent.width
              spacing: Style.space(8)
              Repeater {
                model: root.sessions
                delegate: PanelButton {
                  required property var modelData
                  readonly property bool needsAttention: modelData.phase === "blocked"
                    || modelData.phase === "failed"
                  label: modelData.name + " "
                    + (modelData.busy || modelData.queue_active ? "●" : needsAttention ? "!"
                      : modelData.phase === "done" ? "✓" : "·")
                    + (modelData.queued > 0 ? " +" + modelData.queued : "")
                  width: Math.min(implicitWidth, projectTabs.width)
                  primary: modelData.project === root.activeProject
                  labelColor: !primary && needsAttention ? root.urgent
                    : primary && enabled ? root.background : root.foreground
                  enabled: root.engineOnline
                  onClicked: root.act("/api/project/select", { path: modelData.project })
                }
              }
            }

            // -------------------------------------------- project + tools
            Row {
              width: parent.width
              spacing: Style.space(10)

              Column {
                width: parent.width - changeProjectButton.width - parent.spacing
                Text {
                  text: root.projectName
                  color: root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(13)
                  font.bold: true
                }
                Text {
                  width: parent.width
                  text: root.engineState ? root.engineState.project : ""
                  color: root.mutedForeground
                  elide: Text.ElideMiddle
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(10)
                }
              }
              PanelButton {
                id: changeProjectButton
                label: "Change project"
                enabled: root.engineOnline
                onClicked: root.openChooser()
              }
            }

            Flow {
              width: parent.width
              spacing: Style.space(8)
              PanelButton {
                label: "planner: "
                  + (root.engineState ? root.engineState.settings.planner : "…")
                onClicked: root.cycleTool("planner")
              }
              PanelButton {
                label: "architect: "
                  + (root.engineState ? (root.engineState.settings.architect || "codex") : "…")
                onClicked: root.cycleTool("architect")
              }
              PanelButton {
                label: "automatic routing: " + (root.engineState && root.engineState.settings.automatic_routing !== false ? "yes" : "no")
                onClicked: root.act("/api/settings", { automatic_routing: !(root.engineState && root.engineState.settings.automatic_routing !== false) })
              }
              PanelButton {
                label: "implementer: "
                  + (root.engineState ? root.engineState.settings.implementer : "…")
                onClicked: root.cycleTool("implementer")
              }
              PanelButton {
                label: root.reviewerLabel()
                onClicked: root.cycleReviewer()
              }
              CadenceButton { role: "architect" }
              CadenceButton { role: "reviewer" }
              PanelButton {
                label: "push at end: "
                  + (root.engineState && root.engineState.settings.auto_push ? "yes" : "no")
                onClicked: root.act("/api/settings", {
                  auto_push: !(root.engineState && root.engineState.settings.auto_push) })
              }
              PanelButton {
                label: "auto-approve: "
                  + (root.engineState && root.engineState.settings.queue_auto_approve ? "yes" : "no")
                enabled: root.engineOnline
                onClicked: root.act("/api/settings", {
                  queue_auto_approve: !(root.engineState && root.engineState.settings.queue_auto_approve) })
              }
            }

            Column {
              width: parent.width
              spacing: Style.space(6)
              Flow {
                width: parent.width
                spacing: Style.space(8)
                PanelButton {
                  label: root.catalogue && root.catalogue.refreshing ? "Models: refreshing…" : "Refresh models"
                  enabled: root.engineOnline && !(root.catalogue && root.catalogue.refreshing)
                  onClicked: root.act("/api/models/refresh", {})
                }
                PanelButton {
                  label: "Cancel refresh"
                  visible: !!root.catalogue && root.catalogue.refreshing
                  onClicked: root.act("/api/models/cancel", {})
                }
                PanelButton {
                  label: root.engineState && root.engineState.claude_quota && root.engineState.claude_quota.refreshing
                    ? "Limits: checking…" : "Refresh Claude limits"
                  enabled: root.engineOnline && !(root.engineState && root.engineState.claude_quota && root.engineState.claude_quota.refreshing)
                  onClicked: root.act("/api/quota/refresh", {})
                }
                PanelButton {
                  label: root.catalogueOpen ? "Close model settings" : "Model settings & options"
                  onClicked: { if (root.catalogueOpen) root.catalogueOpen = false; else root.openCatalogue() }
                }
              }
              Text {
                width: parent.width
                text: UsageFormat.quotaSummary(root.engineState && root.engineState.claude_quota
                  ? Object.assign({}, root.engineState.claude_quota, {error: ""}) : null)
                textFormat: Text.PlainText
                color: root.foreground
                wrapMode: Text.Wrap
                font.family: root.fontFamily
                font.pixelSize: root.fs(12)
              }
              PanelDetail {
                width: parent.width
                visible: originalText !== ""
                metadata: "Quota error"
                error: true
                originalText: root.engineState && root.engineState.claude_quota ? root.engineState.claude_quota.error || "" : ""
              }
              Text {
                width: parent.width
                text: root.catalogue ? "Model tiers: configured user policy · revision " + root.catalogue.policy_revision
                  + " · " + root.catalogue.configured_count + " explicit entries" : "Model catalogue pending"
                color: root.mutedForeground
                wrapMode: Text.Wrap
                font.family: root.fontFamily
                font.pixelSize: root.fs(11)
              }
              PanelDetail {
                width: parent.width
                visible: !!root.catalogue && !!root.catalogue.policy_error
                originalText: root.catalogue && root.catalogue.policy_error ? root.catalogue.policy_error : ""
                metadata: "Model policy error"
                error: true
              }
              Text {
                width: parent.width
                visible: !!root.engineState && !!root.engineState.model_selection
                text: visible ? "Last model: " + root.engineState.model_selection.provider + "/"
                  + (root.engineState.model_selection.model || "provider default") + " · "
                  + root.engineState.model_selection.availability : ""
                color: root.mutedForeground
                wrapMode: Text.Wrap
                font.family: root.fontFamily
                font.pixelSize: root.fs(11)
              }
              PanelFields {
                objectName: "providerDetails"
                width: parent.width
                entries: PanelDetails.providers(root.catalogue ? root.catalogue.providers : [])
              }
              Text {
                width: parent.width
                visible: !!root.catalogue
                text: CataloguePresentation.catalogueMetadataSummaryText(root.catalogue && root.catalogue.metadata
                  ? Object.assign({}, root.catalogue.metadata, {store_error: ""}) : null)
                color: root.catalogue && root.catalogue.metadata
                  && (root.catalogue.metadata.source_errors || root.catalogue.metadata.store_error)
                  ? root.urgent : root.mutedForeground
                wrapMode: Text.Wrap
                font.family: root.fontFamily
                font.pixelSize: root.fs(11)
              }

              PanelDetail {
                width: parent.width
                visible: originalText !== ""
                metadata: "Metadata store error"
                error: true
                originalText: root.catalogue && root.catalogue.metadata ? root.catalogue.metadata.store_error || "" : ""
              }
            }

            // ---------------------------------------------------- goal
            Rectangle {
              width: parent.width
              height: Math.min(Math.max(Style.space(52),
                goalField.contentHeight + Style.space(16)), Style.space(140))
              color: root.surface
              radius: 4
              border.width: 1
              border.color: goalField.activeFocus
                ? root.accent : Qt.darker(root.foreground, 3)
              Flickable {
                id: goalFlick
                anchors.fill: parent
                anchors.margins: Style.space(6)
                clip: true
                contentWidth: goalField.width
                contentHeight: goalField.height
                flickableDirection: Flickable.VerticalFlick
                boundsBehavior: Flickable.StopAtBounds

                function ensureCursorVisible() {
                  const cursor = goalField.cursorRectangle
                  if (contentY > cursor.y)
                    contentY = cursor.y
                  else if (contentY + height < cursor.y + cursor.height)
                    contentY = cursor.y + cursor.height - height
                  contentY = Math.max(0, Math.min(contentY, contentHeight - height))
                }

                onHeightChanged: Qt.callLater(ensureCursorVisible)
                onContentHeightChanged: Qt.callLater(ensureCursorVisible)

                TextEdit {
                  id: goalField
                  Keys.onPressed: event => {
                    if (event.key === Qt.Key_F1) {
                      root.helpOpen = true
                      keyHandler.forceActiveFocus()
                      event.accepted = true
                    }
                  }
                  Keys.onEscapePressed: event => {
                    keyHandler.forceActiveFocus()
                    event.accepted = true
                  }
                  width: goalFlick.width
                  height: Math.max(contentHeight, goalFlick.height)
                  wrapMode: TextEdit.Wrap
                  color: root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(12)
                  onCursorRectangleChanged: goalFlick.ensureCursorVisible()
                  Text {
                    visible: goalField.text === "" && !goalField.activeFocus
                    text: "Describe a goal, or leave empty and press Refactor plan for suggestions"
                    color: root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(12)
                  }
                }
              }
            }

            Text {
              width: parent.width
              visible: text !== ""
              text: root.goalEnhancePending ? "enhancing the description…"
                : root.goalEnhanceError !== "" ? root.goalEnhanceError
                : root.goalEnhanceReady !== "" ? "AI rewrite ready — press Apply AI description" : ""
              textFormat: Text.PlainText
              color: root.goalEnhanceError !== "" ? root.urgent : root.mutedForeground
              wrapMode: Text.Wrap
              font.family: root.fontFamily
              font.pixelSize: root.fs(11)
            }

            Flow {
              width: parent.width
              spacing: Style.space(8)
              PanelButton {
                id: createPlanButton
                label: "Create plan"
                primary: true
                enabled: !root.editingPlan && !root.revisePending && !root.busy && goalField.text.trim() !== ""
                onClicked: root.act("/api/plan", { goal: goalField.text })
              }
              PanelButton {
                id: enhanceGoalButton
                label: "Enhance with AI"
                enabled: root.engineOnline && !root.busy && !root.editingPlan && !root.revisePending
                  && !root.goalEnhancePending && goalField.text.trim() !== ""
                onClicked: root.enhanceGoal()
              }
              PanelButton {
                label: "Apply AI description"
                visible: root.goalEnhanceReady !== ""
                onClicked: root.applyGoalEnhancement()
              }
              PanelButton {
                label: "Undo enhance"
                visible: root.goalEnhanceUndo !== ""
                onClicked: root.undoGoalEnhancement()
              }
              PanelButton {
                label: "Refactor plan"
                enabled: !root.editingPlan && !root.revisePending && !root.busy && root.engineOnline
                onClicked: root.act("/api/plan", { mode: "refactor", goal: goalField.text })
              }
              PanelButton {
                label: "Add to queue"
                enabled: root.engineOnline && goalField.text.trim() !== ""
                onClicked: {
                  root.act("/api/queue/add", { goal: goalField.text })
                  goalField.text = ""
                }
              }
              PanelButton {
                id: approvePlanButton
                label: "Plan is OK — approve"
                enabled: !root.editingPlan && !root.revisePending && !root.busy
                  && root.plan !== null && root.plan.status === "draft"
                onClicked: root.act("/api/approve")
              }
              PanelButton {
                id: runPlanButton
                label: "Start implementing"
                primary: true
                enabled: !root.editingPlan && !root.revisePending && !root.busy && root.plan !== null
                  && (root.plan.status === "approved" || root.plan.status === "done")
                onClicked: root.act("/api/run")
              }
              PanelButton {
                label: "Stop"
                enabled: root.phase === "running" || root.queueActive
                onClicked: root.act("/api/stop")
              }
              PanelButton {
                id: editPlanButton
                label: "Edit plan"
                visible: !root.editingPlan
                enabled: !root.editingPlan && !root.revisePending && root.engineOnline && !root.busy && !root.queueActive
                  && root.plan !== null && ["draft", "approved", "done"].indexOf(root.plan.status) !== -1
                onClicked: root.beginPlanEdit()
              }
              PanelButton {
                label: "Discard plan"
                enabled: !root.editingPlan && !root.revisePending && !root.busy && root.plan !== null
                onClicked: root.act("/api/reset_plan")
              }
              PanelButton {
                label: "View diff"
                enabled: root.engineOnline
                onClicked: root.openDiff()
              }
              PanelButton {
                label: "Update Forge"
                enabled: root.engineOnline && !root.busy
                onClicked: root.act("/api/self_update")
              }
            }

            // Entry point for the discussion; the chat itself is a stack page.
            Row {
              width: parent.width
              spacing: Style.space(8)
              PanelButton {
                id: discussionButton
                label: "Discuss before planning"
                  + (root.discussion.length > 0 ? " (" + root.discussion.length + ")" : "")
                onClicked: root.openDiscussion()
              }
              Text {
                width: Math.max(0, parent.width - discussionButton.width - parent.spacing)
                anchors.verticalCenter: discussionButton.verticalCenter
                visible: text !== ""
                text: root.discussionPending ? "Forge is replying…" : root.discussionError
                textFormat: Text.PlainText
                color: root.discussionError !== "" ? root.urgent : root.mutedForeground
                wrapMode: Text.Wrap
                font.family: root.fontFamily
                font.pixelSize: root.fs(11)
              }
            }

            Row {
              width: parent.width
              spacing: Style.space(8)
              Rectangle {
                width: parent.width - improvePlanButton.width - parent.spacing
                height: improvePlanButton.height
                color: root.surface
                radius: 4
                border.width: 1
                border.color: feedbackField.activeFocus
                  ? root.accent : Qt.darker(root.foreground, 3)
                TextInput {
                  id: feedbackField
                  anchors.fill: parent
                  anchors.margins: Style.space(6)
                  verticalAlignment: TextInput.AlignVCenter
                  color: root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(12)
                  selectByMouse: true
                  clip: true
                  onAccepted: root.revisePlan()
                  Keys.onPressed: event => {
                    if (event.key === Qt.Key_F1) {
                      root.helpOpen = true
                      keyHandler.forceActiveFocus()
                      event.accepted = true
                    }
                  }
                  Keys.onEscapePressed: event => {
                    keyHandler.forceActiveFocus()
                    event.accepted = true
                  }
                  Text {
                    visible: feedbackField.text === "" && !feedbackField.activeFocus
                    text: "what should be improved…"
                    color: root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(12)
                  }
                }
              }
              PanelButton {
                id: improvePlanButton
                label: "Improve with AI"
                enabled: root.engineOnline && !root.busy && !root.queueActive
                  && !root.editingPlan && !root.revisePending && root.plan !== null
                  && ["draft", "approved", "done"].indexOf(root.plan.status) !== -1
                  && feedbackField.text.trim() !== ""
                onClicked: root.revisePlan()
              }
            }

            ArchitectureReviewView {
              width: parent.width
              engineState: root.engineState
              plan: root.plan
              architecture: root.architecture
              planReview: root.planReview
              planReviewView: root.planReviewView
              planReviewVersion: root.planReviewVersion
              planReviewScope: root.planReviewScope
              planReviewExpanded: root.planReviewExpanded
              detailScope: JSON.stringify([root.lastProject, root.projectViewRevision, (root.plan || {}).plan_id || ""])
              planReviewStatusText: root.planReviewStatusText
              foreground: root.foreground
              mutedForeground: root.mutedForeground
              background: root.background
              surface: root.surface
              accent: root.accent
              urgent: root.urgent
              fontFamily: root.fontFamily
              fontSize11: root.fs(11)
              fontSize12: root.fs(12)
              onPlanReviewExpansionRequested: expanded => root.planReviewExpanded = expanded
              onPlanReviewLoadRequested: root.loadPlanReviewRequests()
              onLeaveRequested: keyHandler.forceActiveFocus()
              onDetailRevealed: control => root.revealDetail(control)
              onDetailInspected: control => root.inspectDetail(control)
            }

            // ------------------------------------------------ plan Q&A
            Column {
              id: chatSection
              visible: root.plan !== null
              width: parent.width
              spacing: Style.space(6)

              Row {
                width: parent.width
                spacing: Style.space(8)
                PanelButton {
                  id: chatToggle
                  label: (root.chatExpanded ? "▾" : "▸") + " Plan Q&A"
                    + (root.chat.length > 0 ? " (" + root.chat.length + ")" : "")
                  onClicked: root.chatExpanded = !root.chatExpanded
                }
                Rectangle {
                  width: Math.max(0, parent.width - chatToggle.width - askPlanButton.width
                    - parent.spacing * 2)
                  height: askPlanButton.height
                  color: root.surface
                  radius: 4
                  border.width: 1
                  border.color: questionField.activeFocus
                    ? root.accent : Qt.darker(root.foreground, 3)
                  TextInput {
                    id: questionField
                    anchors.fill: parent
                    anchors.margins: Style.space(6)
                    verticalAlignment: TextInput.AlignVCenter
                    color: root.foreground
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(12)
                    selectByMouse: true
                    clip: true
                    onAccepted: root.askPlanQuestion()
                    Keys.onPressed: event => {
                      if (event.key === Qt.Key_F1) {
                        root.helpOpen = true
                        keyHandler.forceActiveFocus()
                        event.accepted = true
                      }
                    }
                    Keys.onEscapePressed: event => {
                      keyHandler.forceActiveFocus()
                      event.accepted = true
                    }
                    Text {
                      visible: questionField.text === "" && !questionField.activeFocus
                      text: "ask about this plan…"
                      color: root.mutedForeground
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(12)
                    }
                  }
                }
                PanelButton {
                  id: askPlanButton
                  label: root.chatPending ? "Asking…" : "Ask"
                  enabled: root.engineOnline && !root.busy && !root.queueActive
                    && !root.editingPlan && !root.revisePending && !root.chatPending
                    && root.plan !== null && questionField.text.trim() !== ""
                  onClicked: root.askPlanQuestion()
                }
              }

              Rectangle {
                visible: root.chatExpanded && root.chat.length > 0
                width: parent.width
                height: Math.min(chatList.contentHeight, Style.space(96)) + Style.space(16)
                color: root.surface
                radius: 4
                Flickable {
                  id: chatList
                  anchors.fill: parent
                  anchors.margins: Style.space(8)
                  clip: true
                  boundsBehavior: Flickable.StopAtBounds
                  contentHeight: chatDetails.height
                  property bool followTail: true
                  property real readingY: 0
                  function scrollToTail() { if (followTail && !moving) contentY = Math.max(0, contentHeight - height) }
                  function restoreReadingPosition() {
                    if (!followTail && !moving) contentY = Math.max(0, Math.min(readingY, Math.max(0, contentHeight - height)))
                  }
                  onContentYChanged: if (moving) { followTail = atYEnd; readingY = contentY }
                  onMovementEnded: { followTail = atYEnd; readingY = contentY }
                  onContentHeightChanged: Qt.callLater(function() { restoreReadingPosition(); scrollToTail() })
                  onHeightChanged: Qt.callLater(function() { restoreReadingPosition(); scrollToTail() })
                  onVisibleChanged: if (visible) Qt.callLater(scrollToTail)
                  PanelFields {
                    id: chatDetails
                    objectName: "chatDetails"
                    width: chatList.width
                    entries: root.chat.map(function(message, i) {
                      // Chat has no durable IDs. Scope by plan, position and the
                      // entire original record, keeping identical adjacent messages distinct.
                      return PanelDetails.field(JSON.stringify([i, message]),
                        message.role === "user" ? "You" : "Forge", message.text)
                    })
                    onInspecting: { chatList.followTail = false; chatList.readingY = chatList.contentY }
                  }
                }
              }
            }
            PanelDetail {
              objectName: "localErrorDetail"
              width: parent.width
              visible: root.localError !== ""
              originalText: root.localError
              metadata: "Error"
              error: true
            }

            // -------------------------------------------------- queue
            Rectangle {
              id: queueSection
              visible: root.queue.length > 0
              width: parent.width
              height: queueHeader.height + queueList.height + Style.space(24)
              color: root.surface
              radius: 4

              Row {
                id: queueHeader
                anchors.top: parent.top
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.margins: Style.space(8)
                spacing: Style.space(8)
                Text {
                  width: parent.width - startQueueButton.width - parent.spacing
                  anchors.verticalCenter: parent.verticalCenter
                  text: "Queue (" + root.queue.length + ")"
                  color: root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(12)
                  font.bold: true
                }
                PanelButton {
                  id: startQueueButton
                  label: "Start queue"
                  primary: true
                  enabled: !root.editingPlan && root.engineOnline && !root.busy && !root.queueActive && root.hasQueuedGoals
                  onClicked: root.act("/api/queue/start")
                }
              }

              ListView {
                id: queueList
                anchors.top: queueHeader.bottom
                anchors.topMargin: Style.space(8)
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.margins: Style.space(8)
                height: Math.min(contentHeight, Style.space(96))
                clip: true
                spacing: Style.space(4)
                model: root.queue
                delegate: Row {
                  id: queueRow
                  required property var modelData
                  required property int index
                  readonly property bool active: modelData.status === "planning"
                    || modelData.status === "awaiting_approval" || modelData.status === "running"
                  width: queueList.width
                  height: Math.max(queueGoal.implicitHeight, queueControls.implicitHeight)
                  spacing: Style.space(8)
                  Text {
                    id: queueGlyph
                    width: root.fs(12)
                    anchors.verticalCenter: parent.verticalCenter
                    text: queueRow.active ? "●" : queueRow.modelData.status === "done" ? "✓"
                      : queueRow.modelData.status === "blocked" || queueRow.modelData.status === "failed"
                        ? "!" : "·"
                    color: queueRow.active ? root.accent
                      : queueRow.modelData.status === "blocked" || queueRow.modelData.status === "failed"
                        ? root.urgent : root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(12)
                    font.bold: true
                  }
                  PanelDetail {
                    id: queueGoal
                    width: Math.max(0, parent.width - queueGlyph.width - parent.spacing
                      - (queueControls.visible ? queueControls.width + parent.spacing : 0))
                    metadata: "Goal"
                    originalText: queueRow.modelData.goal
                  }
                  Row {
                    id: queueControls
                    visible: queueRow.modelData.status === "queued"
                      || queueRow.modelData.status === "failed" || queueRow.modelData.status === "blocked"
                    spacing: Style.space(4)
                    PanelButton {
                      label: "↑"
                      visible: queueRow.modelData.status === "queued"
                      enabled: root.engineOnline && root.canMoveQueueGoal(queueRow.index, -1)
                      onClicked: root.act("/api/queue/move", { id: queueRow.modelData.id, dir: "up" })
                    }
                    PanelButton {
                      label: "↓"
                      visible: queueRow.modelData.status === "queued"
                      enabled: root.engineOnline && root.canMoveQueueGoal(queueRow.index, 1)
                      onClicked: root.act("/api/queue/move", { id: queueRow.modelData.id, dir: "down" })
                    }
                    PanelButton {
                      label: "×"
                      enabled: root.engineOnline
                      onClicked: root.act("/api/queue/remove", { id: queueRow.modelData.id })
                    }
                  }
                }
              }
            }

            PlanEditorView {
              id: planEditor

              width: parent.width
              editingPlan: root.editingPlan
              editPending: root.editPending
              editValid: root.editValid
              queueActive: root.queueActive
              engineOnline: root.engineOnline
              busy: root.busy
              displayedStages: root.displayedStages
              editStages: root.editStages
              plan: root.plan
              stageSnapshot: root.stageSnapshot
              expandedStageId: root.expandedStageId
              selectedStageIndex: root.selectedStageIndex
              stageRoutingExpanded: root.stageRoutingExpanded
              stageReviewBlocks: root.stageReviewBlocks
              agentNow: root.agentNow
              panelHeight: panelScroll.height
              editFocusedField: root.editFocusedField
              fs: root.fs
              stageActivity: root.stageActivity
              stageDetailScope: root.stageDetailScope
              reviewView: root.reviewView
              reviewGateText: root.reviewGateText
              reviewDecision: root.reviewDecision
              reviewFields: root.reviewFields
              reviewRoundLabel: root.reviewRoundLabel
              reviewTimestamp: root.reviewTimestamp
              stageReviewIncompleteRange: root.stageReviewIncompleteRange
              stageReviewHasHeldPreview: root.stageReviewHasHeldPreview
              ensureStageReviewsLoaded: root.ensureStageReviewsLoaded
              reconcileStageReviewPresentation: root.reconcileStageReviewPresentation
              foreground: root.foreground
              mutedForeground: root.mutedForeground
              background: root.background
              surface: root.surface
              accent: root.accent
              urgent: root.urgent
              success: root.success
              working: root.working
              fontFamily: root.fontFamily
              onSavePlanEdit: root.savePlanEdit()
              onCancelPlanEdit: root.cancelPlanEdit()
              onAddEditStage: root.addEditStage()
              onMoveEditStage: (index, direction) => root.moveEditStage(index, direction)
              onDeleteEditStage: index => root.deleteEditStage(index)
              onChangeStageField: (index, field, value) => root.changeStageField(index, field, value)
              onChangeModelConstraint: (index, key, value) => root.changeModelConstraint(index, key, value)
              onLoadStageReviews: (stageId, cursor, end) => root.loadStageReviews(stageId, cursor, end)
              onRetryStageReviews: stage => root.retryStageReviews(stage)
              onExpandedStageRequested: stageId => root.expandedStageId = stageId
              onStageRoutingExpandedRequested: expanded => root.stageRoutingExpanded = expanded
              onEditFocusChanged: field => root.editFocusedField = field
              onHelpRequested: root.helpOpen = true
              onLeaveRequested: keyHandler.forceActiveFocus()
              onDetailRevealed: control => panelScroll.reveal(control)
              onDetailInspected: control => root.inspectDetail(control)
            }

            AgentOutputView {
              id: agentOutput

              width: parent.width
              liveTab: root.liveTab
              historyFilter: root.historyFilter
              hasReports: root.hasReports
              reportsVisible: root.reportsVisible
              logError: root.logError
              liveModel: liveEntries
              historyModel: historyEntries
              reportIndex: root.reportIndex
              projectViewRevision: root.projectViewRevision
              selectedReportKey: root.selectedReportKey
              expandedReportKey: root.expandedReportKey
              now: root.agentNow
              panelHeight: panelScroll.height
              detailScope: JSON.stringify([root.lastProject, root.projectViewRevision, (root.plan || {}).plan_id || ""])
              foreground: root.foreground
              mutedForeground: root.mutedForeground
              background: root.background
              surface: root.surface
              accent: root.accent
              urgent: root.urgent
              success: root.success
              working: root.working
              fontFamily: root.fontFamily
              fontSize10: root.fs(10)
              fontSize11: root.fs(11)
              onLiveTabRequested: live => root.liveTab = live
              onHistoryFilterRequested: filter => root.historyFilter = filter
              onReportSelected: key => root.selectedReportKey = key
              onReportExpansionRequested: key => root.expandedReportKey = key
              onLeaveRequested: keyHandler.forceActiveFocus()
              onDetailRevealed: control => root.revealDetail(control)
              onDetailInspected: control => root.inspectDetail(control)
            }
          }
          }

          Text {
            id: keyboardHint
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            anchors.margins: Style.space(16)
            text: root.insertMode ? "INSERT - Esc to normal mode" : "NORMAL - click here or press ? (Shift+/) or F1 for keyboard help"
            color: root.mutedForeground
            font.family: root.fontFamily
            font.pixelSize: root.fs(10)
            font.underline: !root.insertMode && keyboardHintMouseArea.containsMouse
            MouseArea {
              id: keyboardHintMouseArea
              anchors.fill: parent
              hoverEnabled: true
              cursorShape: Qt.PointingHandCursor
              onClicked: if (!root.helpOpen) root.helpOpen = true
            }
          }
        }
      }

      // ------------------------------------------- pre-planning discussion
      // A persistent stack page: pushed and popped by open/closeDiscussion, so
      // it keeps its transcript, reading position and draft. No anchors, size
      // or visible binding - the StackView owns all three.
      DiscussionView {
        id: discussionView

        entries: root.discussion
        pending: root.discussionPending
        sentMessage: root.discussionSent
        error: root.discussionError
        canSend: root.discussionCanSend
        canPlan: root.discussionCanPlan
        canClear: root.discussionCanClear
        foreground: root.foreground
        mutedForeground: root.mutedForeground
        background: root.background
        surface: root.surface
        accent: root.accent
        urgent: root.urgent
        fontFamily: root.fontFamily
        fontSize11: root.fs(11)
        fontSize12: root.fs(12)
        onSendRequested: message => root.sendDiscussionMessage(message)
        onPlanRequested: root.planFromDiscussion()
        onClearRequested: root.clearDiscussion()
        onCloseRequested: root.closeDiscussion()
        onLeaveRequested: keyHandler.forceActiveFocus()
        onHelpRequested: root.helpOpen = true
        onDetailRevealed: control => root.revealDetail(control)
        onDetailInspected: control => root.inspectDetail(control)
      }

      // ------------------------------------------- model policy and options
      CatalogueEditor {
        id: catalogueEditorView

        anchors.fill: parent
        open: root.catalogueOpen
        engineOnline: root.engineOnline
        engineBusy: !!root.engineState && (root.engineState.busy || root.engineState.queue_active)
        aiPending: root.catalogueAiPending
        aiReady: root.catalogueAiReady
        aiUndo: root.catalogueAiUndo
        aiMessage: root.catalogueAiMessage
        catalogue: root.catalogue
        details: root.catalogueDetails
        draft: root.catalogueDraft
        detailScope: JSON.stringify([root.lastProject, root.projectViewRevision, (root.plan || {}).plan_id || ""])
        foreground: root.foreground
        mutedForeground: root.mutedForeground
        background: root.background
        surface: root.surface
        accent: root.accent
        urgent: root.urgent
        fontFamily: root.fontFamily
        fontSize10: root.fs(10)
        fontSize11: root.fs(11)
        onCloseRequested: root.catalogueOpen = false
        onSuggestRequested: root.suggestCatalogue()
        onApplyRequested: root.applyCatalogueSuggestion()
        onUndoRequested: root.undoCatalogueSuggestion()
        onReloadRequested: root.reloadCatalogue()
        onSaveRequested: root.saveCatalogue()
        onMetadataRefreshRequested: root.act("/api/models/metadata/refresh", {})
        onLeaveRequested: keyHandler.forceActiveFocus()
        onDetailRevealed: control => root.revealDetail(control)
        onDetailInspected: control => root.inspectDetail(control)
      }

      // ------------------------------------------------ diff viewer
      DiffView {
        id: diffView

        anchors.fill: parent
        open: root.diffOpen
        engineOnline: root.engineOnline
        pending: root.diffPending
        diffText: root.diffText
        errorText: root.diffError
        foreground: root.foreground
        mutedForeground: root.mutedForeground
        background: root.background
        surface: root.surface
        accent: root.accent
        urgent: root.urgent
        success: root.success
        info: root.info
        fontFamily: root.fontFamily
        fontSize10: root.fs(10)
        fontSize11: root.fs(11)
        onCloseRequested: root.diffOpen = false
        onRefreshRequested: root.refreshDiff()
        onLeaveRequested: keyHandler.forceActiveFocus()
        onDetailInspected: control => root.inspectDetail(control)
      }

      // ------------------------------------------------ project chooser
      ProjectChooser {
        id: projectChooser

        anchors.fill: parent
        open: root.chooserOpen
        manualEntry: root.manualEntry
        rowsForFilter: filter => root.chooserRows(root.projectsData, filter)
        foreground: root.foreground
        mutedForeground: root.mutedForeground
        background: root.background
        surface: root.surface
        accent: root.accent
        urgent: root.urgent
        success: root.success
        fontFamily: root.fontFamily
        fontSize10: root.fs(10)
        fontSize11: root.fs(11)
        fontSize12: root.fs(12)
        onCloseRequested: root.chooserOpen = false
        onRowChosen: row => root.chooseRow(row)
        onManualPathRequested: path => {
          root.act("/api/project", {path: path})
          root.chooserOpen = false
        }
        onHelpRequested: root.helpOpen = true
        onLeaveRequested: keyHandler.forceActiveFocus()
        onDetailInspected: control => root.inspectDetail(control)
      }

      // ------------------------------------------------ keyboard help
      Rectangle {
        visible: root.helpOpen
        anchors.fill: parent
        z: 10
        color: Qt.rgba(0, 0, 0, 0.55)
        MouseArea {
          anchors.fill: parent
          onClicked: root.helpOpen = false
          onWheel: wheel => { wheel.accepted = true }
        }

        Rectangle {
          anchors.centerIn: parent
          width: parent.width * 0.92
          height: parent.height * 0.9
          radius: 6
          color: root.surface
          border.width: 1
          border.color: Qt.darker(root.foreground, 3)
          MouseArea {
            anchors.fill: parent
            onWheel: wheel => { wheel.accepted = true }
          }

          Column {
            anchors.fill: parent
            anchors.margins: Style.space(12)
            spacing: Style.space(8)

            Text {
              text: "Keyboard · :help"
              color: root.accent
              font.family: root.fontFamily
              font.pixelSize: root.fs(16)
              font.bold: true
            }

            Text {
              width: parent.width
              text: "Normal mode uses shortcuts. Focus a text field for insert mode; Escape returns to normal. Actions follow the buttons’ enabled state."
              wrapMode: Text.Wrap
              color: root.mutedForeground
              font.family: root.fontFamily
              font.pixelSize: root.fs(11)
            }

            Flickable {
              id: helpList
              width: parent.width
              height: parent.height - y - helpFooter.height - parent.spacing
              contentWidth: width
              contentHeight: helpRows.height
              clip: true
              boundsBehavior: Flickable.StopAtBounds
              onVisibleChanged: if (visible) contentY = 0

              Column {
                id: helpRows
                width: helpList.width
                spacing: Style.space(5)

                Repeater {
                  model: [
                    { key: "", description: "Panel · normal mode" },
                    { key: "i", description: "Edit the goal (insert mode)" },
                    { key: "I", description: "Edit plan feedback (insert mode); Enter improves with AI" },
                    { key: "Escape", description: "Leave a text field, close the top overlay, or cancel plan editing" },
                    { key: "j / k", description: "Select next / previous stage (report in Reports)" },
                    { key: "gg / G", description: "Select first / last stage (report in Reports)" },
                    { key: "Enter / o / Space", description: "Expand or collapse selected stage or report; focus stage title when editing" },
                    { key: "Tab", description: "Toggle Live / History" },
                    { key: "h / l", description: "Select Live / History" },
                    { key: "Ctrl+d / Ctrl+u", description: "Scroll Live / History half a page down / up" },
                    { key: "Page Down / Page Up", description: "Scroll the whole panel down / up" },
                    { key: "Home / End", description: "Jump to the top / bottom of the panel" },
                    { key: "1 / 2 / 3 / 4 / 5", description: "History: All / Runs / Git / Reviews / Errors" },
                    { key: "6", description: "History: Reports (when available)" },
                    { key: "p", description: "Create plan from goal" },
                    { key: "t", description: "Focus the discussion message input" },
                    { key: "P", description: "Create plan from discussion" },
                    { key: "E", description: "Enhance the goal description with AI" },
                    { key: "e", description: "Edit plan stages by hand" },
                    { key: "a", description: "Approve draft plan" },
                    { key: "r", description: "Run approved or completed plan" },
                    { key: "x", description: "Stop run or active queue" },
                    { key: "d", description: "Open uncommitted diff" },
                    { key: "c", description: "Change project" },
                    { key: "? (Shift+/) / F1", description: "Open keyboard help" },
                    { key: "", description: "Diff viewer" },
                    { key: "j / k", description: "Scroll down / up" },
                    { key: "Ctrl+d / Ctrl+u", description: "Scroll half a page down / up" },
                    { key: "gg / G", description: "Jump to top / bottom" },
                    { key: "R", description: "Refresh diff" },
                    { key: "q / Escape", description: "Close diff" },
                    { key: "", description: "Project chooser · normal mode" },
                    { key: "j / k", description: "Select next / previous project" },
                    { key: "Enter", description: "Open selection; in filter, open first match; in path field, set path" },
                    { key: "/ / i", description: "Edit project filter (insert mode)" },
                    { key: "q / Escape", description: "Close chooser (Escape leaves a text field first)" },
                    { key: "", description: "Keyboard help" },
                    { key: "? (Shift+/) / F1 / q / Escape", description: "Close help before any other overlay" }
                  ]

                  delegate: Row {
                    id: helpRow
                    required property var modelData
                    readonly property bool heading: modelData.key === ""
                    width: helpRows.width
                    spacing: Style.space(10)

                    Text {
                      visible: !helpRow.heading
                      width: helpRows.width * 0.36
                      text: helpRow.modelData.key
                      wrapMode: Text.Wrap
                      color: root.accent
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(11)
                    }
                    Text {
                      width: helpRow.heading ? helpRows.width
                        : helpRows.width * 0.64 - helpRow.spacing
                      topPadding: helpRow.heading ? Style.space(6) : 0
                      text: helpRow.modelData.description
                      wrapMode: Text.Wrap
                      color: root.foreground
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(11)
                      font.bold: helpRow.heading
                    }
                  }
                }
              }
            }

            Text {
              id: helpFooter
              width: parent.width
              text: "Scroll for more · Uppercase keys use Shift · ? (Shift+/) / F1 / q / Escape closes help"
              wrapMode: Text.Wrap
              color: root.mutedForeground
              font.family: root.fontFamily
              font.pixelSize: root.fs(10)
            }
          }
        }
      }
    }
  }



  component CadenceButton: PanelButton {
    id: cadenceButton
    required property string role
    label: role + " review: " + root.reviewCadenceLabel(root.engineState, role)
    enabled: root.engineOnline
    activeFocusOnTab: enabled
    border.color: activeFocus ? root.accent : Qt.darker(root.foreground, 3)
    onClicked: root.toggleReviewCadence(role)
    onActiveFocusChanged: if (activeFocus) root.revealDetail(cadenceButton)
    Keys.onPressed: event => {
      if (event.key === Qt.Key_Escape) keyHandler.forceActiveFocus()
      else if (event.key === Qt.Key_Space || event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
        if (enabled && !event.isAutoRepeat) clicked()
      } else if (event.key === Qt.Key_Tab || event.key === Qt.Key_Backtab) return
      event.accepted = true
    }
  }

  component PanelButton: PanelViewButton {
    foreground: root.foreground
    background: root.background
    surface: root.surface
    accent: root.accent
    fontFamily: root.fontFamily
    fontSize: root.fs(11)
  }
}
