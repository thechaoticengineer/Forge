pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
// qs.Ui exports its own Button, which shadows the Controls one and carries a
// different API. Controls-specific buttons name it explicitly.
import Quickshell
import qs.Commons
import qs.Ui
import "DetailView.js" as DetailView
import "ReviewView.js" as ReviewView
import "ReviewPresentation.js" as ReviewPresentation
import "PlanReview.js" as PlanReview
import "PanelDetails.js" as PanelDetails
import "ModelRouting.js" as ModelRouting
import "UsageFormat.js" as UsageFormat
import "GoalEnhancement.js" as GoalEnhancement
import "Discussion.js" as Discussion
import "PanelNavigation.js" as PanelNavigation
import "PanelActions.js" as PanelActions

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
  // The stage detail page follows the stage expandedStageId names across polls and closes
  // only when that stage goes (a project switch clears expandedStageId).
  readonly property bool stageDetailOpen: panelStack.currentItem === stageDetailPage
  readonly property int stageDetailIndex: (displayedStages || []).findIndex(s => s.id === expandedStageId)
  onStageDetailIndexChanged: if (stageDetailOpen) {
    if (stageDetailIndex < 0) closeStageDetail()
    else selectedStageIndex = stageDetailIndex
  }
  property string stageDetailTab: "instructions"
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

  PanelPalette { id: statusPalette }
  readonly property color success: statusPalette.success
  readonly property color working: statusPalette.working
  readonly property color info: statusPalette.info

  // Font size in design pixels; Style.fontPx takes a multiplier of the 12px base.
  function fs(px) { return Style.fontPx(px / 12) }
  // The palette and font sizes every view takes as its theme.
  readonly property var theme: ({
    foreground: foreground, mutedForeground: mutedForeground, background: background, surface: surface,
    accent: accent, urgent: urgent, success: success, working: working, info: info, fontFamily: fontFamily,
    fontSize10: fs(10), fontSize11: fs(11), fontSize12: fs(12)
  })

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

  // The selected view tab. Only the user changes it: polling, project switches,
  // reopening and closing overlays keep it (a new panel starts on Overview).
  property string currentTab: "overview"
  // The tab shown before the current one; q / Escape in Features return to it.
  property string previousTab: "overview"
  property string shownTab: "overview"
  onCurrentTabChanged: {
    if (currentTab !== shownTab) {
      previousTab = shownTab
      shownTab = currentTab
    }
    // featuresOpen drives the Features keys and activity polling while its tab is shown.
    features.featuresOpen = currentTab === "features"
    if (features.featuresOpen && features.featuresLoadedRevision !== projectViewRevision)
      features.openFeatures()
  }
  readonly property string stepText: engineState !== null && engineState.current_step !== ""
    ? (engineState.current_stage !== null
      ? "stage " + engineState.current_stage + ": " + (currentActivity || engineState.current_step)
      : engineState.current_step) : ""
  // One guard per action, shared by its button and its keyboard shortcut.
  readonly property var guardFlags: ({
    engineOnline: engineOnline, busy: busy, phase: phase, queueActive: queueActive,
    editingPlan: editingPlan, revisePending: revisePending, chatPending: chatPending,
    planStatus: plan !== null ? plan.status : null, goalText: goalField.text,
    feedbackText: feedbackField.text, questionText: questionField.text,
    goalEnhancePending: goalEnhancePending, hasQueuedGoals: hasQueuedGoals
  })
  readonly property var guards: PanelActions.guards(guardFlags)

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

  property bool chooserOpen: false
  ChooserController {
    id: chooserController
    host: root
    chooser: projectChooser
    keyTarget: keyHandler
  }

  property alias diffOpen: diffController.diffOpen
  DiffController {
    id: diffController
    host: root
    lastProject: root.lastProject
    projectViewRevision: root.projectViewRevision
    active: window.visible
    busy: root.busy
    listView: diffView.listView
  }

  FeaturesController {
    id: features
    host: root
    lastProject: root.lastProject
    projectViewRevision: root.projectViewRevision
    active: window.visible
  }

  CatalogueController {
    id: catalogueController
    host: root
    engineState: root.engineState
    catalogue: root.catalogue
    catalogueEditorView: catalogueEditorView
    lastProject: root.lastProject
    projectViewRevision: root.projectViewRevision
  }
  readonly property var catalogue: engineState && engineState.model_catalogue ? engineState.model_catalogue : null

  property bool helpOpen: false
  // The Plan view owns these controls; the panel keeps driving them by name.
  property alias planEditor: planView.planEditor
  property alias feedbackField: planView.feedbackField
  property alias questionField: planView.questionField
  property alias chatList: planView.chatList
  // The Overview goal field; drafts and enhancement undo keep driving it by name.
  property alias goalField: overviewView.goalField
  property alias goalFlick: overviewView.goalFlick
  readonly property bool insertMode: goalField.activeFocus
    || feedbackField.activeFocus || questionField.activeFocus || discussionView.input.activeFocus
    || projectChooser.filterField.activeFocus || projectChooser.manualField.activeFocus || catalogueEditorView.editor.activeFocus
    || featuresView.newSlugField.activeFocus || featuresView.newTitleField.activeFocus
    || featuresView.chatMessageField.activeFocus
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
  // The Activity tab's AgentOutputView: keyboard routing and reading positions.
  readonly property alias agentOutput: activityView.output
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
    if (project === lastProject) { syncHistory(); syncGoalEnhancement(); catalogueController.syncCatalogueSuggestion(); syncReviewViews(); syncDiscussion(); return }
    if (lastProject !== "") goalDrafts[lastProject] = goalField.text
    lastProject = project
    // Ignore log/diff responses from an earlier visit, even after switching back.
    projectViewRevision++
    catalogueController.reset()
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
    features.reset()
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
    if (!root.guards.enhance) return
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
    if (!root.guards.improve) return
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
    if (!root.guards.ask) return
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

  // Hand editing (e) lives in PlanEditController; the keys and views call these.
  function beginPlanEdit() { planEdit.beginPlanEdit() }
  function cancelPlanEdit() { planEdit.cancelPlanEdit() }
  PlanEditController {
    id: planEdit
    host: root
    keyTarget: keyHandler
    editor: root.planEditor
  }

  function openChooser() { chooserController.openChooser() }

  function openDiff() { diffController.openDiff() }
  function refreshDiff() { diffController.refreshDiff() }

  // Stage detail: a persistent page pushed over the tab views. Plan keeps its
  // selection and reading position underneath, so closing returns to them.
  function openStageDetail(index) {
    const stages = displayedStages
    const at = Math.max(0, Math.min(index, stages.length - 1))
    if (stages.length === 0 || stages[at].id === undefined) return
    if (!stageDetailOpen) stageDetailTab = "instructions"
    currentTab = "plan"
    selectedStageIndex = at
    expandedStageId = stages[at].id
    if (!stageDetailOpen) panelStack.push(stageDetailPage, StackView.Immediate)
    keyHandler.pendingKey = ""
    keyHandler.forceActiveFocus()
  }

  function closeStageDetail() {
    if (stageDetailOpen) panelStack.pop(panelPage, StackView.Immediate)
    expandedStageId = -1
    keyHandler.pendingKey = ""
    keyHandler.forceActiveFocus()
  }

  // A tab click or a g jump leaves a pushed page first, then shows the tab.
  function selectTab(id) {
    if (stageDetailOpen) closeStageDetail()
    if (discussionOpen) closeDiscussion()
    currentTab = id
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
  function stageActivity(stage) {
    if (!engineState || !busy || stage.status !== "in_progress"
        || stage.id !== engineState.current_stage) return ""
    const step = engineState.current_step || ""
    if (!step) return ""
    const activity = step.indexOf("fixing") === 0 ? "fixing for review" : step
    return "now: " + activity + " · " + ReviewPresentation.reviewRoundLabel({ round: stage.rounds })
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
          // The stages live on Plan: show the view that the selection acts on.
          root.currentTab = "plan"
          root.selectedStageIndex = Math.max(0, Math.min(index, stages.length - 1))
          if (root.editingPlan) {
            planEditor.stageList.positionViewAtIndex(root.selectedStageIndex, ListView.Contain)
            planView.reveal(planEditor.stageFrame)
          } else {
            const row = planView.stageRows.rowItem(root.selectedStageIndex)
            if (row) planView.reveal(row)
          }
        }

        function selectReport(index) {
          if (root.reports.length === 0) return
          const selected = Math.max(0, Math.min(index, root.reports.length - 1))
          root.selectedReportKey = root.reportKey(root.reports[selected], selected)
          agentOutput.reportList.positionViewAtIndex(selected, ListView.Contain)
          agentOutput.reportList.captureReading()
        }

        // The scrollable area of the current tab for Page Up/Down and Home/End.
        function tabScroller() {
          return root.currentTab === "architecture" ? architectureView
            : root.currentTab === "queue" ? queueView.listView
            : root.currentTab === "plan" ? planView
            : root.currentTab === "settings" ? settingsView : overviewView
        }

        // Settings owns j/k, gg/G and Enter while it is shown.
        function settingsKey(event, prefix) {
          const result = settingsView.handleKey(event, prefix)
          if (result === "g") pendingKey = "g"
          return result !== ""
        }

        Keys.onPressed: event => {
          const prefix = pendingKey
          pendingKey = ""
          const question = (event.key === Qt.Key_Question
            && (event.modifiers === Qt.NoModifier || event.modifiers === Qt.ShiftModifier))
            || (event.key === Qt.Key_Slash && event.modifiers === Qt.ShiftModifier)
            || (event.key === Qt.Key_F1 && event.modifiers === Qt.NoModifier)
          // View keys: ] / [ step through the tabs; g then o/p/a/r/f/q/s jumps to one.
          const viewTab = event.modifiers !== Qt.NoModifier ? ""
            : event.key === Qt.Key_BracketRight || event.key === Qt.Key_BracketLeft
              ? PanelNavigation.stepTab(root.currentTab, event.key === Qt.Key_BracketRight ? 1 : -1)
            : prefix === "g" ? PanelNavigation.tabForGKey(event.text) : ""
          // The project switcher and the ⋯ menu close on any key; Escape only closes them.
          if (overflowMenu.open || panelHeader.switcherOpen) {
            overflowMenu.open = false
            panelHeader.closeSwitcher()
            if (event.key === Qt.Key_Escape) {
              event.accepted = true
              return
            }
          }
          // Modal normal mode owns every key; focused text fields handle insert mode.
          if (catalogueController.catalogueOpen) {
            event.accepted = true
            if (event.key === Qt.Key_Escape) catalogueController.catalogueOpen = false
            return
          }
          if (root.helpOpen) {
            event.accepted = true
            if (question || event.key === Qt.Key_Escape
                || (event.key === Qt.Key_Q && event.modifiers === Qt.NoModifier))
              root.helpOpen = false
          } else if (root.diffOpen) {
            event.accepted = true
            pendingKey = diffView.handleKey(event, prefix)
          } else if (root.chooserOpen) {
            event.accepted = true
            projectChooser.handleKey(event)
          } else if (root.discussionOpen) {
            event.accepted = true
            if (question) {
              root.helpOpen = true
            } else if (event.key === Qt.Key_Escape
                || (event.key === Qt.Key_Q && event.modifiers === Qt.NoModifier)) {
              root.closeDiscussion()
            } else if (event.key === Qt.Key_I && event.modifiers === Qt.NoModifier) {
              discussionView.input.forceActiveFocus()
            }
          } else if (root.stageDetailOpen && !(prefix === "g" && viewTab !== "")) {
            // The page owns ] [ l h q Escape; g and a tab letter still jump (viewTab below).
            event.accepted = true
            if (question) root.helpOpen = true
            else if (stageDetailPage.handleKey(event) === "" && event.modifiers === Qt.NoModifier) {
              if (event.key === Qt.Key_G) pendingKey = "g"
              else if (event.key === Qt.Key_D) { if (root.guards.diff) root.openDiff() }
              else if (event.key === Qt.Key_X) { if (root.guards.stop) root.act("/api/stop") }
            }
          } else if (features.featuresOpen) {
            // The Features tab while no page is pushed on top of it.
            event.accepted = true
            if (viewTab !== "") root.currentTab = viewTab
            else if (question) root.helpOpen = true
            else pendingKey = featuresView.handleKey(event)
          } else if (question) {
            root.helpOpen = true
            event.accepted = true
          } else if (viewTab !== "") {
            root.selectTab(viewTab)
            event.accepted = true
          } else if (root.currentTab === "settings" && settingsKey(event, prefix)) {
            event.accepted = true
          } else if (event.key === Qt.Key_I && event.modifiers === Qt.NoModifier) {
            root.currentTab = "overview"
            overviewView.focusGoal()
            event.accepted = true
          } else if (event.key === Qt.Key_I && event.modifiers === Qt.ShiftModifier) {
            // The feedback field lives on Plan.
            root.currentTab = "plan"
            feedbackField.forceActiveFocus()
            planView.reveal(feedbackField.parent)
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
              // Output keys act on Activity, the view that shows the output.
              root.currentTab = "activity"
              activityView.scrollOutput(event.key === Qt.Key_D ? 1 : -1)
              event.accepted = true
            } else if (event.modifiers === Qt.NoModifier) {
              // Keep action guards identical to their buttons: both read root.guards.
              if (event.key === Qt.Key_PageDown || event.key === Qt.Key_PageUp) {
                const scroller = tabScroller()
                const step = event.key === Qt.Key_PageDown ? 1 : -1
                scroller.cancelFlick()
                scroller.contentY = Math.max(scroller.originY, Math.min(scroller.originY
                  + Math.max(0, scroller.contentHeight - scroller.height), scroller.contentY + step * scroller.height * 0.8))
                event.accepted = true
              } else if (event.key === Qt.Key_Home || event.key === Qt.Key_End) {
                const scroller = tabScroller()
                scroller.cancelFlick()
                scroller.contentY = event.key === Qt.Key_Home ? scroller.originY
                  : scroller.originY + Math.max(0, scroller.contentHeight - scroller.height)
                event.accepted = true
              } else if (event.key === Qt.Key_P) {
                if (root.guards.createPlan)
                  root.act("/api/plan", { goal: goalField.text })
                event.accepted = true
              } else if (event.key === Qt.Key_A) {
                if (root.guards.approve)
                  root.act("/api/approve")
                event.accepted = true
              } else if (event.key === Qt.Key_R) {
                if (root.guards.run)
                  root.act("/api/run")
                event.accepted = true
              } else if (event.key === Qt.Key_E) {
                if (root.guards.editPlan) root.beginPlanEdit()
                event.accepted = true
              } else if (event.key === Qt.Key_X) {
                if (root.guards.stop)
                  root.act("/api/stop")
                event.accepted = true
              } else if (event.key === Qt.Key_D) {
                if (root.guards.diff) root.openDiff()
                event.accepted = true
              } else if (event.key === Qt.Key_C) {
                if (root.guards.changeProject) root.openChooser()
                event.accepted = true
              } else if (event.key === Qt.Key_F) {
                if (root.guards.features) features.openFeatures()
                event.accepted = true
              } else if (event.key === Qt.Key_Tab) {
                root.currentTab = "activity"
                root.liveTab = !root.liveTab
                event.accepted = true
              } else if (event.key === Qt.Key_H || event.key === Qt.Key_L) {
                root.currentTab = "activity"
                root.liveTab = event.key === Qt.Key_H
                event.accepted = true
              } else if (event.key >= Qt.Key_1 && event.key <= Qt.Key_6) {
                root.currentTab = "activity"
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
                  if (root.editingPlan && stages[root.selectedStageIndex].status !== "committed") {
                    root.currentTab = "plan"
                    const row = planEditor.stageList.itemAtIndex(root.selectedStageIndex)
                    if (row) row.focusEditor()
                  } else root.openStageDetail(root.selectedStageIndex)
                }
                event.accepted = true
              }
            }
          }
        }

        // ------------------------------------------ fixed header and tabs
        PanelHeader {
          id: panelHeader
          anchors.top: parent.top
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.margins: Style.space(16)
          projectName: root.projectName
          projectPath: root.engineState ? root.engineState.project : ""
          sessions: root.sessions
          activeProject: root.activeProject
          phase: root.phase
          engineOnline: root.engineOnline
          busy: root.busy
          stepText: root.stepText
          backgroundBusy: root.backgroundBusy
          activeProjectCount: root.activeProjectCount
          theme: root.theme
          titleFontSize: root.fs(18)
          spacing: Style.space(10)
          onProjectSelected: path => root.act("/api/project/select", { path: path })
          onChangeProjectRequested: if (root.guards.changeProject) root.openChooser()
          onOverflowRequested: overflowMenu.open = true
        }

        PanelTabBar {
          id: panelTabBar
          anchors.top: panelHeader.bottom
          anchors.topMargin: Style.space(8)
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.leftMargin: Style.space(16)
          anchors.rightMargin: Style.space(16)
          currentTab: root.currentTab
          queueCount: root.queue.length
          theme: root.theme
          onTabRequested: id => root.selectTab(id)
        }

        // The pages under the header: the tab views (panelPage), the discussion
        // and stage detail. The header, tab bar, errors and hint stay around them.
        Item {
          id: stackArea
          anchors.top: panelTabBar.bottom
          anchors.topMargin: Style.space(10)
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.bottom: localErrorArea.top
          anchors.bottomMargin: localErrorDetail.visible ? Style.space(10) : 0

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
        }

        Item {
          id: panelPage

          // The content area under the tab bar. Each tab has one persistent
          // item that is shown while its tab is selected, never rebuilt.
          Item {
            id: tabArea
            anchors.fill: parent
            anchors.leftMargin: Style.space(16)
            anchors.rightMargin: Style.space(16)

            // ------------------------------------------------ plan
            PlanView {
              id: planView

              anchors.fill: parent
              visible: root.currentTab === "plan"
              guards: root.guards
              editingPlan: root.editingPlan
              editPending: root.editPending
              editValid: root.editValid
              queueActive: root.queueActive
              engineOnline: root.engineOnline
              busy: root.busy
              displayedStages: root.displayedStages
              editStages: root.editStages
              plan: root.plan
              planReview: root.planReview
              expandedStageId: root.expandedStageId
              selectedStageIndex: root.selectedStageIndex
              agentNow: root.agentNow
              editFocusedField: root.editFocusedField
              fs: root.fs
              stageActivity: root.stageActivity
              activityText: root.currentActivity
              reviewCadenceText: root.reviewCadenceLabel(root.engineState, "architect")
              chat: root.chat
              chatExpanded: root.chatExpanded
              chatPending: root.chatPending
              detailScope: JSON.stringify([root.lastProject, root.projectViewRevision, (root.plan || {}).plan_id || ""])
              spacing: Style.space(8)
              fieldPadding: Style.space(6)
              chatMaxHeight: Style.space(96)
              horizontalPadding: Style.space(18)
              verticalPadding: Style.space(10)
              theme: root.theme
              // The same calls the old Overview buttons made.
              onActionRequested: id => {
                if (id === "approve") root.act("/api/approve")
                else if (id === "run") root.act("/api/run")
                else if (id === "editPlan") root.beginPlanEdit()
                else if (id === "improve") root.revisePlan()
                else if (id === "ask") root.askPlanQuestion()
              }
              onChatExpandedRequested: expanded => root.chatExpanded = expanded
              onStageOpened: index => root.openStageDetail(index)
              // The review strip opens the plan review details, as expanding them in Architecture does.
              onPlanReviewRequested: {
                root.currentTab = "architecture"
                root.planReviewExpanded = true
                root.loadPlanReviewRequests()
              }
              onSavePlanEdit: planEdit.savePlanEdit()
              onCancelPlanEdit: planEdit.cancelPlanEdit()
              onAddEditStage: planEdit.addEditStage()
              onMoveEditStage: (index, direction) => planEdit.moveEditStage(index, direction)
              onDeleteEditStage: index => planEdit.deleteEditStage(index)
              onChangeStageField: (index, field, value) => planEdit.changeStageField(index, field, value)
              onChangeModelConstraint: (index, key, value) => planEdit.changeModelConstraint(index, key, value)
              onEditFocusChanged: field => root.editFocusedField = field
              onHelpRequested: root.helpOpen = true
              onCopyRequested: original => Quickshell.clipboardText = original
              onLeaveRequested: keyHandler.forceActiveFocus()
              onDetailRevealed: control => root.revealDetail(control)
            }

            // ------------------------------------------------ settings
            SettingsView {
              id: settingsView

              anchors.fill: parent
              visible: root.currentTab === "settings"
              engineState: root.engineState
              catalogue: root.catalogue
              quota: root.engineState ? root.engineState.claude_quota : null
              engineOnline: root.engineOnline
              busy: root.busy
              catalogueOpen: catalogueController.catalogueOpen
              reviewerLabel: root.reviewerLabel()
              cadenceLabels: ({
                architect: root.reviewCadenceLabel(root.engineState, "architect"),
                reviewer: root.reviewCadenceLabel(root.engineState, "reviewer")
              })
              updateEnabled: root.guards.update
              changeProjectEnabled: root.guards.changeProject
              detailScope: JSON.stringify([root.lastProject, root.projectViewRevision, (root.plan || {}).plan_id || ""])
              spacing: Style.space(8)
              horizontalPadding: Style.space(18)
              verticalPadding: Style.space(10)
              theme: root.theme
              // The same calls the old settings buttons made.
              onSettingActivated: key => {
                if (key === "planner" || key === "architect" || key === "implementer") root.cycleTool(key)
                else if (key === "reviewer") root.cycleReviewer()
                else if (key === "automatic_routing")
                  root.act("/api/settings", { automatic_routing: !(root.engineState
                    && root.engineState.settings.automatic_routing !== false) })
                else if (key === "architect_review") root.toggleReviewCadence("architect")
                else if (key === "reviewer_review") root.toggleReviewCadence("reviewer")
                else if (key === "auto_push")
                  root.act("/api/settings", { auto_push: !(root.engineState
                    && root.engineState.settings.auto_push) })
                else if (key === "queue_auto_approve")
                  root.act("/api/settings", { queue_auto_approve: !(root.engineState
                    && root.engineState.settings.queue_auto_approve) })
              }
              onActionRequested: id => {
                if (id === "modelSettings") { if (catalogueController.catalogueOpen) catalogueController.catalogueOpen = false; else catalogueController.openCatalogue() }
                else if (id === "refreshModels") root.act("/api/models/refresh", {})
                else if (id === "cancelRefresh") root.act("/api/models/cancel", {})
                else if (id === "refreshLimits") root.act("/api/quota/refresh", {})
                else if (id === "update") root.act("/api/self_update")
                else if (id === "changeProject") root.openChooser()
              }
              onCopyRequested: original => Quickshell.clipboardText = original
              onLeaveRequested: keyHandler.forceActiveFocus()
              onDetailRevealed: control => root.revealDetail(control)
              onDetailInspected: control => root.inspectDetail(control)
            }

            // ------------------------------------------------ activity
            ActivityView {
              id: activityView

              anchors.fill: parent
              visible: root.currentTab === "activity"
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
              detailScope: JSON.stringify([root.lastProject, root.projectViewRevision, (root.plan || {}).plan_id || ""])
              theme: root.theme
              onLiveTabRequested: live => root.liveTab = live
              onHistoryFilterRequested: filter => root.historyFilter = filter
              onReportSelected: key => root.selectedReportKey = key
              onReportExpansionRequested: key => root.expandedReportKey = key
              onLeaveRequested: keyHandler.forceActiveFocus()
              onDetailRevealed: control => root.revealDetail(control)
              onDetailInspected: control => root.inspectDetail(control)
            }

            // ------------------------------------------------ architecture
            ArchitectureView {
              id: architectureView

              anchors.fill: parent
              visible: root.currentTab === "architecture"
              engineState: root.engineState
              plan: root.plan
              architecture: root.architecture
              planReview: root.planReview
              planReviewView: root.planReviewView
              planReviewVersion: root.planReviewVersion
              planReviewScope: root.planReviewScope
              planReviewExpanded: root.planReviewExpanded
              detailScope: JSON.stringify([root.lastProject, root.projectViewRevision, (root.plan || {}).plan_id || ""])
              scrollBarSpace: Style.space(16)
              theme: root.theme
              onPlanReviewExpansionRequested: expanded => root.planReviewExpanded = expanded
              onPlanReviewLoadRequested: root.loadPlanReviewRequests()
              onLeaveRequested: keyHandler.forceActiveFocus()
              onDetailRevealed: control => root.revealDetail(control)
              onDetailInspected: control => root.inspectDetail(control)
            }

            // ------------------------------------------------ feature specs
            FeaturesView {
              id: featuresView

              anchors.fill: parent
              visible: root.currentTab === "features"
              open: root.currentTab === "features"
              embedded: true
              pending: features.featuresPending
              errorText: features.featuresError
              rows: features.featureList
              specs: features.featureSpecs
              activity: features.featureActivity
              selectedSlug: features.selectedFeatureSlug
              detailState: features.featureDetailState
              theme: root.theme
              onCloseRequested: features.closeFeatures()
              onRefreshRequested: features.openFeatures()
              onOpenRequested: feature => features.openFeatureInEditor(feature)
              onCreateRequested: (slug, title) => features.createFeature(slug, title)
              onChatRequested: (feature, message) => features.sendFeatureChat(feature, message)
              onReviewRequested: feature => features.requestFeatureReview(feature)
              onApproveSpecRequested: feature => features.approveFeatureSpec(feature)
              onApproveScenariosRequested: feature => features.approveFeatureScenarios(feature)
              onFeatureSelected: feature => features.selectFeature(feature)
              onLeaveRequested: keyHandler.forceActiveFocus()
            }

            // ------------------------------------------------ queue
            QueueView {
              id: queueView

              anchors.fill: parent
              visible: root.currentTab === "queue"
              queue: root.queue
              engineOnline: root.engineOnline
              startEnabled: root.guards.startQueue
              canMove: root.canMoveQueueGoal
              detailScope: JSON.stringify([root.lastProject, root.projectViewRevision, (root.plan || {}).plan_id || ""])
              spacing: Style.space(8)
              horizontalPadding: Style.space(18)
              verticalPadding: Style.space(10)
              theme: root.theme
              onStartRequested: root.act("/api/queue/start")
              onMoveRequested: (id, dir) => root.act("/api/queue/move", { id: id, dir: dir })
              onRemoveRequested: id => root.act("/api/queue/remove", { id: id })
              onCopyRequested: original => Quickshell.clipboardText = original
              onLeaveRequested: keyHandler.forceActiveFocus()
              onDetailRevealed: control => root.revealDetail(control)
              onDetailInspected: control => root.inspectDetail(control)
            }
          }

          // ------------------------------------------------ overview
          OverviewView {
            id: overviewView

            anchors.fill: tabArea
            visible: root.currentTab === "overview"
            engineState: root.engineState
            plan: root.plan
            phase: root.phase
            busy: root.busy
            agent: root.agent
            agentActive: root.agentActive
            agentElapsedText: root.agentElapsed()
            latestOutputText: root.agentActive ? root.agent.last_line || "" : ""
            liveEntries: liveEntries
            runSummaryText: UsageFormat.overviewSummary(root.engineState, root.plan, root.busy, root.agentNow)
            actions: PanelActions.overviewActions(root.guardFlags, root.guards)
            goalEnhanceStatus: root.goalEnhancePending ? "enhancing the description…"
              : root.goalEnhanceError !== "" ? root.goalEnhanceError
              : root.goalEnhanceReady !== "" ? "AI rewrite ready — press Apply AI description" : ""
            goalEnhanceError: root.goalEnhanceError !== ""
            canApplyEnhancement: root.goalEnhanceReady !== ""
            canUndoEnhancement: root.goalEnhanceUndo !== ""
            discussionStatus: root.discussionPending ? "Forge is replying…" : root.discussionError
            discussionError: root.discussionError !== ""
            discussionCount: root.discussion.length
            diffEnabled: root.guards.diff
            hasReports: root.hasReports
            limitsSummaryText: UsageFormat.quotaLine(root.engineState ? root.engineState.claude_quota : null)
            now: root.agentNow
            selectedStageIndex: root.selectedStageIndex
            spacing: Style.space(8)
            horizontalPadding: Style.space(18)
            verticalPadding: Style.space(10)
            scrollBarSpace: Style.space(16)
            theme: root.theme
            // The same calls the old Overview buttons made.
            onActionRequested: id => {
              if (id === "createPlan") root.act("/api/plan", { goal: goalField.text })
              else if (id === "discuss") root.openDiscussion()
              else if (id === "enhance") root.enhanceGoal()
              else if (id === "applyEnhancement") root.applyGoalEnhancement()
              else if (id === "undoEnhancement") root.undoGoalEnhancement()
              else if (id === "addToQueue") {
                root.act("/api/queue/add", { goal: goalField.text })
                goalField.text = ""
              }
              else if (id === "approve") root.act("/api/approve")
              else if (id === "run") root.act("/api/run")
              else if (id === "editPlan") root.beginPlanEdit()
              else if (id === "stop") root.act("/api/stop")
              else if (id === "diff") root.openDiff()
              else if (id === "reports") {
                root.currentTab = "activity"
                root.liveTab = false
                if (root.hasReports) root.historyFilter = "reports"
              }
            }
            onOpenTab: id => root.currentTab = id
            // A stage row or the needs-attention stage opens its stage detail page.
            onStageRequested: index => root.openStageDetail(index)
            onOverflowRequested: overflowMenu.open = true
            onCopyRequested: original => Quickshell.clipboardText = original
            onHelpRequested: root.helpOpen = true
            onLeaveRequested: keyHandler.forceActiveFocus()
          }
        }

        // Errors stay visible on every tab, just above the hint line.
        Item {
          id: localErrorArea
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.bottom: keyboardHint.top
          anchors.leftMargin: Style.space(16)
          anchors.rightMargin: Style.space(16)
          anchors.bottomMargin: Style.space(10)
          height: localErrorDetail.visible ? localErrorDetail.height : 0

          PanelDetail {
            id: localErrorDetail
            objectName: "localErrorDetail"
            width: parent.width
            visible: root.localError !== ""
            originalText: root.localError
            metadata: "Error"
            error: true
          }
        }

        Text {
          id: keyboardHint
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.bottom: parent.bottom
          anchors.margins: Style.space(16)
          text: PanelNavigation.hintText(root.stageDetailOpen ? "stageDetail"
            : root.discussionOpen ? "discussion" : root.currentTab, root.insertMode)
          elide: Text.ElideRight
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

        OverflowMenu {
          id: overflowMenu
          anchors.top: panelHeader.bottom
          anchors.topMargin: Style.space(4)
          anchors.right: parent.right
          anchors.rightMargin: Style.space(16)
          width: Style.space(220)
          items: PanelActions.overflowItems(root.guardFlags)
          theme: root.theme
          // The same calls the old buttons made.
          onItemChosen: id => {
            if (id === "update") root.act("/api/self_update")
            else if (id === "discard") root.act("/api/reset_plan")
            else if (id === "refactor") root.act("/api/plan", { mode: "refactor", goal: goalField.text })
            else if (id === "diff") root.openDiff()
            else if (id === "changeProject") root.openChooser()
            else if (id === "help") root.helpOpen = true
          }
        }
      }

      // ------------------------------------------- pre-planning discussion
      // A persistent stack page: pushed and popped by open/closeDiscussion, so
      // it keeps its transcript, reading position and draft. It starts hidden
      // so nothing draws over panelPage before the first push; the StackView
      // then shows and sizes it while it is current, and hides it again after
      // pop. That's a literal starting value, not a binding to discussionOpen
      // - the StackView owns visibility (and size) once the page is in play.
      DiscussionView {
        id: discussionView

        visible: false

        entries: root.discussion
        pending: root.discussionPending
        sentMessage: root.discussionSent
        error: root.discussionError
        canSend: root.discussionCanSend
        canPlan: root.discussionCanPlan
        canClear: root.discussionCanClear
        theme: root.theme
        onSendRequested: message => root.sendDiscussionMessage(message)
        onPlanRequested: root.planFromDiscussion()
        onClearRequested: root.clearDiscussion()
        onCloseRequested: root.closeDiscussion()
        onLeaveRequested: keyHandler.forceActiveFocus()
        onHelpRequested: root.helpOpen = true
        onDetailRevealed: control => root.revealDetail(control)
        onDetailInspected: control => root.inspectDetail(control)
      }

      // ------------------------------------------------ stage detail
      // A persistent stack page like the discussion: openStageDetail pushes it and
      // closeStageDetail pops it; the StackView owns its visibility and size.
      StageDetailPage {
        id: stageDetailPage

        visible: false

        stages: root.displayedStages
        stageIndex: root.stageDetailIndex
        subTab: root.stageDetailTab
        stageSnapshot: root.stageSnapshot
        stageReviewBlocks: root.stageReviewBlocks
        stageRoutingExpanded: root.stageRoutingExpanded
        agentNow: root.agentNow
        editingPlan: root.editingPlan
        reviewView: root.reviewView
        stageDetailScope: root.stageDetailScope
        stageActivity: root.stageActivity
        stageReviewIncompleteRange: root.stageReviewIncompleteRange
        ensureStageReviewsLoaded: root.ensureStageReviewsLoaded
        margin: Style.space(16)
        spacing: Style.space(8)
        horizontalPadding: Style.space(18)
        verticalPadding: Style.space(10)
        scrollBarSpace: Style.space(16)
        theme: root.theme
        onBackRequested: root.closeStageDetail()
        onStageStepRequested: step => root.openStageDetail(PanelNavigation.stepStageIndex(
          root.displayedStages.length, root.stageDetailIndex, step))
        onSubTabRequested: id => root.stageDetailTab = id
        onLoadStageReviews: (stageId, cursor, end) => root.loadStageReviews(stageId, cursor, end)
        onRetryStageReviews: stage => root.retryStageReviews(stage)
        onStageRoutingExpandedRequested: expanded => root.stageRoutingExpanded = expanded
        onDiffRequested: if (root.guards.diff) root.openDiff()
        onLiveOutputRequested: {
          root.closeStageDetail()
          root.currentTab = "activity"
          root.liveTab = true
        }
        onCopyRequested: original => Quickshell.clipboardText = original
        onLeaveRequested: keyHandler.forceActiveFocus()
        onDetailRevealed: control => root.revealDetail(control)
        onDetailInspected: control => root.inspectDetail(control)
      }

      // ------------------------------------------- model policy and options
      CatalogueEditor {
        id: catalogueEditorView

        anchors.fill: parent
        open: catalogueController.catalogueOpen
        engineOnline: root.engineOnline
        engineBusy: !!root.engineState && (root.engineState.busy || root.engineState.queue_active)
        aiPending: catalogueController.catalogueAiPending
        aiReady: catalogueController.catalogueAiReady
        aiUndo: catalogueController.catalogueAiUndo
        aiMessage: catalogueController.catalogueAiMessage
        catalogue: root.catalogue
        details: catalogueController.catalogueDetails
        draft: catalogueController.catalogueDraft
        detailScope: JSON.stringify([root.lastProject, root.projectViewRevision, (root.plan || {}).plan_id || ""])
        theme: root.theme
        onCloseRequested: catalogueController.catalogueOpen = false
        onSuggestRequested: catalogueController.suggestCatalogue()
        onApplyRequested: catalogueController.applyCatalogueSuggestion()
        onUndoRequested: catalogueController.undoCatalogueSuggestion()
        onReloadRequested: catalogueController.reloadCatalogue()
        onSaveRequested: catalogueController.saveCatalogue()
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
        pending: diffController.diffPending
        diffText: diffController.diffText
        errorText: diffController.diffError
        theme: root.theme
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
        manualEntry: chooserController.manualEntry
        projectsData: chooserController.projectsData
        theme: root.theme
        onCloseRequested: root.chooserOpen = false
        onRowChosen: row => chooserController.chooseRow(row)
        onManualPathRequested: path => {
          root.act("/api/project", {path: path})
          root.chooserOpen = false
        }
        onHelpRequested: root.helpOpen = true
        onLeaveRequested: keyHandler.forceActiveFocus()
        onDetailInspected: control => root.inspectDetail(control)
      }

      // ------------------------------------------------ keyboard help
      KeyboardHelp {
        anchors.fill: parent
        z: 10
        visible: root.helpOpen
        model: PanelNavigation.helpRows()
        spacing: Style.space(8)
        theme: root.theme
        titleFontSize: root.fs(16)
        onCloseRequested: root.helpOpen = false
      }
    }
  }
}
