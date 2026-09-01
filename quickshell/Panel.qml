pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import qs.Commons
import qs.Ui

Item {
  id: root

  property var shell: null
  property var manifest: null
  property bool closingFromHost: false
  property int selectedSection: 0
  property int selectedTaskIndex: 0
  property string focusArea: "sections"
  property bool editingDraft: false
  property bool editingPlanTask: false
  property bool rejectingPlan: false
  property bool confirmingImplementationCancel: false
  property string implementationInterventionMode: ""
  property string localDraftError: ""
  property var repositoryPathCandidates: []
  property string draftStep: "repository"
  property int repositorySourceIndex: 0
  property int selectedRepositoryIndex: 0
  property string selectedRepositoryPath: ""
  property string selectedRepositoryLabel: ""
  property int draftSessionGeneration: 0
  property int cloneDraftSession: -1

  readonly property real panelDesignHeight: window.height
    / Math.max(0.01, Style.spacing.scale)
  readonly property real panelDesignWidth: window.width
    / Math.max(0.01, Style.spacing.scale)
  readonly property bool compactDraftLayout: root.editingDraft
    && root.panelDesignHeight < 620
  readonly property bool compactWidthLayout: root.panelDesignWidth < 800

  readonly property string pluginId: manifest && manifest.id
    ? manifest.id
    : "dev.omarchy-ai-build-orchestrator"
  readonly property color foreground: Color.foreground
  readonly property color mutedForeground: Qt.darker(Color.foreground, 1.45)
  readonly property color background: Color.background
  readonly property color surface: Color.popups.background
  readonly property color accent: Color.accent
  readonly property color urgent: Color.urgent
  readonly property string fontFamily: Style.font.family

  function open(payloadJson) {
    closingFromHost = false
    focusArea = "sections"
    window.visible = true
    engine.reconnect()
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function close() {
    closingFromHost = true
    window.visible = false
    closingFromHost = false
  }

  function requestClose() {
    if (implementationInterventionMode !== "") {
      cancelImplementationIntervention()
      return
    }
    if (confirmingImplementationCancel) {
      confirmingImplementationCancel = false
      return
    }
    if (editingDraft) {
      cancelDraftEntry()
      return
    }
    if (editingPlanTask || rejectingPlan) {
      cancelPlanInput()
      return
    }
    if (shell && typeof shell.hide === "function") shell.hide(pluginId)
    else window.visible = false
  }

  function moveSection(delta) {
    if (editingDraft || editingPlanTask || rejectingPlan) return
    var count = sectionModel.count
    selectedSection = ((selectedSection + delta) % count + count) % count
    selectedTaskIndex = 0
    sectionList.positionViewAtIndex(selectedSection, ListView.Contain)
  }

  function moveNavigation(dx, dy) {
    if (confirmingImplementationCancel || implementationInterventionMode !== "") return
    if (editingDraft && draftStep === "repository") {
      if (dx !== 0) {
        repositorySourceIndex = Math.max(0, Math.min(1, repositorySourceIndex + dx))
        selectedRepositoryIndex = 0
      } else if (dy !== 0) {
        var repositories = currentRepositoryList()
        selectedRepositoryIndex = Math.max(
          0,
          Math.min(Math.max(0, repositories.length - 1), selectedRepositoryIndex + dy)
        )
        currentRepositoryView().positionViewAtIndex(selectedRepositoryIndex, ListView.Contain)
      }
      return
    }
    if (editingDraft || editingPlanTask || rejectingPlan) return

    if (dx !== 0) {
      if (dx > 0 && focusArea === "sections") focusArea = "content"
      else if (dx < 0 && focusArea === "content") focusArea = "sections"
      return
    }

    if (dy === 0) return
    if (focusArea === "sections") {
      moveSection(dy)
      return
    }

    var plan = currentPlan()
    if (selectedSection === 1 && plan && plan.tasks && plan.tasks.length > 0) {
      selectedTaskIndex = Math.max(0, Math.min(plan.tasks.length - 1, selectedTaskIndex + dy))
      planTaskList.positionViewAtIndex(selectedTaskIndex, ListView.Contain)
    }
  }

  function activateNavigation() {
    if (confirmingImplementationCancel) {
      confirmImplementationCancel()
      return
    }
    if (editingDraft && draftStep === "repository") {
      activateRepository()
      return
    }
    if (focusArea === "sections") {
      focusArea = "content"
      return
    }
    if (selectedSection === 0 && !editingDraft) beginDraftEntry()
    else if (selectedSection === 1) beginPlanTaskEdit()
  }

  function beginDraftEntry() {
    if (!engine.connected || engine.requestPending
        || (engine.activeRun && engine.activeRun.run_status === "running")) return
    selectedSection = 0
    focusArea = "content"
    editingDraft = true
    draftSessionGeneration++
    cloneDraftSession = -1
    draftStep = "repository"
    repositorySourceIndex = 0
    selectedRepositoryIndex = 0
    selectedRepositoryPath = ""
    selectedRepositoryLabel = ""
    localDraftError = ""
    repositoryPathCandidates = []
    engine.requestError = ""
    repositoryEngine.requestError = ""
    repositoryEngine.listRepositories()
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function cancelDraftEntry() {
    if (engine.requestPending) return
    if (repositoryEngine.requestPending) repositoryEngine.abandonRequest()
    editingDraft = false
    localDraftError = ""
    repositoryPathCandidates = []
    draftStep = "repository"
    selectedRepositoryPath = ""
    selectedRepositoryLabel = ""
    cloneDraftSession = -1
    repositoryField.focus = false
    repositorySearchField.focus = false
    repositorySearchField.text = ""
    goalField.focus = false
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function submitDraft() {
    var repository = selectedRepositoryPath || repositoryField.text.trim()
    var goal = goalField.text.trim()
    if (repository === "") {
      localDraftError = "Enter an absolute repository path"
      repositoryField.forceActiveFocus()
      return
    }
    if (goal === "") {
      localDraftError = "Describe the engineering goal"
      goalField.forceActiveFocus()
      return
    }

    localDraftError = ""
    engine.createDraft(repository, goal)
  }

  function repositoryMatches(repository) {
    var query = repositorySearchField.text.trim().toLowerCase()
    if (query === "") return true
    return String(repository.name || "").toLowerCase().indexOf(query) !== -1
      || String(repository.name_with_owner || "").toLowerCase().indexOf(query) !== -1
      || String(repository.path || "").toLowerCase().indexOf(query) !== -1
  }

  function filteredLocalRepositories() {
    var result = []
    var repositories = repositoryEngine.repositoryCatalog.local || []
    for (var i = 0; i < repositories.length; i++)
      if (repositoryMatches(repositories[i])) result.push(repositories[i])
    return result
  }

  function filteredGithubRepositories() {
    var result = []
    var repositories = repositoryEngine.repositoryCatalog.github || []
    for (var i = 0; i < repositories.length; i++)
      if (repositoryMatches(repositories[i])) result.push(repositories[i])
    return result
  }

  function currentRepositoryList() {
    return repositorySourceIndex === 0
      ? filteredLocalRepositories()
      : filteredGithubRepositories()
  }

  function currentRepositoryView() {
    return repositorySourceIndex === 0 ? localRepositoryList : githubRepositoryList
  }

  function activateRepository() {
    var repositories = currentRepositoryList()
    if (repositories.length === 0 || repositoryEngine.requestPending) return
    selectedRepositoryIndex = Math.max(0, Math.min(repositories.length - 1, selectedRepositoryIndex))
    var repository = repositories[selectedRepositoryIndex]
    localDraftError = ""
    repositoryEngine.requestError = ""
    if (repositorySourceIndex === 0) {
      selectedRepositoryPath = String(repository.path || "")
      selectedRepositoryLabel = String(repository.name_with_owner || repository.name || "")
      draftStep = "goal"
      Qt.callLater(function() { goalField.forceActiveFocus() })
    } else {
      cloneDraftSession = draftSessionGeneration
      if (!repositoryEngine.cloneRepository(repository.name_with_owner))
        cloneDraftSession = -1
    }
  }

  function beginManualRepositoryEntry() {
    if (repositoryEngine.requestPending) return
    draftStep = "path"
    localDraftError = ""
    repositoryEngine.requestError = ""
    Qt.callLater(function() { repositoryField.forceActiveFocus() })
  }

  function acceptManualRepositoryPath() {
    var path = repositoryField.text.trim()
    if (path === "") {
      localDraftError = "Enter an absolute repository path"
      return
    }
    selectedRepositoryPath = path
    selectedRepositoryLabel = path
    draftStep = "goal"
    Qt.callLater(function() { goalField.forceActiveFocus() })
  }

  function returnToRepositoryBrowser() {
    if (engine.requestPending) return
    draftStep = "repository"
    localDraftError = ""
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function beginRepositorySearch() {
    if (draftStep !== "repository") return
    Qt.callLater(function() { repositorySearchField.forceActiveFocus() })
  }

  function completeRepositoryPath() {
    if (!engine.connected || engine.requestPending) return
    localDraftError = ""
    repositoryPathCandidates = []
    engine.completeRepositoryPath(repositoryField.text)
  }

  function repositoryCandidateText() {
    var labels = []
    var count = Math.min(repositoryPathCandidates.length, 6)
    for (var i = 0; i < count; i++) {
      var candidate = String(repositoryPathCandidates[i])
      var withoutSlash = candidate.endsWith("/")
        ? candidate.slice(0, candidate.length - 1)
        : candidate
      labels.push(withoutSlash.slice(withoutSlash.lastIndexOf("/") + 1) + "/")
    }
    if (repositoryPathCandidates.length > count)
      labels.push("… " + (repositoryPathCandidates.length - count) + " more")
    return "Matches:  " + labels.join("    ")
  }

  function draftError() {
    return localDraftError || repositoryEngine.requestError || engine.requestError
  }

  function currentPlan() {
    return engine.activeRun && engine.activeRun.plan ? engine.activeRun.plan : null
  }

  function selectedTask() {
    var plan = currentPlan()
    if (!plan || !plan.tasks || plan.tasks.length === 0) return null
    selectedTaskIndex = Math.max(0, Math.min(plan.tasks.length - 1, selectedTaskIndex))
    return plan.tasks[selectedTaskIndex]
  }

  function generatePlan(agent) {
    if (engine.requestPending || !engine.activeRun) return
    engine.requestError = ""
    engine.generatePlan(agent)
  }

  function beginPlanTaskEdit() {
    var task = selectedTask()
    var plan = currentPlan()
    if (!task || !plan || plan.status !== "proposed" || engine.requestPending) return
    editingPlanTask = true
    taskTitleField.text = task.title || ""
    taskDescriptionField.text = task.description || ""
    taskCriteriaField.text = (task.acceptance_criteria || []).join(" || ")
    engine.requestError = ""
    Qt.callLater(function() { taskTitleField.forceActiveFocus() })
  }

  function savePlanTask() {
    var task = selectedTask()
    if (!task) return
    var parts = taskCriteriaField.text.split("||")
    var criteria = []
    for (var i = 0; i < parts.length; i++) {
      var criterion = parts[i].trim()
      if (criterion !== "") criteria.push(criterion)
    }
    engine.updatePlanTask(
      task.id,
      taskTitleField.text.trim(),
      taskDescriptionField.text.trim(),
      criteria
    )
  }

  function cancelPlanInput() {
    if (engine.requestPending) return
    editingPlanTask = false
    rejectingPlan = false
    taskTitleField.focus = false
    taskDescriptionField.focus = false
    taskCriteriaField.focus = false
    rejectionReasonField.focus = false
    engine.requestError = ""
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function beginRejectPlan() {
    var plan = currentPlan()
    if (!plan || plan.status !== "proposed" || engine.requestPending) return
    rejectingPlan = true
    rejectionReasonField.text = ""
    Qt.callLater(function() { rejectionReasonField.forceActiveFocus() })
  }

  function submitPlanRejection() {
    engine.rejectPlan(rejectionReasonField.text.trim())
  }

  function moveCurrentTask(direction) {
    var task = selectedTask()
    var plan = currentPlan()
    if (!task || !plan || plan.status !== "proposed" || engine.requestPending) return
    engine.movePlanTask(task.id, direction)
  }

  function latestImplementationAttempt() {
    var attempts = engine.activeRun && engine.activeRun.implementation_attempts
      ? engine.activeRun.implementation_attempts : []
    return attempts.length > 0 ? attempts[attempts.length - 1] : null
  }

  function finishContext() {
    var run = engine.activeRun
    var plan = currentPlan()
    var task = selectedTask()
    if (!run || !plan || plan.status !== "approved" || !task) return null
    var worktree = null
    for (var i = (run.worktrees || []).length - 1; i >= 0; i--)
      if (run.worktrees[i].task_id === task.id && run.worktrees[i].status === "ready") {
        worktree = run.worktrees[i]; break
      }
    if (!worktree) return null
    var implementation = null
    for (var j = (run.implementation_attempts || []).length - 1; j >= 0; j--)
      if (run.implementation_attempts[j].task_id === task.id
          && run.implementation_attempts[j].worktree_id === worktree.id
          && run.implementation_attempts[j].status === "completed") {
        implementation = run.implementation_attempts[j]; break
      }
    return implementation ? ({ plan: plan, task: task, worktree: worktree, implementation: implementation }) : null
  }

  function latestTaskRecord(records) {
    var context = finishContext()
    if (!context || !records) return null
    for (var i = records.length - 1; i >= 0; i--)
      if (records[i].task_id === context.task.id) return records[i]
    return null
  }

  function finishSelectedTask() {
    var context = finishContext()
    if (!context || engine.requestPending) return
    engine.finishTask(context.plan.id, context.task.id, context.worktree.id, context.implementation.id)
  }

  function runningImplementationAttempt() {
    var attempt = latestImplementationAttempt()
    return attempt && attempt.status === "running" ? attempt : null
  }

  function pendingContinuationAttempt() {
    var attempts = engine.activeRun && engine.activeRun.implementation_attempts
      ? engine.activeRun.implementation_attempts : []
    for (var i = attempts.length - 1; i >= 0; i--) {
      if (attempts[i].pending_continuation_kind
          && attempts[i].pending_user_instruction) return attempts[i]
    }
    return null
  }

  function implementationTaskTitle(attempt) {
    var plan = root.currentPlan()
    if (!attempt || !plan || !plan.tasks) return "approved task"
    for (var i = 0; i < plan.tasks.length; i++) {
      if (plan.tasks[i].id === attempt.task_id)
        return plan.tasks[i].position + ". " + plan.tasks[i].title
    }
    return "approved task"
  }

  function latestImplementationActivity() {
    var attempt = latestImplementationAttempt()
    var activity = engine.activeRun && engine.activeRun.implementation_activity
      ? engine.activeRun.implementation_activity : []
    if (!attempt) return []
    var result = []
    for (var i = 0; i < activity.length; i++)
      if (activity[i].attempt_id === attempt.id) result.push(activity[i])
    return result
  }

  function beginImplementationCancel() {
    if (!runningImplementationAttempt() || controlEngine.requestPending
        || continuationEngine.requestPending
        || implementationInterventionMode !== "") return
    confirmingImplementationCancel = true
  }

  function confirmImplementationCancel() {
    var attempt = runningImplementationAttempt()
    if (!attempt || !engine.activeRun || controlEngine.requestPending) return
    controlEngine.cancelImplementation(engine.activeRun.id, attempt.id)
  }

  function toggleImplementationPause() {
    var attempt = runningImplementationAttempt()
    if (!attempt || !engine.activeRun || controlEngine.requestPending
        || continuationEngine.requestPending || confirmingImplementationCancel
        || implementationInterventionMode !== "") return
    if (attempt.paused)
      controlEngine.resumeImplementation(engine.activeRun.id, attempt.id)
    else
      controlEngine.pauseImplementation(engine.activeRun.id, attempt.id)
  }

  function beginImplementationIntervention(mode) {
    if (!runningImplementationAttempt() || continuationEngine.requestPending
        || controlEngine.requestPending || confirmingImplementationCancel) return
    implementationInterventionMode = mode
    implementationInstructionField.text = ""
    Qt.callLater(function() { implementationInstructionField.forceActiveFocus() })
  }

  function cancelImplementationIntervention() {
    implementationInterventionMode = ""
    implementationInstructionField.text = ""
    implementationInstructionField.focus = false
    continuationEngine.requestError = ""
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function submitImplementationIntervention() {
    var attempt = runningImplementationAttempt()
    var instruction = implementationInstructionField.text.trim()
    if (!attempt || !engine.activeRun || instruction === ""
        || continuationEngine.requestPending) return
    var kind = implementationInterventionMode === "redirect"
      ? "redirect" : "additional_context"
    if (continuationEngine.continueImplementation(
          engine.activeRun.id, attempt.id, kind, instruction)) {
      implementationInterventionMode = ""
      implementationInstructionField.focus = false
      Qt.callLater(function() { keyCatcher.forceActiveFocus() })
    }
  }

  function retryPendingContinuation() {
    var attempt = pendingContinuationAttempt()
    if (!attempt || !engine.activeRun || continuationEngine.requestPending) return
    continuationEngine.continueImplementation(
      engine.activeRun.id,
      attempt.id,
      attempt.pending_continuation_kind,
      attempt.pending_user_instruction
    )
  }

  function footerHelp() {
    if (editingDraft && draftStep === "repository")
      return "h/l or ←/→  Source    j/k or ↑/↓  Repository    Enter  Open or clone    /  Search    p  Path    Esc  Cancel"
    if (editingDraft && draftStep === "path")
      return "Tab  Complete path    Enter  Continue    Esc  Repository browser"
    if (editingDraft) return "Enter  Create draft    Shift+Tab  Repositories    Esc  Cancel"
    if (editingPlanTask) return "Tab  Next field    Enter  Continue or save    Esc  Cancel"
    if (rejectingPlan) return "Enter  Reject plan    Esc  Cancel"
    if (confirmingImplementationCancel)
      return "Enter  Confirm cancellation    Esc  Keep running"
    if (implementationInterventionMode !== "")
      return "Enter  Submit instruction    Esc  Keep current attempt"
    if (focusArea === "sections")
      return "j/k or ↑/↓  Sections    l/→ or Enter  Open    r  Reconnect    Esc  Close"
    if (selectedSection === 1 && currentPlan() && currentPlan().status === "proposed")
      return "h/←  Sections    j/k or ↑/↓  Tasks    Enter/e  Edit    J/K  Reorder    a  Approve    x  Reject"
    if (selectedSection === 1)
      return "h/←  Sections    c  Plan with Codex    d  Plan with Claude    Esc  Close"
    if (selectedSection === 0 && runningImplementationAttempt())
      return "h/←  Sections    p  Pause/resume    i  Redirect    a  Add context    x  Cancel"
    if (selectedSection === 0 && engine.activeRun)
      return "h/←  Sections    Enter/n  New draft    r  Reconnect    Esc  Close"
    if (selectedSection === 0)
      return "h/←  Sections    Enter  Create draft    r  Reconnect    Esc  Close"
    return "h/←  Sections    r  Reconnect    Esc  Close"
  }

  function statusLabel() {
    if (!engine.connected) return "Engine offline"
    return engine.engineStatus.replace(/_/g, " ")
  }

  function statusColor() {
    if (!engine.connected) return mutedForeground
    if (engine.engineStatus === "failed" || engine.engineStatus === "blocked") return urgent
    if (engine.engineStatus === "waiting_for_user") return urgent
    if (engine.engineStatus === "running") return accent
    return foreground
  }

  EngineConnection {
    id: engine
    onDraftCreated: {
      root.editingDraft = false
      root.draftStep = "repository"
      root.repositoryPathCandidates = []
      root.selectedRepositoryPath = ""
      root.selectedRepositoryLabel = ""
      repositoryField.text = ""
      repositorySearchField.text = ""
      goalField.text = ""
      Qt.callLater(function() { keyCatcher.forceActiveFocus() })
    }
    onRepositoryPathCompleted: function(replacement, candidates) {
      repositoryField.text = replacement
      repositoryField.cursorPosition = repositoryField.text.length
      root.repositoryPathCandidates = candidates || []
      Qt.callLater(function() { repositoryField.forceActiveFocus() })
    }
    onRequestCompleted: function(method) {
      if (method === "complete_repository_path") return
      if (method === "update_plan_task") root.editingPlanTask = false
      if (method === "reject_plan") root.rejectingPlan = false
      root.selectedTaskIndex = Math.max(0, root.selectedTaskIndex)
      Qt.callLater(function() { keyCatcher.forceActiveFocus() })
    }
    onSnapshotChanged: {
      var plan = root.currentPlan()
      if (plan && plan.tasks)
        root.selectedTaskIndex = Math.max(0, Math.min(plan.tasks.length - 1, root.selectedTaskIndex))
      else
        root.selectedTaskIndex = 0
      if (!root.runningImplementationAttempt())
        root.confirmingImplementationCancel = false
      if (!root.runningImplementationAttempt() && !continuationEngine.requestPending)
        root.implementationInterventionMode = ""
      var latestAttempt = root.latestImplementationAttempt()
      if (continuationEngine.requestPending && latestAttempt
          && latestAttempt.status === "running" && latestAttempt.parent_attempt_id) {
        continuationEngine.abandonRequest()
        root.implementationInterventionMode = ""
      }
    }
  }

  EngineConnection {
    id: controlEngine
    onRequestCompleted: function(method) {
      if (method === "cancel_task_implementation")
        root.confirmingImplementationCancel = false
      Qt.callLater(function() { keyCatcher.forceActiveFocus() })
    }
  }


  EngineConnection {
    id: continuationEngine
    onRequestCompleted: function(method) {
      if (method === "continue_task_implementation")
        root.implementationInterventionMode = ""
      Qt.callLater(function() { keyCatcher.forceActiveFocus() })
    }
  }

  EngineConnection {
    id: repositoryEngine
    onRepositoryCloned: function(nameWithOwner, path) {
      if (!root.editingDraft || root.cloneDraftSession !== root.draftSessionGeneration) return
      root.cloneDraftSession = -1
      root.selectedRepositoryPath = path
      root.selectedRepositoryLabel = nameWithOwner
      root.draftStep = "goal"
      Qt.callLater(function() { goalField.forceActiveFocus() })
    }
    onRepositoryCatalogChanged: {
      root.selectedRepositoryIndex = 0
      if (root.editingDraft && root.draftStep === "repository"
          && !repositorySearchField.activeFocus)
        Qt.callLater(function() { keyCatcher.forceActiveFocus() })
    }
  }

  ListModel {
    id: sectionModel
    ListElement {
      title: "Overview"
      description: "Current run, ownership, queue, and the next required decision."
    }
    ListElement {
      title: "Plan"
      description: "Generate, inspect, revise, reorder, approve, or reject the explicit task plan."
    }
    ListElement {
      title: "Changes"
      description: "Changed files, worktrees, diffs, and proposed semantic commits will appear here."
    }
    ListElement {
      title: "Verification"
      description: "Deterministic build, test, format, lint, and analyzer results will appear here."
    }
    ListElement {
      title: "Review"
      description: "Independent review findings and correction loops will appear here."
    }
  }

  FloatingWindow {
    id: window
    visible: false
    title: "Omarchy AI Build Orchestrator — working title"
    color: root.background
    implicitWidth: 880
    implicitHeight: 640
    minimumSize: Qt.size(640, 480)

    onVisibleChanged: {
      if (!visible && !root.closingFromHost && root.shell && typeof root.shell.hide === "function")
        root.shell.hide(root.pluginId)
    }

    Rectangle {
      anchors.fill: parent
      color: root.background

      PanelKeyCatcher {
        id: keyCatcher
        anchors.fill: parent
        blocked: repositoryField.activeFocus || goalField.activeFocus
          || repositorySearchField.activeFocus
          || taskTitleField.activeFocus || taskDescriptionField.activeFocus
          || taskCriteriaField.activeFocus || rejectionReasonField.activeFocus
          || implementationInstructionField.activeFocus
        onMoveRequested: function(dx, dy) {
          root.moveNavigation(dx, dy)
        }
        onActivateRequested: root.activateNavigation()
        onCloseRequested: root.requestClose()
        onTextKey: function(text) {
          if (root.editingDraft && root.draftStep === "repository") {
            if (text === "/") root.beginRepositorySearch()
            else if (text === "p") root.beginManualRepositoryEntry()
            else if (text === "r") repositoryEngine.listRepositories()
            return
          }
          if (text === "r") engine.reconnect()
          else if (text === "n") root.beginDraftEntry()
          else if (root.selectedSection === 0 && text === "x") root.beginImplementationCancel()
          else if (root.selectedSection === 0 && text === "p") root.toggleImplementationPause()
          else if (root.selectedSection === 0 && text === "i") root.beginImplementationIntervention("redirect")
          else if (root.selectedSection === 0 && text === "a") root.beginImplementationIntervention("additional_context")
          else if (root.selectedSection === 1 && text === "c") root.generatePlan("codex")
          else if (root.selectedSection === 1 && text === "d") root.generatePlan("claude")
          else if (root.selectedSection === 1 && text === "e") root.beginPlanTaskEdit()
          else if (root.selectedSection === 1 && text === "J") root.moveCurrentTask("down")
          else if (root.selectedSection === 1 && text === "K") root.moveCurrentTask("up")
          else if (root.selectedSection === 1 && text === "a" && root.currentPlan() && root.currentPlan().status === "proposed") engine.approvePlan()
          else if (root.selectedSection === 1 && text === "x") root.beginRejectPlan()
        }

        Item {
          anchors.fill: parent
          anchors.margins: Style.space(20)

          Row {
            id: headerRow
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: parent.top
            spacing: Style.space(12)

            Column {
              width: parent.width - statusPill.width - parent.spacing
              spacing: Style.space(4)

              Text {
                text: "Software workshop"
                color: root.foreground
                font.family: root.fontFamily
                font.pixelSize: Style.font.title
                font.bold: true
              }

              Text {
                visible: !root.compactDraftLayout
                text: "Omarchy AI Build Orchestrator is a temporary working title."
                color: root.mutedForeground
                font.family: root.fontFamily
                font.pixelSize: Style.font.bodySmall
              }
            }

            Rectangle {
              id: statusPill
              width: statusText.implicitWidth + Style.space(20)
              height: statusText.implicitHeight + Style.space(10)
              radius: height / 2
              color: Qt.rgba(root.statusColor().r, root.statusColor().g, root.statusColor().b, 0.14)
              border.width: 1
              border.color: root.statusColor()

              Text {
                id: statusText
                anchors.centerIn: parent
                text: root.statusLabel()
                color: root.statusColor()
                font.family: root.fontFamily
                font.pixelSize: Style.font.bodySmall
                font.bold: true
              }
            }
          }

          Rectangle {
            id: headerDivider
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: headerRow.bottom
            anchors.topMargin: Style.space(18)
            height: 1
            color: Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.18)
          }

          Row {
            id: workspaceRow
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.top: headerDivider.bottom
            anchors.topMargin: Style.space(18)
            anchors.bottom: footerText.top
            anchors.bottomMargin: Style.space(18)
            spacing: Style.space(18)

            ListView {
              id: sectionList
              width: root.compactWidthLayout
                ? Math.min(Style.space(150), parent.width * 0.26)
                : Math.min(Style.space(220), parent.width * 0.32)
              height: parent.height
              model: sectionModel
              interactive: contentHeight > height
              clip: true
              spacing: Style.space(6)

              delegate: Rectangle {
                id: sectionDelegate
                required property int index
                required property string title
                width: ListView.view.width
                height: Style.space(42)
                radius: Style.cornerRadius
                color: index === root.selectedSection
                  ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b,
                            root.focusArea === "sections" ? 0.18 : 0.08)
                  : "transparent"
                border.width: index === root.selectedSection && root.focusArea === "sections" ? 1 : 0
                border.color: root.accent

                Text {
                  anchors.left: parent.left
                  anchors.leftMargin: Style.space(12)
                  anchors.verticalCenter: parent.verticalCenter
                  text: sectionDelegate.title
                  color: root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: Style.font.body
                  font.bold: sectionDelegate.index === root.selectedSection
                }

                MouseArea {
                  anchors.fill: parent
                  onClicked: {
                    root.selectedSection = sectionDelegate.index
                    root.selectedTaskIndex = 0
                    root.focusArea = "sections"
                  }
                  onDoubleClicked: {
                    root.selectedSection = sectionDelegate.index
                    root.selectedTaskIndex = 0
                    root.focusArea = "content"
                  }
                }
              }
            }

            Rectangle {
              width: 1
              height: parent.height
              color: Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.14)
            }

            Column {
              id: contentPane
              width: parent.width - sectionList.width - Style.space(19)
              height: parent.height
              spacing: Style.space(14)

              Text {
                id: contentTitle
                visible: !root.compactDraftLayout
                text: sectionModel.get(root.selectedSection).title
                color: root.foreground
                font.family: root.fontFamily
                font.pixelSize: Style.font.title
                font.bold: true
              }

              Text {
                id: contentDescription
                visible: !root.compactDraftLayout
                width: parent.width
                text: sectionModel.get(root.selectedSection).description
                color: root.mutedForeground
                font.family: root.fontFamily
                font.pixelSize: Style.font.body
                wrapMode: Text.WordWrap
              }

              Rectangle {
                id: contentSurface
                width: parent.width
                height: root.compactDraftLayout
                  ? parent.height
                  : Math.max(0, parent.height - contentTitle.height
                    - contentDescription.height - parent.spacing * 2)
                radius: Style.cornerRadius
                color: root.surface
                clip: true
                border.width: root.focusArea === "content" ? 2 : 1
                border.color: root.focusArea === "content"
                  ? root.accent
                  : Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.16)

                Column {
                  id: contentBody
                  anchors.fill: parent
                  anchors.margins: Style.space(16)
                  spacing: Style.space(10)

                  Text {
                    id: contentHeading
                    visible: !root.compactDraftLayout
                    text: {
                      if (root.selectedSection === 1) {
                        var plan = root.currentPlan()
                        if (!engine.activeRun) return "Create a draft first"
                        if (engine.activeRun.run_status === "planning") return "Planner is inspecting the repository"
                        if (engine.activeRun.run_status === "failed") return "Planning failed"
                        if (!plan || plan.status === "rejected") return "Choose a planning agent"
                        if (plan.status === "approved") return "Plan approved"
                        return "Plan revision " + plan.revision
                      }
                      if (root.selectedSection !== 0) return "Planned capability"
                      if (!engine.connected) return "Start the Rust engine to connect"
                      if (root.editingDraft) return "Create a durable draft run"
                      if (engine.activeRun) {
                        var implementation = root.latestImplementationAttempt()
                        if (implementation && implementation.status === "running")
                          return implementation.agent + (implementation.paused
                            ? " implementation paused" : " is implementing")
                        if (implementation)
                          return "Implementation " + implementation.status
                        return "Draft saved"
                      }
                      return "No active run"
                    }
                    color: root.foreground
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.body
                    font.bold: true
                  }

                  Text {
                    visible: root.selectedSection > 1
                    width: parent.width
                    text: sectionModel.get(root.selectedSection).description
                    color: root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.bodySmall
                    wrapMode: Text.WordWrap
                  }

                  Text {
                    visible: root.selectedSection === 0 && !engine.connected
                    width: parent.width
                    text: "The panel remains safe while the engine is unavailable and will reconnect automatically."
                    color: root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.bodySmall
                    wrapMode: Text.WordWrap
                  }

                  Column {
                    visible: root.selectedSection === 0 && engine.connected && !root.editingDraft && !engine.activeRun
                    width: parent.width
                    spacing: Style.space(12)

                    Text {
                      width: parent.width
                      text: "Open a local Git repository and preserve an engineering goal. The engine will record its canonical path, current revision, branch, and working-tree condition without modifying it."
                      color: root.mutedForeground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      wrapMode: Text.WordWrap
                    }

                    Button {
                      text: "Create draft"
                      bordered: true
                      foreground: root.foreground
                      accent: root.accent
                      onClicked: root.beginDraftEntry()
                    }
                  }

                  Column {
                    id: draftEditor
                    visible: root.selectedSection === 0 && root.editingDraft
                    width: parent.width
                    height: root.compactDraftLayout
                      ? Math.max(0, parent.height - (draftErrorText.visible
                        ? draftErrorText.height + parent.spacing : 0))
                      : implicitHeight
                    spacing: Style.space(8)

                    Text {
                      id: draftStepTitle
                      text: root.draftStep === "goal" ? "Engineering goal" : "Choose a repository"
                      color: root.foreground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      font.bold: true
                    }

                    Column {
                      id: repositoryBrowser
                      visible: root.draftStep === "repository"
                      width: parent.width
                      height: root.compactDraftLayout
                        ? Math.max(0, parent.height - draftStepTitle.height - parent.spacing)
                        : implicitHeight
                      spacing: Style.space(7)

                      property real fixedContentHeight: repositorySearchField.height
                        + projectsRootText.height
                        + (repositoryStatus.visible ? repositoryStatus.height : 0)
                        + (localDiscoveryWarning.visible ? localDiscoveryWarning.height : 0)
                        + (githubDiscoveryWarning.visible ? githubDiscoveryWarning.height : 0)
                        + spacing * (2
                          + (repositoryStatus.visible ? 1 : 0)
                          + (localDiscoveryWarning.visible ? 1 : 0)
                          + (githubDiscoveryWarning.visible ? 1 : 0))

                      TextField {
                        id: repositorySearchField
                        width: parent.width
                        enabled: true
                        placeholderText: root.compactWidthLayout
                          ? "Search repositories  (/ to focus)"
                          : "Search local and GitHub repositories  (/ to focus)"
                        foreground: root.foreground
                        accent: root.accent
                        onTextEdited: root.selectedRepositoryIndex = 0

                        Keys.onPressed: function(event) {
                          if (event.key === Qt.Key_Escape) {
                            repositorySearchField.focus = false
                            keyCatcher.forceActiveFocus()
                            event.accepted = true
                          } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                            repositorySearchField.focus = false
                            keyCatcher.forceActiveFocus()
                            root.activateRepository()
                            event.accepted = true
                          }
                        }
                      }

                      Text {
                        id: projectsRootText
                        width: parent.width
                        text: "Projects root: " + (repositoryEngine.repositoryCatalog.project_roots || []).join(", ")
                        color: root.mutedForeground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        elide: Text.ElideMiddle
                      }

                      Row {
                        id: repositoryLists
                        width: parent.width
                        height: root.compactDraftLayout
                          ? Math.max(Style.space(80), parent.height - parent.fixedContentHeight)
                          : Style.space(245)
                        spacing: Style.space(8)

                        Rectangle {
                          visible: !root.compactWidthLayout || root.repositorySourceIndex === 0
                          width: root.compactWidthLayout
                            ? parent.width
                            : (parent.width - parent.spacing) / 2
                          height: parent.height
                          radius: Style.cornerRadius
                          color: "transparent"
                          border.width: root.repositorySourceIndex === 0 ? 2 : 1
                          border.color: root.repositorySourceIndex === 0
                            ? root.accent
                            : Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.16)

                          Column {
                            anchors.fill: parent
                            anchors.margins: Style.space(8)
                            spacing: Style.space(5)

                            Text {
                              text: "Local  " + root.filteredLocalRepositories().length
                              color: root.foreground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              font.bold: true
                            }

                            ListView {
                              id: localRepositoryList
                              width: parent.width
                              height: parent.height - Style.space(28)
                              clip: true
                              spacing: Style.space(3)
                              model: root.filteredLocalRepositories()
                              currentIndex: root.repositorySourceIndex === 0
                                ? root.selectedRepositoryIndex
                                : -1

                              delegate: Rectangle {
                                id: localRepositoryDelegate
                                required property int index
                                required property var modelData
                                width: ListView.view.width
                                height: Style.space(48)
                                radius: Style.cornerRadius
                                color: root.repositorySourceIndex === 0
                                  && index === root.selectedRepositoryIndex
                                  ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.14)
                                  : "transparent"

                                Column {
                                  anchors.left: parent.left
                                  anchors.right: parent.right
                                  anchors.verticalCenter: parent.verticalCenter
                                  anchors.leftMargin: Style.space(7)
                                  anchors.rightMargin: Style.space(7)
                                  spacing: Style.space(2)

                                  Text {
                                    width: parent.width
                                    text: localRepositoryDelegate.modelData.name_with_owner
                                      || localRepositoryDelegate.modelData.name
                                    color: root.foreground
                                    font.family: root.fontFamily
                                    font.pixelSize: Style.font.bodySmall
                                    font.bold: true
                                    elide: Text.ElideRight
                                  }
                                  Text {
                                    width: parent.width
                                    text: (localRepositoryDelegate.modelData.branch || "detached HEAD")
                                      + (localRepositoryDelegate.modelData.dirty ? "  •  dirty" : "  •  clean")
                                    color: localRepositoryDelegate.modelData.dirty
                                      ? root.urgent : root.mutedForeground
                                    font.family: root.fontFamily
                                    font.pixelSize: Style.font.bodySmall
                                    elide: Text.ElideRight
                                  }
                                }

                                MouseArea {
                                  anchors.fill: parent
                                  onClicked: {
                                    root.repositorySourceIndex = 0
                                    root.selectedRepositoryIndex = localRepositoryDelegate.index
                                  }
                                  onDoubleClicked: root.activateRepository()
                                }
                              }
                            }
                          }
                        }

                        Rectangle {
                          visible: !root.compactWidthLayout || root.repositorySourceIndex === 1
                          width: root.compactWidthLayout
                            ? parent.width
                            : (parent.width - parent.spacing) / 2
                          height: parent.height
                          radius: Style.cornerRadius
                          color: "transparent"
                          border.width: root.repositorySourceIndex === 1 ? 2 : 1
                          border.color: root.repositorySourceIndex === 1
                            ? root.accent
                            : Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.16)

                          Column {
                            anchors.fill: parent
                            anchors.margins: Style.space(8)
                            spacing: Style.space(5)

                            Text {
                              text: "GitHub  " + root.filteredGithubRepositories().length
                              color: root.foreground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              font.bold: true
                            }

                            ListView {
                              id: githubRepositoryList
                              width: parent.width
                              height: parent.height - Style.space(28)
                              clip: true
                              spacing: Style.space(3)
                              model: root.filteredGithubRepositories()
                              currentIndex: root.repositorySourceIndex === 1
                                ? root.selectedRepositoryIndex
                                : -1

                              delegate: Rectangle {
                                id: githubRepositoryDelegate
                                required property int index
                                required property var modelData
                                width: ListView.view.width
                                height: Style.space(48)
                                radius: Style.cornerRadius
                                color: root.repositorySourceIndex === 1
                                  && index === root.selectedRepositoryIndex
                                  ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.14)
                                  : "transparent"

                                Column {
                                  anchors.left: parent.left
                                  anchors.right: parent.right
                                  anchors.verticalCenter: parent.verticalCenter
                                  anchors.leftMargin: Style.space(7)
                                  anchors.rightMargin: Style.space(7)
                                  spacing: Style.space(2)

                                  Text {
                                    width: parent.width
                                    text: githubRepositoryDelegate.modelData.name_with_owner
                                    color: root.foreground
                                    font.family: root.fontFamily
                                    font.pixelSize: Style.font.bodySmall
                                    font.bold: true
                                    elide: Text.ElideRight
                                  }
                                  Text {
                                    width: parent.width
                                    text: "Clone"
                                      + (githubRepositoryDelegate.modelData.fork ? "  •  fork" : "")
                                      + (githubRepositoryDelegate.modelData.archived ? "  •  archived" : "")
                                    color: githubRepositoryDelegate.modelData.archived
                                      ? root.urgent : root.mutedForeground
                                    font.family: root.fontFamily
                                    font.pixelSize: Style.font.bodySmall
                                    elide: Text.ElideRight
                                  }
                                }

                                MouseArea {
                                  anchors.fill: parent
                                  onClicked: {
                                    root.repositorySourceIndex = 1
                                    root.selectedRepositoryIndex = githubRepositoryDelegate.index
                                  }
                                  onDoubleClicked: root.activateRepository()
                                }
                              }
                            }
                          }
                        }
                      }

                      Text {
                        id: repositoryStatus
                        visible: repositoryEngine.requestPending
                          && (repositoryEngine.pendingMethod === "list_repositories"
                              || repositoryEngine.pendingMethod === "clone_repository")
                        width: parent.width
                        text: repositoryEngine.pendingMethod === "clone_repository"
                          ? "Cloning repository into the projects root…"
                          : "Refreshing local and GitHub repositories…"
                        color: root.accent
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                      }

                      Text {
                        id: localDiscoveryWarning
                        visible: !!repositoryEngine.repositoryCatalog.local_error
                        width: parent.width
                        text: "Local discovery warning: "
                          + repositoryEngine.repositoryCatalog.local_error
                        color: root.urgent
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                        maximumLineCount: 2
                        elide: Text.ElideRight
                      }

                      Text {
                        id: githubDiscoveryWarning
                        visible: !!repositoryEngine.repositoryCatalog.github_error
                        width: parent.width
                        text: "GitHub unavailable: " + repositoryEngine.repositoryCatalog.github_error
                        color: root.urgent
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                        maximumLineCount: 2
                        elide: Text.ElideRight
                      }
                    }

                    Column {
                      visible: root.draftStep === "path"
                      width: parent.width
                      spacing: Style.space(7)

                      Text {
                        width: parent.width
                        text: "Use a repository outside the configured projects root. Tab completes directories through the Rust engine."
                        color: root.mutedForeground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                      }

                      TextField {
                        id: repositoryField
                        width: parent.width
                        enabled: !engine.requestPending
                          || engine.pendingMethod === "complete_repository_path"
                        placeholderText: "/absolute/path/to/repository"
                        foreground: root.foreground
                        accent: root.accent
                        onTextEdited: root.repositoryPathCandidates = []

                        Keys.onPressed: function(event) {
                          if (event.key === Qt.Key_Escape) {
                            root.returnToRepositoryBrowser(); event.accepted = true
                          } else if (event.key === Qt.Key_Tab && !(event.modifiers & Qt.ShiftModifier)) {
                            root.completeRepositoryPath(); event.accepted = true
                          } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                            root.acceptManualRepositoryPath(); event.accepted = true
                          }
                        }
                      }

                      Text {
                        visible: root.repositoryPathCandidates.length > 1
                        width: parent.width
                        text: root.repositoryCandidateText()
                        color: root.mutedForeground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                      }
                    }

                    Column {
                      visible: root.draftStep === "goal"
                      width: parent.width
                      spacing: Style.space(8)

                      Text {
                        width: parent.width
                        text: root.selectedRepositoryLabel + "\n" + root.selectedRepositoryPath
                        color: root.mutedForeground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WrapAnywhere
                      }

                      TextField {
                        id: goalField
                        width: parent.width
                        enabled: !engine.requestPending
                        placeholderText: "Describe a small engineering goal"
                        foreground: root.foreground
                        accent: root.accent

                        Keys.onPressed: function(event) {
                          if (event.key === Qt.Key_Escape) {
                            root.cancelDraftEntry(); event.accepted = true
                          } else if (event.key === Qt.Key_Backtab || (event.key === Qt.Key_Tab && (event.modifiers & Qt.ShiftModifier))) {
                            root.returnToRepositoryBrowser(); event.accepted = true
                          } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                            root.submitDraft(); event.accepted = true
                          }
                        }
                      }

                      Row {
                        spacing: Style.space(8)

                        Button {
                          text: engine.requestPending ? "Creating…" : "Create draft"
                          bordered: true
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.submitDraft()
                        }

                        Button {
                          text: "Back"
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.returnToRepositoryBrowser()
                        }
                      }
                    }

                    Text {
                      id: draftErrorText
                      visible: root.draftError() !== ""
                      width: parent.width
                      text: root.draftError()
                      color: root.urgent
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      wrapMode: Text.WordWrap
                      maximumLineCount: 2
                      elide: Text.ElideRight
                    }
                  }

                  Column {
                    visible: root.selectedSection === 0 && !root.editingDraft && !!engine.activeRun
                    width: parent.width
                    spacing: Style.space(8)

                    Text {
                      width: parent.width
                      text: engine.activeRun ? engine.activeRun.goal : ""
                      color: root.foreground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.body
                      font.bold: true
                      wrapMode: Text.WordWrap
                    }

                    Text {
                      width: parent.width
                      text: engine.activeRun ? engine.activeRun.repository : ""
                      color: root.mutedForeground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      elide: Text.ElideMiddle
                    }

                    Text {
                      text: engine.activeRun
                        ? ((engine.activeRun.branch || "detached HEAD") + "  •  "
                           + String(engine.activeRun.base_revision || "").slice(0, 12) + "  •  "
                           + (engine.activeRun.worktree_dirty ? "dirty working tree" : "clean working tree"))
                        : ""
                      color: engine.activeRun && engine.activeRun.worktree_dirty ? root.urgent : root.mutedForeground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                    }

                    Text {
                      width: parent.width
                      visible: !root.latestImplementationAttempt()
                      text: engine.activeRun && engine.activeRun.plan
                        ? "The plan is durable. Open the Plan section to inspect its tasks and decision state."
                        : "This draft survives engine and shell restarts. Open the Plan section to choose Codex or Claude as the read-only planner."
                      color: root.mutedForeground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      wrapMode: Text.WordWrap
                    }

                    Column {
                      visible: !!root.latestImplementationAttempt()
                      width: parent.width
                      spacing: Style.space(7)

                      Text {
                        width: parent.width
                        text: {
                          var attempt = root.latestImplementationAttempt()
                          if (!attempt) return ""
                          return attempt.agent + "  •  "
                            + root.implementationTaskTitle(attempt) + "  •  "
                            + (attempt.paused ? "paused" : attempt.status)
                        }
                        color: root.runningImplementationAttempt()
                          ? root.accent
                          : (root.latestImplementationAttempt()
                             && root.latestImplementationAttempt().status === "failed"
                             ? root.urgent : root.foreground)
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        font.bold: true
                        wrapMode: Text.WordWrap
                      }

                      Text {
                        visible: root.latestImplementationActivity().length === 0
                        width: parent.width
                        text: root.runningImplementationAttempt()
                          ? "Waiting for the agent's first activity update…"
                          : "No activity output was recorded for this attempt."
                        color: root.mutedForeground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                      }

                      ListView {
                        id: implementationActivityView
                        visible: root.latestImplementationActivity().length > 0
                        width: parent.width
                        height: root.panelDesignHeight < 620
                          ? Style.space(105) : Style.space(155)
                        clip: true
                        spacing: Style.space(4)
                        model: root.latestImplementationActivity()

                        onCountChanged: positionViewAtEnd()

                        delegate: Rectangle {
                          id: activityDelegate
                          required property var modelData
                          width: ListView.view.width
                          height: activityText.implicitHeight + Style.space(10)
                          radius: Style.cornerRadius
                          color: Qt.rgba(
                            root.foreground.r,
                            root.foreground.g,
                            root.foreground.b,
                            activityDelegate.modelData.kind === "diagnostic" ? 0.08 : 0.04
                          )

                          Text {
                            id: activityText
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            anchors.margins: Style.space(5)
                            text: activityDelegate.modelData.message || ""
                            color: activityDelegate.modelData.kind === "diagnostic"
                              ? root.mutedForeground : root.foreground
                            font.family: root.fontFamily
                            font.pixelSize: Style.font.bodySmall
                            wrapMode: Text.WordWrap
                          }
                        }
                      }

                      Text {
                        visible: !!root.latestImplementationAttempt()
                          && !!root.latestImplementationAttempt().error_message
                        width: parent.width
                        text: root.latestImplementationAttempt()
                          ? (root.latestImplementationAttempt().error_message || "") : ""
                        color: root.latestImplementationAttempt()
                          && root.latestImplementationAttempt().status === "cancelled"
                          ? root.mutedForeground : root.urgent
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                      }

                      Flow {
                        visible: !!root.runningImplementationAttempt()
                          && !root.confirmingImplementationCancel
                          && root.implementationInterventionMode === ""
                        width: parent.width
                        spacing: Style.space(8)

                        Button {
                          text: root.runningImplementationAttempt()
                            && root.runningImplementationAttempt().paused ? "Resume" : "Pause"
                          enabled: !controlEngine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.toggleImplementationPause()
                        }
                        Button {
                          text: "Redirect"
                          enabled: !controlEngine.requestPending && !continuationEngine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.beginImplementationIntervention("redirect")
                        }
                        Button {
                          text: "Add context"
                          enabled: !controlEngine.requestPending && !continuationEngine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.beginImplementationIntervention("additional_context")
                        }
                        Button {
                          text: "Cancel"
                          enabled: !controlEngine.requestPending
                          foreground: root.foreground
                          accent: root.urgent
                          onClicked: root.beginImplementationCancel()
                        }
                      }

                      Column {
                        visible: !root.runningImplementationAttempt()
                          && !!root.pendingContinuationAttempt()
                        width: parent.width
                        spacing: Style.space(7)

                        Text {
                          width: parent.width
                          text: {
                            var pending = root.pendingContinuationAttempt()
                            if (!pending) return ""
                            return "A " + pending.pending_continuation_kind.replace(/_/g, " ")
                              + " instruction was saved before the engine stopped. Retry it to continue from the retained worktree."
                          }
                          color: root.urgent
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          wrapMode: Text.WordWrap
                        }

                        Button {
                          text: continuationEngine.requestPending
                            ? "Starting continuation…" : "Retry saved continuation"
                          bordered: true
                          enabled: !continuationEngine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.retryPendingContinuation()
                        }
                      }

                      Column {
                        visible: root.implementationInterventionMode !== ""
                        width: parent.width
                        spacing: Style.space(7)

                        Text {
                          width: parent.width
                          text: root.implementationInterventionMode === "redirect"
                            ? "Redirect the implementation. The current process will stop and a linked continuation will inspect its partial changes."
                            : "Add context. The current process will stop and a linked continuation will inspect its partial changes."
                          color: root.mutedForeground
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          wrapMode: Text.WordWrap
                        }

                        TextField {
                          id: implementationInstructionField
                          width: parent.width
                          enabled: !continuationEngine.requestPending
                          placeholderText: root.implementationInterventionMode === "redirect"
                            ? "Describe the corrected approach" : "Provide the additional context"
                          foreground: root.foreground
                          accent: root.accent
                          Keys.onPressed: function(event) {
                            if (event.key === Qt.Key_Escape) {
                              root.cancelImplementationIntervention(); event.accepted = true
                            } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                              root.submitImplementationIntervention(); event.accepted = true
                            }
                          }
                        }

                        Row {
                          spacing: Style.space(8)
                          Button {
                            text: "Submit"
                            bordered: true
                            enabled: !continuationEngine.requestPending
                              && implementationInstructionField.text.trim() !== ""
                            foreground: root.foreground
                            accent: root.accent
                            onClicked: root.submitImplementationIntervention()
                          }
                          Button {
                            text: "Keep current attempt"
                            enabled: !continuationEngine.requestPending
                            foreground: root.foreground
                            accent: root.accent
                            onClicked: root.cancelImplementationIntervention()
                          }
                        }
                      }

                      Column {
                        visible: root.confirmingImplementationCancel
                        width: parent.width
                        spacing: Style.space(7)

                        Text {
                          width: parent.width
                          text: {
                            var attempt = root.runningImplementationAttempt()
                            return attempt
                              ? "Stop " + attempt.agent + " on "
                                + root.implementationTaskTitle(attempt)
                                + "? Partial changes remain in the task worktree for inspection or retry."
                              : ""
                          }
                          color: root.urgent
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          wrapMode: Text.WordWrap
                        }

                        Row {
                          spacing: Style.space(8)
                          Button {
                            text: controlEngine.requestPending ? "Cancelling…" : "Confirm cancel"
                            bordered: true
                            enabled: !controlEngine.requestPending
                            foreground: root.foreground
                            accent: root.urgent
                            onClicked: root.confirmImplementationCancel()
                          }
                          Button {
                            text: "Keep running"
                            enabled: !controlEngine.requestPending
                            foreground: root.foreground
                            accent: root.accent
                            onClicked: root.confirmingImplementationCancel = false
                          }
                        }
                      }

                      Text {
                        visible: controlEngine.requestError !== ""
                          || continuationEngine.requestError !== ""
                        width: parent.width
                        text: controlEngine.requestError !== ""
                          ? controlEngine.requestError : continuationEngine.requestError
                        color: root.urgent
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                      }
                    }

                    Button {
                      text: "New draft"
                      enabled: !engine.activeRun || engine.activeRun.run_status !== "running"
                      foreground: root.foreground
                      accent: root.accent
                      onClicked: root.beginDraftEntry()
                    }
                  }

                  Column {
                    visible: root.selectedSection === 1
                    width: parent.width
                    spacing: Style.space(10)

                    Text {
                      visible: !engine.activeRun
                      width: parent.width
                      text: "Create a durable draft in Overview before asking an agent to inspect the repository."
                      color: root.mutedForeground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      wrapMode: Text.WordWrap
                    }

                    Text {
                      visible: !!engine.activeRun && engine.activeRun.run_status === "planning"
                      width: parent.width
                      text: "The selected agent is inspecting the repository with read-only permissions. The engine owns the process and will preserve either the proposal or the failure evidence."
                      color: root.mutedForeground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      wrapMode: Text.WordWrap
                    }

                    Column {
                      visible: !!engine.activeRun
                        && engine.activeRun.run_status !== "planning"
                        && (!root.currentPlan() || root.currentPlan().status === "rejected"
                            || engine.activeRun.run_status === "failed")
                      width: parent.width
                      spacing: Style.space(10)

                      Text {
                        width: parent.width
                        text: engine.activeRun && engine.activeRun.last_error
                          ? engine.activeRun.last_error
                          : (root.currentPlan() && root.currentPlan().status === "rejected"
                             ? "The previous proposal was rejected. Generate a new revision when ready."
                             : "Choose which independent CLI should inspect the repository and prepare the first structured plan.")
                        color: engine.activeRun && engine.activeRun.last_error ? root.urgent : root.mutedForeground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                      }

                      Row {
                        spacing: Style.space(8)

                        Button {
                          text: engine.requestPending && engine.pendingMethod === "generate_plan" ? "Planning…" : "Plan with Codex"
                          bordered: true
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.generatePlan("codex")
                        }

                        Button {
                          text: "Plan with Claude"
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.generatePlan("claude")
                        }
                      }
                    }

                    Column {
                      visible: !!root.currentPlan()
                        && root.currentPlan().status !== "rejected"
                        && !root.editingPlanTask && !root.rejectingPlan
                      width: parent.width
                      spacing: Style.space(8)

                      Text {
                        width: parent.width
                        text: root.currentPlan() ? root.currentPlan().summary : ""
                        color: root.foreground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        font.bold: true
                        wrapMode: Text.WordWrap
                      }

                      Text {
                        text: root.currentPlan()
                          ? ("Revision " + root.currentPlan().revision + "  •  "
                             + root.currentPlan().planner + "  •  " + root.currentPlan().status)
                          : ""
                        color: root.mutedForeground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                      }

                      ListView {
                        id: planTaskList
                        width: parent.width
                        height: Style.space(215)
                        clip: true
                        spacing: Style.space(6)
                        model: root.currentPlan() && root.currentPlan().tasks
                          ? root.currentPlan().tasks
                          : []
                        currentIndex: root.selectedTaskIndex

                        delegate: Rectangle {
                          id: taskDelegate
                          required property int index
                          required property var modelData
                          width: ListView.view.width
                          height: taskContent.implicitHeight + Style.space(16)
                          radius: Style.cornerRadius
                          color: index === root.selectedTaskIndex
                            ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.14)
                            : "transparent"
                          border.width: index === root.selectedTaskIndex ? 1 : 0
                          border.color: root.accent

                          Column {
                            id: taskContent
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.top: parent.top
                            anchors.margins: Style.space(8)
                            spacing: Style.space(3)

                            Text {
                              width: parent.width
                              text: taskDelegate.modelData.position + ". " + taskDelegate.modelData.title
                              color: root.foreground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              font.bold: true
                              wrapMode: Text.WordWrap
                            }

                            Text {
                              width: parent.width
                              text: taskDelegate.modelData.description
                              color: root.mutedForeground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              wrapMode: Text.WordWrap
                            }

                            Text {
                              width: parent.width
                              text: "✓ " + (taskDelegate.modelData.acceptance_criteria || []).join("\n✓ ")
                              color: root.mutedForeground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              wrapMode: Text.WordWrap
                            }

                            Text {
                              visible: taskDelegate.modelData.depends_on
                                && taskDelegate.modelData.depends_on.length > 0
                              text: "Depends on: " + (taskDelegate.modelData.depends_on || []).join(", ")
                              color: root.mutedForeground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                            }
                          }

                          MouseArea {
                            anchors.fill: parent
                            onClicked: {
                              root.selectedTaskIndex = taskDelegate.index
                              root.focusArea = "content"
                            }
                            onDoubleClicked: {
                              root.selectedTaskIndex = taskDelegate.index
                              root.beginPlanTaskEdit()
                            }
                          }
                        }
                      }

                      Row {
                        visible: root.currentPlan() && root.currentPlan().status === "proposed"
                        spacing: Style.space(6)

                        Button {
                          text: "Approve"
                          bordered: true
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: engine.approvePlan()
                        }
                        Button {
                          text: "Edit"
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.beginPlanTaskEdit()
                        }
                        Button {
                          text: "↑"
                          enabled: !engine.requestPending && root.selectedTaskIndex > 0
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.moveCurrentTask("up")
                        }
                        Button {
                          text: "↓"
                          enabled: !engine.requestPending && root.currentPlan()
                            && root.selectedTaskIndex < root.currentPlan().tasks.length - 1
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.moveCurrentTask("down")
                        }
                        Button {
                          text: "Reject"
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.beginRejectPlan()
                        }
                      }

                      Text {
                        visible: root.currentPlan() && root.currentPlan().status === "approved"
                        width: parent.width
                        text: root.latestImplementationAttempt()
                          ? "This plan is approved and durable. Open Overview to inspect the latest implementation attempt and its recent activity."
                          : "This plan is approved and durable. Create the task worktree and assign an implementer from the CLI."
                        color: root.mutedForeground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                      }
                    }

                    Column {
                      visible: root.editingPlanTask
                      width: parent.width
                      spacing: Style.space(7)

                      Text {
                        text: "Edit selected task"
                        color: root.foreground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        font.bold: true
                      }

                      TextField {
                        id: taskTitleField
                        width: parent.width
                        enabled: !engine.requestPending
                        placeholderText: "Task title"
                        foreground: root.foreground
                        accent: root.accent
                        Keys.onPressed: function(event) {
                          if (event.key === Qt.Key_Escape) {
                            root.cancelPlanInput(); event.accepted = true
                          } else if (event.key === Qt.Key_Tab || event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                            taskDescriptionField.forceActiveFocus(); event.accepted = true
                          }
                        }
                      }

                      TextField {
                        id: taskDescriptionField
                        width: parent.width
                        enabled: !engine.requestPending
                        placeholderText: "Task description"
                        foreground: root.foreground
                        accent: root.accent
                        Keys.onPressed: function(event) {
                          if (event.key === Qt.Key_Escape) {
                            root.cancelPlanInput(); event.accepted = true
                          } else if (event.key === Qt.Key_Tab || event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                            taskCriteriaField.forceActiveFocus(); event.accepted = true
                          }
                        }
                      }

                      TextField {
                        id: taskCriteriaField
                        width: parent.width
                        enabled: !engine.requestPending
                        placeholderText: "Criteria separated with ||"
                        foreground: root.foreground
                        accent: root.accent
                        Keys.onPressed: function(event) {
                          if (event.key === Qt.Key_Escape) {
                            root.cancelPlanInput(); event.accepted = true
                          } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                            root.savePlanTask(); event.accepted = true
                          }
                        }
                      }

                      Row {
                        spacing: Style.space(8)
                        Button {
                          text: engine.requestPending ? "Saving…" : "Save revision"
                          bordered: true
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.savePlanTask()
                        }
                        Button {
                          text: "Cancel"
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.cancelPlanInput()
                        }
                      }
                    }

                    Column {
                      visible: root.rejectingPlan
                      width: parent.width
                      spacing: Style.space(8)

                      Text {
                        width: parent.width
                        text: "Rejecting preserves this proposal in history and returns the run to draft state. Add an optional reason."
                        color: root.mutedForeground
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                      }
                      TextField {
                        id: rejectionReasonField
                        width: parent.width
                        enabled: !engine.requestPending
                        placeholderText: "Reason for rejection (optional)"
                        foreground: root.foreground
                        accent: root.accent
                        Keys.onPressed: function(event) {
                          if (event.key === Qt.Key_Escape) {
                            root.cancelPlanInput(); event.accepted = true
                          } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                            root.submitPlanRejection(); event.accepted = true
                          }
                        }
                      }
                      Row {
                        spacing: Style.space(8)
                        Button {
                          text: engine.requestPending ? "Rejecting…" : "Confirm rejection"
                          bordered: true
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.submitPlanRejection()
                        }
                        Button {
                          text: "Cancel"
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.cancelPlanInput()
                        }
                      }
                    }

                    Text {
                      visible: engine.requestError !== ""
                      width: parent.width
                      text: engine.requestError
                      color: root.urgent
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      wrapMode: Text.WordWrap
                    }
                  }

                  Text {
                    visible: engine.lastError !== ""
                    width: parent.width
                    text: engine.lastError
                    color: root.urgent
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.bodySmall
                    wrapMode: Text.WordWrap
                  }

                  Column {
                    visible: root.selectedSection === 3
                    width: parent.width
                    spacing: Style.space(10)
                    property var verification: root.latestTaskRecord(engine.activeRun ? engine.activeRun.verification_attempts : [])

                    Button {
                      text: engine.requestPending && engine.pendingMethod === "finish_task" ? "Finishing…" : "Finish selected task"
                      bordered: true
                      enabled: root.finishContext() !== null && !engine.requestPending
                      foreground: root.foreground
                      accent: root.accent
                      onClicked: root.finishSelectedTask()
                    }
                    Text {
                      width: parent.width
                      text: parent.verification ? "Latest verification: " + parent.verification.status : "No verification recorded for this task"
                      color: parent.verification && parent.verification.status === "passed" ? root.accent : root.mutedForeground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      wrapMode: Text.WordWrap
                    }
                    Repeater {
                      model: parent.verification ? parent.verification.commands : []
                      Text {
                        required property var modelData
                        width: parent.width
                        text: modelData.label + " — " + modelData.status
                          + (modelData.exit_code === null ? "" : " (exit " + modelData.exit_code + ")")
                        color: modelData.status === "passed" ? root.accent : root.urgent
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                      }
                    }
                  }

                  Column {
                    visible: root.selectedSection === 4
                    width: parent.width
                    spacing: Style.space(10)
                    property var review: root.latestTaskRecord(engine.activeRun ? engine.activeRun.review_attempts : [])
                    property var taskCommit: root.latestTaskRecord(engine.activeRun ? engine.activeRun.task_commits : [])

                    Button {
                      text: engine.requestPending && engine.pendingMethod === "finish_task" ? "Reviewing…" : "Finish selected task"
                      bordered: true
                      enabled: root.finishContext() !== null && !engine.requestPending
                      foreground: root.foreground
                      accent: root.accent
                      onClicked: root.finishSelectedTask()
                    }
                    Text {
                      width: parent.width
                      text: parent.review ? "Reviewer: " + parent.review.reviewer + " · "
                        + parent.review.independence + " · " + parent.review.status
                        : "No independent review recorded for this task"
                      color: parent.review && parent.review.status === "approved" ? root.accent : root.mutedForeground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      wrapMode: Text.WordWrap
                    }
                    Repeater {
                      model: parent.review && parent.review.result ? parent.review.result.findings : []
                      Text {
                        required property var modelData
                        width: parent.width
                        text: modelData.severity + ": " + modelData.summary + " — " + modelData.evidence
                        color: root.urgent
                        font.family: root.fontFamily
                        font.pixelSize: Style.font.bodySmall
                        wrapMode: Text.WordWrap
                      }
                    }
                    Text {
                      width: parent.width
                      visible: parent.taskCommit !== null
                      text: parent.taskCommit ? "Local commit: " + parent.taskCommit.status
                        + (parent.taskCommit.commit_hash ? " · " + parent.taskCommit.commit_hash.slice(0, 12) : "") : ""
                      color: parent.taskCommit && parent.taskCommit.status === "created" ? root.accent : root.urgent
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                    }
                  }
                }
              }
            }
          }

          Text {
            id: footerText
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            text: root.footerHelp()
            color: root.mutedForeground
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
            elide: Text.ElideRight
          }
        }
      }
    }
  }
}
