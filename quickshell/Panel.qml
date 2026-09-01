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
  property string taskActionMode: ""
  property string taskActionTaskId: ""
  property bool confirmingImplementationCancel: false
  property string completionDecisionMode: ""
  property string integrationMode: ""
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
    if (taskActionMode !== "") {
      cancelTaskAction()
      return
    }
    if (completionDecisionMode !== "") {
      completionDecisionMode = ""
      return
    }
    if (integrationMode !== "") {
      integrationMode = ""
      Qt.callLater(function() { keyCatcher.forceActiveFocus() })
      return
    }
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
    if (confirmingImplementationCancel || completionDecisionMode !== ""
        || integrationMode !== ""
        || implementationInterventionMode !== "" || taskActionMode !== "") return
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
    } else if (selectedSection === 2 && changesFlickable.visible) {
      changesFlickable.contentY = Math.max(
        0,
        Math.min(
          Math.max(0, changesFlickable.contentHeight - changesFlickable.height),
          changesFlickable.contentY + dy * Style.space(36)
        )
      )
    }
  }

  function activateNavigation() {
    if (taskActionMode === "confirm_worktree") {
      confirmTaskWorktree()
      return
    }
    if (taskActionMode === "choose_agent") return
    if (completionDecisionMode !== "") {
      confirmCompletionDecision()
      return
    }
    if (integrationMode === "confirm") {
      confirmTaskIntegration()
      return
    }
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
    else if (selectedSection === 1) {
      var plan = currentPlan()
      if (plan && plan.status === "approved") beginSelectedTaskAction()
      else beginPlanTaskEdit()
    }
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

  function taskById(taskId) {
    var plan = currentPlan()
    var tasks = plan && plan.tasks ? plan.tasks : []
    for (var i = 0; i < tasks.length; i++)
      if (tasks[i].id === taskId) return tasks[i]
    return null
  }

  function latestRecordForTask(records, taskId) {
    if (!records || !taskId) return null
    for (var i = records.length - 1; i >= 0; i--)
      if (records[i].task_id === taskId) return records[i]
    return null
  }

  function latestWorktreeForTask(taskId) {
    return latestRecordForTask(
      engine.activeRun ? engine.activeRun.worktrees : [], taskId)
  }

  function readyWorktreeForTask(taskId) {
    var worktrees = engine.activeRun ? engine.activeRun.worktrees || [] : []
    for (var i = worktrees.length - 1; i >= 0; i--)
      if (worktrees[i].task_id === taskId && worktrees[i].status === "ready")
        return worktrees[i]
    return null
  }

  function latestImplementationForTask(taskId) {
    return latestRecordForTask(
      engine.activeRun ? engine.activeRun.implementation_attempts : [], taskId)
  }

  function latestCommitForTask(taskId) {
    return latestRecordForTask(
      engine.activeRun ? engine.activeRun.task_commits : [], taskId)
  }

  function taskActionCode(task) {
    var run = engine.activeRun
    var plan = currentPlan()
    if (!task || !run || !plan || plan.status !== "approved") return "unavailable"

    var taskCommit = latestCommitForTask(task.id)
    if (taskCommit) {
      if (taskCommit.status === "proposed" || taskCommit.status === "reserved")
        return "inspect"
      if (taskCommit.status === "created") return "complete"
      if (taskCommit.status === "rejected") return "rejected"
    }

    var attempt = latestImplementationForTask(task.id)
    if (attempt && attempt.status === "running") return "running"
    if ((task.depends_on || []).length > 0) return "dependencies_blocked"

    var readyWorktree = readyWorktreeForTask(task.id)
    if (attempt && attempt.status === "completed" && readyWorktree
        && attempt.worktree_id === readyWorktree.id) return "finish"
    if (run.run_status === "running") return "busy"

    var worktree = latestWorktreeForTask(task.id)
    if (!worktree || worktree.status === "failed") return "create_worktree"
    if (worktree.status === "reserved") return "creating_worktree"
    if (worktree.status === "ready")
      return attempt && attempt.worktree_id === worktree.id
        ? "retry_implementation" : "choose_agent"
    return "worktree_unavailable"
  }

  function taskActionLabel(task) {
    if (!task) return "Unavailable"
    if (taskActionEngine.requestPending && taskActionTaskId === task.id) {
      return taskActionEngine.pendingMethod === "create_task_worktree"
        ? "Creating isolated worktree…" : "Implementation running…"
    }
    var code = taskActionCode(task)
    if (code === "create_worktree") return "Ready to create worktree"
    if (code === "creating_worktree") return "Worktree creation in progress"
    if (code === "choose_agent") return "Ready to choose implementer"
    if (code === "retry_implementation") return "Ready to retry implementation"
    if (code === "running") return "Implementation running"
    if (code === "busy") return "Waiting for the active implementation"
    if (code === "dependencies_blocked")
      return "Blocked until prerequisite task results can be composed"
    if (code === "finish") return "Ready for verification and review"
    if (code === "inspect") return "Ready for final inspection"
    if (code === "complete") return "Local task commit created"
    if (code === "rejected") return "Final result rejected and preserved"
    if (code === "worktree_unavailable") return "Worktree needs manual attention"
    return "Awaiting an approved plan"
  }

  function taskActionTask() {
    return taskById(taskActionTaskId)
  }

  function beginSelectedTaskAction() {
    if (taskActionMode !== "" || taskActionEngine.requestPending) return
    var task = selectedTask()
    var code = taskActionCode(task)
    if (code === "create_worktree") {
      taskActionTaskId = task.id
      taskActionEngine.requestError = ""
      taskActionMode = "confirm_worktree"
    } else if (code === "choose_agent" || code === "retry_implementation") {
      taskActionTaskId = task.id
      taskActionEngine.requestError = ""
      taskActionMode = "choose_agent"
    } else if (code === "running") {
      selectedSection = 0
    } else if (code === "finish" || code === "inspect") {
      selectedSection = 2
    }
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function cancelTaskAction() {
    taskActionMode = ""
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function launchSelectedTaskImplementation(agent) {
    var task = selectedTask()
    var code = taskActionCode(task)
    if (!task || (code !== "choose_agent" && code !== "retry_implementation")) return
    taskActionTaskId = task.id
    taskActionEngine.requestError = ""
    launchTaskImplementation(agent)
  }

  function confirmTaskWorktree() {
    var task = taskActionTask()
    var plan = currentPlan()
    var run = engine.activeRun
    if (!task || !plan || !run || taskActionEngine.requestPending) return
    if (taskActionEngine.createTaskWorktree(run.id, plan.id, task.id))
      taskActionMode = ""
  }

  function launchTaskImplementation(agent) {
    var task = taskActionTask()
    var plan = currentPlan()
    var run = engine.activeRun
    var worktree = task ? readyWorktreeForTask(task.id) : null
    if (!task || !plan || !run || !worktree || taskActionEngine.requestPending) return
    if (taskActionEngine.runTaskImplementation(
          run.id, plan.id, task.id, worktree.id, agent))
      taskActionMode = ""
  }

  function taskActionError(taskId) {
    return taskActionTaskId === taskId ? taskActionEngine.requestError : ""
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
    var task = selectedTask()
    if (!task || !records) return null
    for (var i = records.length - 1; i >= 0; i--)
      if (records[i].task_id === task.id) return records[i]
    return null
  }

  function recordById(records, id) {
    if (!records || !id) return null
    for (var i = records.length - 1; i >= 0; i--)
      if (records[i].id === id) return records[i]
    return null
  }

  function finishSelectedTask() {
    var context = finishContext()
    if (!context || engine.requestPending) return
    engine.finishTask(context.plan.id, context.task.id, context.worktree.id, context.implementation.id)
  }

  function proposedTaskCommit() {
    var proposal = latestTaskRecord(engine.activeRun ? engine.activeRun.task_commits : [])
    return proposal && proposal.status === "proposed" ? proposal : null
  }

  function beginCompletionDecision(mode) {
    if (!proposedTaskCommit() || engine.requestPending) return
    completionDecisionMode = mode
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function confirmCompletionDecision() {
    var proposal = proposedTaskCommit()
    if (!proposal || engine.requestPending || completionDecisionMode === "") return
    var mode = completionDecisionMode
    completionDecisionMode = ""
    if (mode === "approve") engine.approveTaskCommit(proposal.id)
    else engine.rejectTaskCommit(proposal.id, "Rejected during final inspection")
  }

  function createdTaskCommit() {
    var commit = latestTaskRecord(engine.activeRun ? engine.activeRun.task_commits : [])
    return commit && commit.status === "created" ? commit : null
  }

  function latestTaskIntegration() {
    var commit = createdTaskCommit()
    var integrations = engine.activeRun ? engine.activeRun.task_integrations || [] : []
    if (!commit) return null
    for (var i = integrations.length - 1; i >= 0; i--)
      if (integrations[i].task_commit_id === commit.id) return integrations[i]
    return null
  }

  function beginTaskIntegration() {
    var commit = createdTaskCommit()
    if (!commit || engine.requestPending) return
    integrationTargetField.text = engine.activeRun && engine.activeRun.branch
      ? engine.activeRun.branch : ""
    integrationMode = "edit"
    Qt.callLater(function() { integrationTargetField.forceActiveFocus() })
  }

  function reviewTaskIntegration() {
    if (integrationTargetField.text.trim() === "") return
    integrationMode = "confirm"
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function confirmTaskIntegration() {
    var commit = createdTaskCommit()
    var branch = integrationTargetField.text.trim()
    if (!commit || branch === "" || engine.requestPending) return
    integrationMode = ""
    engine.integrateTaskCommit(commit.id, branch)
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
    if (taskActionMode === "confirm_worktree")
      return "Enter  Create isolated worktree    Esc  Cancel"
    if (taskActionMode === "choose_agent")
      return "c  Start Codex    d  Start Claude    Esc  Cancel"
    if (editingDraft && draftStep === "repository")
      return "h/l or ←/→  Source    j/k or ↑/↓  Repository    Enter  Open or clone    /  Search    p  Path    Esc  Cancel"
    if (editingDraft && draftStep === "path")
      return "Tab  Complete path    Enter  Continue    Esc  Repository browser"
    if (editingDraft) return "Enter  Create draft    Shift+Tab  Repositories    Esc  Cancel"
    if (editingPlanTask) return "Tab  Next field    Enter  Continue or save    Esc  Cancel"
    if (rejectingPlan) return "Enter  Reject plan    Esc  Cancel"
    if (confirmingImplementationCancel)
      return "Enter  Confirm cancellation    Esc  Keep running"
    if (completionDecisionMode === "approve")
      return "Enter  Create inspected local commit    Esc  Keep inspecting"
    if (completionDecisionMode === "reject")
      return "Enter  Preserve rejection    Esc  Keep inspecting"
    if (integrationMode === "edit")
      return "Enter  Review branch integration    Esc  Cancel"
    if (integrationMode === "confirm")
      return "Enter  Fast-forward local branch    Esc  Cancel"
    if (implementationInterventionMode !== "")
      return "Enter  Submit instruction    Esc  Keep current attempt"
    if (focusArea === "sections")
      return "j/k or ↑/↓  Sections    l/→ or Enter  Open    r  Reconnect    Esc  Close"
    if (selectedSection === 1 && currentPlan() && currentPlan().status === "proposed")
      return "h/←  Sections    j/k or ↑/↓  Tasks    Enter/e  Edit    J/K  Reorder    a  Approve    x  Reject"
    if (selectedSection === 1 && currentPlan() && currentPlan().status === "approved")
      return "h/←  Sections    j/k or ↑/↓  Tasks    Enter  Next action    c/d  Choose agent"
    if (selectedSection === 1)
      return "h/←  Sections    c  Plan with Codex    d  Plan with Claude    Esc  Close"
    if (selectedSection === 2 && proposedTaskCommit())
      return "h/←  Sections    j/k or ↑/↓  Scroll diff    a  Approve    x  Reject    Esc  Close"
    if (selectedSection === 2 && createdTaskCommit())
      return "h/←  Sections    j/k or ↑/↓  Scroll diff    i  Integrate local branch    Esc  Close"
    if (selectedSection === 2)
      return "h/←  Sections    p  Prepare inspected result    Esc  Close"
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
      if (method === "approve_task_commit" || method === "reject_task_commit")
        root.completionDecisionMode = ""
      if (method === "integrate_task_commit") root.integrationMode = ""
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
      if (!root.proposedTaskCommit()) root.completionDecisionMode = ""
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
    id: taskActionEngine
    onRequestCompleted: function(method) {
      root.taskActionMode = ""
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
      description: "Inspect the exact changed files, complete patch, and proposed local task commit."
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
          || integrationTargetField.activeFocus
        onMoveRequested: function(dx, dy) {
          root.moveNavigation(dx, dy)
        }
        onActivateRequested: root.activateNavigation()
        onCloseRequested: root.requestClose()
        onDeleteRequested: {
          if (root.taskActionMode !== "" || root.editingDraft
              || root.editingPlanTask || root.rejectingPlan) return
          if (root.selectedSection === 0) root.beginImplementationCancel()
          else if (root.selectedSection === 1) root.beginRejectPlan()
          else if (root.selectedSection === 2) root.beginCompletionDecision("reject")
        }
        onTextKey: function(text) {
          if (root.taskActionMode === "choose_agent") {
            if (text === "c") root.launchTaskImplementation("codex")
            else if (text === "d") root.launchTaskImplementation("claude")
            return
          }
          if (root.taskActionMode === "confirm_worktree") return
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
          else if (root.selectedSection === 1 && text === "c"
              && root.currentPlan() && root.currentPlan().status === "approved") {
            root.launchSelectedTaskImplementation("codex")
          }
          else if (root.selectedSection === 1 && text === "d"
              && root.currentPlan() && root.currentPlan().status === "approved") {
            root.launchSelectedTaskImplementation("claude")
          }
          else if (root.selectedSection === 1 && text === "c") root.generatePlan("codex")
          else if (root.selectedSection === 1 && text === "d") root.generatePlan("claude")
          else if (root.selectedSection === 1 && text === "e") root.beginPlanTaskEdit()
          else if (root.selectedSection === 1 && text === "J") root.moveCurrentTask("down")
          else if (root.selectedSection === 1 && text === "K") root.moveCurrentTask("up")
          else if (root.selectedSection === 1 && text === "a" && root.currentPlan() && root.currentPlan().status === "proposed") engine.approvePlan()
          else if (root.selectedSection === 1 && text === "x") root.beginRejectPlan()
          else if (root.selectedSection === 2 && text === "p") root.finishSelectedTask()
          else if (root.selectedSection === 2 && text === "a") root.beginCompletionDecision("approve")
          else if (root.selectedSection === 2 && text === "x") root.beginCompletionDecision("reject")
          else if (root.selectedSection === 2 && text === "i") root.beginTaskIntegration()
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
                      if (root.selectedSection === 2) {
                        var proposal = root.latestTaskRecord(engine.activeRun ? engine.activeRun.task_commits : [])
                        return proposal ? "Final result " + proposal.status : "Prepare final inspection"
                      }
                      if (root.selectedSection === 3) return "Deterministic verification"
                      if (root.selectedSection === 4) return "Independent review"
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
                        height: {
                          var reserved = root.taskActionMode === ""
                            ? Style.space(115) : Style.space(205)
                          return Math.max(Style.space(90), Math.min(
                            Style.space(270), contentBody.height
                              - contentHeading.height - reserved))
                        }
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
                              visible: root.currentPlan() && (root.currentPlan().status === "approved"
                                || (taskDelegate.modelData.depends_on
                                    && taskDelegate.modelData.depends_on.length > 0))
                              width: parent.width
                              text: (taskDelegate.modelData.depends_on || []).length > 0
                                ? ("Dependencies: tasks "
                                   + (taskDelegate.modelData.depends_on || []).join(", ")
                                   + " • blocked until prerequisite results can be composed")
                                : "Dependencies: none"
                              color: root.mutedForeground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              wrapMode: Text.WordWrap
                            }

                            Text {
                              visible: root.currentPlan() && root.currentPlan().status === "approved"
                              width: parent.width
                              text: {
                                var worktree = root.latestWorktreeForTask(taskDelegate.modelData.id)
                                if (!worktree) return "Worktree: not created"
                                return "Worktree: " + worktree.status + " • " + worktree.branch
                              }
                              color: root.mutedForeground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              wrapMode: Text.WrapAnywhere
                            }

                            Text {
                              visible: root.currentPlan() && root.currentPlan().status === "approved"
                                && !!root.latestWorktreeForTask(taskDelegate.modelData.id)
                              width: parent.width
                              text: {
                                var worktree = root.latestWorktreeForTask(taskDelegate.modelData.id)
                                return worktree ? worktree.path : ""
                              }
                              color: root.mutedForeground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              wrapMode: Text.WrapAnywhere
                            }

                            Text {
                              visible: root.currentPlan() && root.currentPlan().status === "approved"
                              width: parent.width
                              text: {
                                var attempt = root.latestImplementationForTask(taskDelegate.modelData.id)
                                return attempt
                                  ? ("Implementer: " + attempt.agent + " • latest attempt "
                                     + attempt.status + (attempt.paused ? " (paused)" : ""))
                                  : "Implementer: unassigned"
                              }
                              color: root.mutedForeground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              wrapMode: Text.WordWrap
                            }

                            Text {
                              visible: root.currentPlan() && root.currentPlan().status === "approved"
                              width: parent.width
                              text: "Next: " + root.taskActionLabel(taskDelegate.modelData)
                              color: root.accent
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              font.bold: true
                              wrapMode: Text.WordWrap
                            }

                            Text {
                              visible: {
                                var worktree = root.latestWorktreeForTask(taskDelegate.modelData.id)
                                var attempt = root.latestImplementationForTask(taskDelegate.modelData.id)
                                return root.taskActionError(taskDelegate.modelData.id) !== ""
                                  || (worktree && worktree.last_error)
                                  || (attempt && attempt.error_message)
                              }
                              width: parent.width
                              text: {
                                var worktree = root.latestWorktreeForTask(taskDelegate.modelData.id)
                                var attempt = root.latestImplementationForTask(taskDelegate.modelData.id)
                                return root.taskActionError(taskDelegate.modelData.id)
                                  || (worktree ? worktree.last_error : "")
                                  || (attempt ? attempt.error_message : "")
                              }
                              color: root.urgent
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              wrapMode: Text.WordWrap
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
                              if (root.currentPlan() && root.currentPlan().status === "approved")
                                root.beginSelectedTaskAction()
                              else
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

                      Row {
                        visible: root.currentPlan() && root.currentPlan().status === "approved"
                          && root.taskActionMode === ""
                        spacing: Style.space(8)

                        Button {
                          text: {
                            var code = root.taskActionCode(root.selectedTask())
                            if (taskActionEngine.requestPending) return "Working…"
                            if (code === "create_worktree") return "Create worktree"
                            if (code === "choose_agent") return "Choose implementer"
                            if (code === "retry_implementation") return "Retry implementation"
                            if (code === "running") return "View activity"
                            if (code === "finish") return "Prepare final inspection"
                            if (code === "inspect") return "Inspect result"
                            return "No action available"
                          }
                          bordered: true
                          enabled: !taskActionEngine.requestPending
                            && ["create_worktree", "choose_agent", "retry_implementation",
                                "running", "finish", "inspect"].indexOf(
                                  root.taskActionCode(root.selectedTask())) !== -1
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.beginSelectedTaskAction()
                        }
                      }

                      Column {
                        visible: root.taskActionMode !== ""
                        width: parent.width
                        spacing: Style.space(8)

                        Text {
                          width: parent.width
                          text: {
                            var task = root.taskActionTask()
                            if (!task) return "The selected task is no longer available."
                            if (root.taskActionMode === "confirm_worktree") {
                              var run = engine.activeRun
                              var base = run ? String(run.base_revision || "").slice(0, 12) : ""
                              var dirtyNote = run && run.worktree_dirty
                                ? " The primary checkout has uncommitted work that the agent will not see."
                                : ""
                              return "Create an isolated worktree and reserved task branch for "
                                + task.position + ". " + task.title
                                + "? It starts from committed base " + base
                                + ", stays outside the primary checkout, and will not merge or push."
                                + dirtyNote
                            }
                            return "Choose the authenticated CLI that will implement "
                              + task.position + ". " + task.title
                              + ". The engine will supervise it inside the ready task worktree."
                          }
                          color: root.foreground
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          wrapMode: Text.WordWrap
                        }

                        Row {
                          spacing: Style.space(8)

                          Button {
                            visible: root.taskActionMode === "confirm_worktree"
                            text: "Create isolated worktree"
                            bordered: true
                            enabled: !taskActionEngine.requestPending
                            foreground: root.foreground
                            accent: root.accent
                            onClicked: root.confirmTaskWorktree()
                          }

                          Button {
                            visible: root.taskActionMode === "choose_agent"
                            text: "Codex"
                            bordered: true
                            enabled: !taskActionEngine.requestPending
                            foreground: root.foreground
                            accent: root.accent
                            onClicked: root.launchTaskImplementation("codex")
                          }

                          Button {
                            visible: root.taskActionMode === "choose_agent"
                            text: "Claude"
                            enabled: !taskActionEngine.requestPending
                            foreground: root.foreground
                            accent: root.accent
                            onClicked: root.launchTaskImplementation("claude")
                          }

                          Button {
                            text: "Cancel"
                            enabled: !taskActionEngine.requestPending
                            foreground: root.foreground
                            accent: root.accent
                            onClicked: root.cancelTaskAction()
                          }
                        }
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
                    id: changesSection
                    visible: root.selectedSection === 2
                    width: parent.width
                    height: Math.max(0, contentBody.height - contentHeading.height
                      - Style.space(56))
                    spacing: Style.space(8)
                    property var proposal: root.latestTaskRecord(
                      engine.activeRun ? engine.activeRun.task_commits : [])
                    property var verification: root.recordById(
                      engine.activeRun ? engine.activeRun.verification_attempts : [],
                      proposal ? proposal.verification_attempt_id : "")
                    property var review: root.recordById(
                      engine.activeRun ? engine.activeRun.review_attempts : [],
                      proposal ? proposal.review_attempt_id : "")

                    Button {
                      id: prepareTaskButton
                      visible: !changesSection.proposal
                        || changesSection.proposal.status === "rejected"
                        || changesSection.proposal.status === "stale"
                        || changesSection.proposal.status === "failed"
                      text: engine.requestPending && engine.pendingMethod === "finish_task"
                        ? "Preparing…" : "Prepare selected task"
                      bordered: true
                      enabled: root.finishContext() !== null && !engine.requestPending
                      foreground: root.foreground
                      accent: root.accent
                      onClicked: root.finishSelectedTask()
                    }

                    Flickable {
                      id: changesFlickable
                      width: parent.width
                      height: Math.max(0, parent.height - (prepareTaskButton.visible
                        ? prepareTaskButton.height + parent.spacing : 0))
                      contentWidth: width
                      contentHeight: changesDocument.height
                      boundsBehavior: Flickable.StopAtBounds
                      clip: true

                      Column {
                        id: changesDocument
                        width: changesFlickable.width
                        spacing: Style.space(9)

                        Text {
                          width: parent.width
                          text: changesSection.proposal
                            ? "Status: " + changesSection.proposal.status
                            : "No inspected result is ready. Prepare the selected completed task to run verification, independent review, and final change capture."
                          color: changesSection.proposal
                            && (changesSection.proposal.status === "proposed"
                              || changesSection.proposal.status === "created")
                            ? root.accent : root.mutedForeground
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          font.bold: changesSection.proposal !== null
                          wrapMode: Text.WordWrap
                        }

                        Text {
                          visible: changesSection.proposal !== null
                          width: parent.width
                          text: changesSection.proposal
                            ? "Proposed commit: " + changesSection.proposal.message : ""
                          color: root.foreground
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          font.bold: true
                          wrapMode: Text.WordWrap
                        }

                        Text {
                          visible: changesSection.proposal !== null
                          width: parent.width
                          text: {
                            if (!changesSection.proposal) return ""
                            return "Evidence: verification "
                              + (changesSection.verification
                                ? changesSection.verification.status : "missing")
                              + " · review " + (changesSection.review
                                ? changesSection.review.status : "missing")
                          }
                          color: root.mutedForeground
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          wrapMode: Text.WordWrap
                        }

                        Repeater {
                          model: changesSection.review && changesSection.review.result
                            ? changesSection.review.result.findings || [] : []
                          Text {
                            required property var modelData
                            width: changesDocument.width
                            text: "Review " + modelData.severity + ": "
                              + modelData.summary + " — " + modelData.evidence
                            color: modelData.severity === "minor"
                              ? root.mutedForeground : root.urgent
                            font.family: root.fontFamily
                            font.pixelSize: Style.font.bodySmall
                            wrapMode: Text.WordWrap
                          }
                        }

                        Text {
                          visible: changesSection.proposal
                            && changesSection.proposal.commit_hash
                          width: parent.width
                          text: changesSection.proposal && changesSection.proposal.commit_hash
                            ? "Local commit: " + changesSection.proposal.commit_hash : ""
                          color: root.accent
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          wrapMode: Text.WrapAnywhere
                        }

                        Text {
                          visible: root.latestTaskIntegration() !== null
                          width: parent.width
                          text: {
                            var integration = root.latestTaskIntegration()
                            if (!integration) return ""
                            return "Integration: " + integration.status + " · "
                              + integration.target_branch
                              + (integration.result_head
                                ? " · " + integration.result_head.slice(0, 12) : "")
                              + (integration.error_message
                                ? " — " + integration.error_message : "")
                          }
                          color: root.latestTaskIntegration()
                            && root.latestTaskIntegration().status === "completed"
                            ? root.accent : root.urgent
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          wrapMode: Text.WordWrap
                        }

                        Button {
                          visible: changesSection.proposal
                            && changesSection.proposal.status === "created"
                            && root.integrationMode === ""
                          text: "Integrate local branch"
                          bordered: true
                          enabled: !engine.requestPending
                          foreground: root.foreground
                          accent: root.accent
                          onClicked: root.beginTaskIntegration()
                        }

                        Column {
                          visible: root.integrationMode === "edit"
                          width: parent.width
                          spacing: Style.space(7)
                          Text {
                            width: parent.width
                            text: "Select a local branch that is checked out in a clean worktree. Forge will only fast-forward it; unowned branches, divergent history, and dirty worktrees are refused."
                            color: root.mutedForeground
                            font.family: root.fontFamily
                            font.pixelSize: Style.font.bodySmall
                            wrapMode: Text.WordWrap
                          }
                          TextField {
                            id: integrationTargetField
                            width: parent.width
                            placeholderText: "Local branch (for example, main)"
                            Keys.onPressed: function(event) {
                              if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                                root.reviewTaskIntegration()
                                event.accepted = true
                              } else if (event.key === Qt.Key_Escape) {
                                root.integrationMode = ""
                                keyCatcher.forceActiveFocus()
                                event.accepted = true
                              }
                            }
                          }
                          Row {
                            spacing: Style.space(8)
                            Button {
                              text: "Review integration"
                              bordered: true
                              enabled: integrationTargetField.text.trim() !== ""
                                && !engine.requestPending
                              foreground: root.foreground
                              accent: root.accent
                              onClicked: root.reviewTaskIntegration()
                            }
                            Button {
                              text: "Cancel"
                              enabled: !engine.requestPending
                              foreground: root.foreground
                              accent: root.accent
                              onClicked: {
                                root.integrationMode = ""
                                keyCatcher.forceActiveFocus()
                              }
                            }
                          }
                        }

                        Rectangle {
                          visible: root.integrationMode === "confirm"
                          width: parent.width
                          height: integrationDecisionColumn.implicitHeight + Style.space(20)
                          radius: Style.cornerRadius
                          color: Qt.rgba(root.urgent.r, root.urgent.g, root.urgent.b, 0.08)
                          border.width: 1
                          border.color: root.urgent

                          Column {
                            id: integrationDecisionColumn
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            anchors.margins: Style.space(10)
                            spacing: Style.space(7)
                            Text {
                              width: parent.width
                              text: "Fast-forward local branch “" + integrationTargetField.text.trim()
                                + "” to commit " + (root.createdTaskCommit()
                                  ? root.createdTaskCommit().commit_hash.slice(0, 12) : "")
                                + "? This updates that local branch and its clean checked-out files. It will not merge divergent history, push, deploy, or delete the task worktree."
                              color: root.foreground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              wrapMode: Text.WordWrap
                            }
                            Row {
                              spacing: Style.space(8)
                              Button {
                                text: "Confirm fast-forward"
                                bordered: true
                                enabled: !engine.requestPending
                                foreground: root.foreground
                                accent: root.accent
                                onClicked: root.confirmTaskIntegration()
                              }
                              Button {
                                text: "Cancel"
                                enabled: !engine.requestPending
                                foreground: root.foreground
                                accent: root.accent
                                onClicked: root.integrationMode = ""
                              }
                            }
                          }
                        }

                        Text {
                          visible: changesSection.proposal
                            && changesSection.proposal.decision_reason
                          width: parent.width
                          text: changesSection.proposal && changesSection.proposal.decision_reason
                            ? "Decision: " + changesSection.proposal.decision_reason : ""
                          color: root.mutedForeground
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          wrapMode: Text.WordWrap
                        }

                        Text {
                          visible: changesSection.proposal !== null
                          text: changesSection.proposal
                            ? "Changed files (" + (changesSection.proposal.changed_files || []).length + ")"
                            : ""
                          color: root.foreground
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          font.bold: true
                        }

                        Repeater {
                          model: changesSection.proposal
                            ? changesSection.proposal.changed_files || [] : []
                          Text {
                            required property var modelData
                            width: changesDocument.width
                            text: modelData.status + " · "
                              + (modelData.previous_path
                                ? modelData.previous_path + " → " : "")
                              + modelData.path
                            color: root.mutedForeground
                            font.family: root.fontFamily
                            font.pixelSize: Style.font.bodySmall
                            wrapMode: Text.WrapAnywhere
                          }
                        }

                        Row {
                          visible: changesSection.proposal
                            && changesSection.proposal.status === "proposed"
                            && root.completionDecisionMode === ""
                          spacing: Style.space(8)
                          Button {
                            text: "Approve local commit"
                            bordered: true
                            enabled: !engine.requestPending
                            foreground: root.foreground
                            accent: root.accent
                            onClicked: root.beginCompletionDecision("approve")
                          }
                          Button {
                            text: "Reject result"
                            enabled: !engine.requestPending
                            foreground: root.foreground
                            accent: root.accent
                            onClicked: root.beginCompletionDecision("reject")
                          }
                        }

                        Rectangle {
                          visible: root.completionDecisionMode !== ""
                          width: parent.width
                          height: decisionColumn.implicitHeight + Style.space(20)
                          radius: Style.cornerRadius
                          color: Qt.rgba(root.urgent.r, root.urgent.g, root.urgent.b, 0.08)
                          border.width: 1
                          border.color: root.completionDecisionMode === "approve"
                            ? root.accent : root.urgent

                          Column {
                            id: decisionColumn
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.verticalCenter: parent.verticalCenter
                            anchors.margins: Style.space(10)
                            spacing: Style.space(7)
                            Text {
                              width: parent.width
                              text: root.completionDecisionMode === "approve"
                                ? "Create exactly this inspected local commit in the reserved task branch? This will not merge, push, retire the worktree, or touch the primary checkout."
                                : "Reject this inspected result? The proposal, worktree, branch, and changes will remain available in durable history."
                              color: root.foreground
                              font.family: root.fontFamily
                              font.pixelSize: Style.font.bodySmall
                              wrapMode: Text.WordWrap
                            }
                            Row {
                              spacing: Style.space(8)
                              Button {
                                text: root.completionDecisionMode === "approve"
                                  ? "Confirm commit" : "Confirm rejection"
                                bordered: true
                                enabled: !engine.requestPending
                                foreground: root.foreground
                                accent: root.accent
                                onClicked: root.confirmCompletionDecision()
                              }
                              Button {
                                text: "Cancel"
                                enabled: !engine.requestPending
                                foreground: root.foreground
                                accent: root.accent
                                onClicked: root.completionDecisionMode = ""
                              }
                            }
                          }
                        }

                        Text {
                          visible: changesSection.proposal !== null
                          width: parent.width
                          text: "Complete patch"
                          color: root.foreground
                          font.family: root.fontFamily
                          font.pixelSize: Style.font.bodySmall
                          font.bold: true
                        }

                        Rectangle {
                          visible: changesSection.proposal !== null
                          width: parent.width
                          height: patchText.implicitHeight + Style.space(16)
                          radius: Style.cornerRadius
                          color: root.background

                          Text {
                            id: patchText
                            anchors.left: parent.left
                            anchors.right: parent.right
                            anchors.top: parent.top
                            anchors.margins: Style.space(8)
                            text: changesSection.proposal
                              ? changesSection.proposal.patch || "(empty patch)" : ""
                            color: root.foreground
                            font.family: root.fontFamily
                            font.pixelSize: Style.font.bodySmall
                            wrapMode: Text.WrapAnywhere
                          }
                        }
                      }
                    }
                  }

                  Column {
                    visible: root.selectedSection === 3
                    width: parent.width
                    spacing: Style.space(10)
                    property var verification: root.latestTaskRecord(engine.activeRun ? engine.activeRun.verification_attempts : [])

                    Button {
                      text: engine.requestPending && engine.pendingMethod === "finish_task" ? "Preparing…" : "Prepare selected task"
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
                      text: engine.requestPending && engine.pendingMethod === "finish_task" ? "Reviewing…" : "Prepare selected task"
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
