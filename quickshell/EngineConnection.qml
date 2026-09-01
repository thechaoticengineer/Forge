import QtQuick
import Quickshell
import Quickshell.Io

Item {
  id: root

  width: 0
  height: 0
  visible: false

  readonly property string runtimeDirectory: Quickshell.env("XDG_RUNTIME_DIR") || ""
  readonly property string socketPath: runtimeDirectory === ""
    ? ""
    : runtimeDirectory + "/omarchy-ai-build-orchestrator/engine.sock"

  readonly property bool connected: socket.connected
  property bool hasSnapshot: false
  property int sequence: 0
  property string engineStatus: "offline"
  property var activeRun: null
  property bool requiresAttention: false
  property string lastError: ""
  property bool requestPending: false
  property string pendingRequestId: ""
  property string pendingMethod: ""
  property string requestError: ""
  property int requestSequence: 0
  property var repositoryCatalog: ({
    project_roots: [],
    local: [],
    local_error: "",
    github: [],
    github_error: ""
  })

  signal snapshotChanged()
  signal draftCreated()
  signal repositoryPathCompleted(string replacement, var candidates)
  signal repositoryCloned(string nameWithOwner, string path)
  signal requestCompleted(string method)

  function reconnect() {
    if (socketPath === "") {
      lastError = "XDG_RUNTIME_DIR is not available"
      return
    }

    requestError = ""
    socket.connected = false
    Qt.callLater(function() { socket.connected = true })
  }

  function abandonRequest() {
    requestPending = false
    pendingRequestId = ""
    pendingMethod = ""
    requestError = ""
    socket.connected = false
    Qt.callLater(function() { socket.connected = true })
  }

  function createDraft(repository, goal) {
    return sendRequest("create_draft_run", {
      repository: String(repository || ""),
      goal: String(goal || "")
    })
  }

  function listRepositories() {
    return sendRequest("list_repositories", {})
  }

  function cloneRepository(nameWithOwner) {
    return sendRequest("clone_repository", {
      name_with_owner: String(nameWithOwner || "")
    })
  }

  function completeRepositoryPath(path) {
    return sendRequest("complete_repository_path", {
      path: String(path || "")
    })
  }

  function generatePlan(agent) {
    if (!activeRun) return false
    return sendRequest("generate_plan", {
      run_id: activeRun.id,
      agent: String(agent || "")
    })
  }

  function updatePlanTask(taskId, title, description, acceptanceCriteria) {
    if (!activeRun || !activeRun.plan) return false
    return sendRequest("update_plan_task", {
      run_id: activeRun.id,
      plan_id: activeRun.plan.id,
      task_id: taskId,
      title: String(title || ""),
      description: String(description || ""),
      acceptance_criteria: acceptanceCriteria || []
    })
  }

  function movePlanTask(taskId, direction) {
    if (!activeRun || !activeRun.plan) return false
    return sendRequest("move_plan_task", {
      run_id: activeRun.id,
      plan_id: activeRun.plan.id,
      task_id: taskId,
      direction: direction
    })
  }

  function approvePlan() {
    if (!activeRun || !activeRun.plan) return false
    return sendRequest("approve_plan", {
      run_id: activeRun.id,
      plan_id: activeRun.plan.id
    })
  }

  function rejectPlan(reason) {
    if (!activeRun || !activeRun.plan) return false
    return sendRequest("reject_plan", {
      run_id: activeRun.id,
      plan_id: activeRun.plan.id,
      reason: String(reason || "")
    })
  }

  function createTaskWorktree(runId, planId, taskId) {
    return sendRequest("create_task_worktree", {
      run_id: String(runId || ""),
      plan_id: String(planId || ""),
      task_id: String(taskId || "")
    })
  }

  function runTaskImplementation(runId, planId, taskId, worktreeId, agent) {
    return sendRequest("run_task_implementation", {
      run_id: String(runId || ""),
      plan_id: String(planId || ""),
      task_id: String(taskId || ""),
      worktree_id: String(worktreeId || ""),
      agent: String(agent || "")
    })
  }

  function cancelImplementation(runId, attemptId) {
    return sendRequest("cancel_task_implementation", {
      run_id: String(runId || ""),
      attempt_id: String(attemptId || "")
    })
  }

  function pauseImplementation(runId, attemptId) {
    return sendRequest("pause_task_implementation", {
      run_id: String(runId || ""),
      attempt_id: String(attemptId || "")
    })
  }

  function resumeImplementation(runId, attemptId) {
    return sendRequest("resume_task_implementation", {
      run_id: String(runId || ""),
      attempt_id: String(attemptId || "")
    })
  }

  function continueImplementation(runId, attemptId, kind, instruction) {
    return sendRequest("continue_task_implementation", {
      run_id: String(runId || ""),
      attempt_id: String(attemptId || ""),
      kind: String(kind || ""),
      instruction: String(instruction || "")
    })
  }

  function finishTask(planId, taskId, worktreeId, implementationAttemptId) {
    if (!activeRun) return false
    return sendRequest("finish_task", {
      run_id: activeRun.id,
      plan_id: String(planId || ""),
      task_id: String(taskId || ""),
      worktree_id: String(worktreeId || ""),
      implementation_attempt_id: String(implementationAttemptId || ""),
      policy: "cross_provider_or_fresh_session",
      max_corrections: 1
    })
  }

  function approveTaskCommit(taskCommitId) {
    if (!activeRun) return false
    return sendRequest("approve_task_commit", {
      run_id: activeRun.id,
      task_commit_id: String(taskCommitId || "")
    })
  }

  function rejectTaskCommit(taskCommitId, reason) {
    if (!activeRun) return false
    return sendRequest("reject_task_commit", {
      run_id: activeRun.id,
      task_commit_id: String(taskCommitId || ""),
      reason: String(reason || "")
    })
  }

  function sendRequest(method, payload) {
    if (!socket.connected) {
      requestError = "The orchestration engine is not connected"
      return false
    }
    if (requestPending) return false

    requestSequence++
    pendingRequestId = "qml-" + Date.now() + "-" + requestSequence
    pendingMethod = method
    requestPending = true
    requestError = ""
    var message = {
      version: 2,
      request_id: pendingRequestId,
      method: method
    }
    for (var key in payload) message[key] = payload[key]
    socket.write(JSON.stringify(message) + "\n")
    socket.flush()
    return true
  }

  function acceptMessage(line) {
    var message
    try {
      message = JSON.parse(String(line))
    } catch (error) {
      lastError = "The engine sent invalid JSON"
      return
    }

    if (!message || message.version !== 2) {
      lastError = "The engine uses an unsupported protocol version"
      return
    }

    if (message.type === "error") {
      if (message.request_id && message.request_id === pendingRequestId) {
        requestPending = false
        pendingRequestId = ""
        pendingMethod = ""
        requestError = String(message.message || "The engine rejected the request")
      } else if (!message.request_id && requestPending) {
        requestPending = false
        pendingRequestId = ""
        pendingMethod = ""
        requestError = String(message.message || "The engine rejected the request")
      } else {
        lastError = String(message.message || "The engine rejected a request")
      }
      return
    }

    if (message.type === "path_completion") {
      if (message.request_id && message.request_id === pendingRequestId) {
        var completedMethod = pendingMethod
        requestPending = false
        pendingRequestId = ""
        pendingMethod = ""
        requestError = ""
        repositoryPathCompleted(
          String(message.replacement || ""),
          message.candidates || []
        )
        requestCompleted(completedMethod)
      }
      return
    }

    if (message.type === "repository_catalog") {
      if (message.request_id && message.request_id === pendingRequestId) {
        var completedMethod = pendingMethod
        requestPending = false
        pendingRequestId = ""
        pendingMethod = ""
        requestError = ""
        repositoryCatalog = message.catalog || {
          project_roots: [], local: [], local_error: "", github: [], github_error: ""
        }
        requestCompleted(completedMethod)
      }
      return
    }

    if (message.type === "repository_cloned") {
      if (message.request_id && message.request_id === pendingRequestId) {
        var completedMethod = pendingMethod
        requestPending = false
        pendingRequestId = ""
        pendingMethod = ""
        requestError = ""
        repositoryCloned(
          String(message.name_with_owner || ""),
          String(message.path || "")
        )
        requestCompleted(completedMethod)
      }
      return
    }

    if (message.type !== "snapshot" || !message.snapshot) return

    var snapshot = message.snapshot
    var validStatuses = [
      "idle",
      "running",
      "blocked",
      "failed",
      "completed",
      "waiting_for_user"
    ]
    if (validStatuses.indexOf(snapshot.status) === -1) {
      lastError = "The engine sent an unknown status"
      return
    }

    sequence = Number(snapshot.sequence || 0)
    engineStatus = snapshot.status
    activeRun = snapshot.active_run || null
    requiresAttention = snapshot.requires_attention === true
    hasSnapshot = true
    lastError = ""
    if (message.request_id && message.request_id === pendingRequestId) {
      var completedMethod = pendingMethod
      requestPending = false
      pendingRequestId = ""
      pendingMethod = ""
      requestError = ""
      if (completedMethod === "create_draft_run") draftCreated()
      requestCompleted(completedMethod)
    }
    snapshotChanged()
  }

  Socket {
    id: socket
    path: root.socketPath
    connected: root.socketPath !== ""

    parser: SplitParser {
      splitMarker: "\n"
      onRead: function(line) { root.acceptMessage(line) }
    }

    onConnectionStateChanged: {
      if (connected) {
        retryTimer.stop()
        root.lastError = ""
      } else {
        root.engineStatus = "offline"
        root.lastError = "The orchestration engine is not available"
        if (root.requestPending) {
          root.requestPending = false
          root.pendingRequestId = ""
          root.pendingMethod = ""
          root.requestError = "Connection lost before the engine responded"
        }
        retryTimer.restart()
      }
    }
  }

  Timer {
    id: retryTimer
    interval: 2000
    repeat: false
    onTriggered: root.reconnect()
  }
}
