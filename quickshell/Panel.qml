pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

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
  property int selectedStageIndex: -1
  property string lastProject: ""
  property int projectViewRevision: 0
  property var goalDrafts: ({})
  property bool revisePending: false
  property bool chatPending: false
  property bool chatExpanded: false
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
  readonly property bool queueActive: engineState !== null && engineState.queue_active === true
  readonly property bool hasQueuedGoals: queue.some(function(item) { return item.status === "queued" })
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

  property bool chooserOpen: false
  property var projectsData: null
  property bool manualEntry: false

  property bool diffOpen: false
  property string diffText: ""
  property bool diffPending: false
  property string diffError: ""

  property bool helpOpen: false
  readonly property bool insertMode: goalField.activeFocus
    || feedbackField.activeFocus || questionField.activeFocus
    || filterField.activeFocus || manualField.activeFocus
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
    if (chooserOpen) chooserList.resetSelection()
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
  property string agentLog: ""
  property double agentLogOffset: 0
  property string logSession: ""
  property bool agentLogPending: false

  onEngineStateChanged: {
    const project = engineState ? engineState.project : ""
    if (project === lastProject) return
    if (lastProject !== "") goalDrafts[lastProject] = goalField.text
    lastProject = project
    // Ignore log/diff responses from an earlier visit, even after switching back.
    projectViewRevision++
    cancelPlanEdit()
    goalField.text = goalDrafts[project] || ""
    feedbackField.text = ""
    revisePending = false
    questionField.text = ""
    chatPending = false
    chatExpanded = false
    chatList.followTail = true
    chatList.readingY = 0
    goalFlick.contentY = 0
    expandedStageId = -1
    selectedStageIndex = -1
    diffOpen = false
    diffText = ""
    diffError = ""
    diffPending = false
    localError = ""
    agentLog = ""
    agentLogOffset = 0
    agentLogPending = false
    logSession = agentSession
    liveOutput.followTail = true
    liveTab = busy
    historyFilter = "all"
    historyList.positionViewAtBeginning()
    selectedReportKey = ""
    expandedReportKey = ""
    reportList.readingY = 0
    reportList.positionViewAtBeginning()
  }

  onBusyChanged: {
    liveTab = busy
    if (!busy) chatPending = false
  }
  onAgentSessionChanged: {
    if (agentSession !== "" && agentSession !== logSession) {
      logSession = agentSession
      agentLog = ""
      agentLogOffset = 0
      liveOutput.followTail = true
    }
    agentNow = Date.now() / 1000
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

  function api(method, path, body, done) {
    const xhr = new XMLHttpRequest()
    xhr.open(method, apiBase + path)
    xhr.setRequestHeader("Content-Type", "application/json")
    xhr.onreadystatechange = function() {
      if (xhr.readyState !== XMLHttpRequest.DONE) return
      if (xhr.status === 0) {
        root.engineOnline = false
        if (done) done(null, xhr.status)
        return
      }
      root.engineOnline = true
      let parsed = null
      try { parsed = JSON.parse(xhr.responseText) } catch (e) {}
      if (parsed && parsed.error) root.localError = parsed.error
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
        root.engineState = resp
      }
      if (done) done()
    })
  }

  function refreshAgentLog() {
    if (!window.visible || !busy || agentLogPending) return
    agentLogPending = true
    const session = logSession
    const revision = projectViewRevision
    api("GET", "/api/agent_log?offset=" + agentLogOffset
        + "&project=" + encodeURIComponent(lastProject), null, function(resp) {
      if (revision !== root.projectViewRevision) return
      root.agentLogPending = false
      if (session !== root.logSession || !resp
          || typeof resp.log !== "string" || typeof resp.size !== "number") return
      // Offsets are bytes supplied by the engine, not JavaScript string lengths.
      if (resp.size < root.agentLogOffset) {
        liveOutput.followTail = true
        root.agentLog = resp.log
      } else {
        root.agentLog += resp.log
      }
      root.agentLogOffset = resp.size
    })
  }

  function act(path, body, done) {
    localError = ""
    api("POST", path, body || {}, function(resp, status) {
      root.refresh(function() { if (done) done(resp, status) })
    })
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

  function beginPlanEdit() {
    if (!editPlanButton.enabled) return
    editStages = JSON.parse(JSON.stringify(plan.stages))
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

  function editableNeighbor(index, direction) {
    for (let target = index + direction; target >= 0 && target < editStages.length;
         target += direction) {
      if (editStages[target].status !== "committed") return target
    }
    return -1
  }

  function moveEditStage(index, direction) {
    if (editPending || editStages[index].status === "committed") return
    const target = editableNeighbor(index, direction)
    if (target < 0) return
    keyHandler.forceActiveFocus()
    const stages = editStages.slice()
    const stage = stages[index]
    stages[index] = stages[target]
    stages[target] = stage
    editStages = stages
    keyHandler.selectStage(target)
  }

  function deleteEditStage(index) {
    if (editPending || editStages[index].status === "committed") return
    keyHandler.forceActiveFocus()
    const stages = editStages.slice()
    stages.splice(index, 1)
    editStages = stages
    selectedStageIndex = -1
    keyHandler.selectStage(Math.min(index, stages.length - 1))
  }

  function addEditStage() {
    if (editPending) return
    keyHandler.forceActiveFocus()
    editStages = editStages.concat([{ title: "", instructions: "", acceptance: "", commit: "" }])
    keyHandler.selectStage(editStages.length - 1)
  }

  function savePlanEdit() {
    if (!savePlanButton.enabled) return
    keyHandler.forceActiveFocus()
    const session = editSession
    const stages = editStages.map(function(stage) {
      const content = { title: stage.title, instructions: stage.instructions,
        acceptance: stage.acceptance, commit: stage.commit }
      if (stage.id !== undefined) content.id = stage.id
      return content
    })
    editPending = true
    act("/api/plan/edit", { project: editProject, plan: { goal: editGoal, stages: stages } }, function(resp) {
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
    diffList.positionViewAtBeginning()
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

  function nonNegativeInt(value) {
    return typeof value === "number" && isFinite(value) && value >= 0
      ? Math.floor(value) : null
  }

  function formatTokens(value) {
    const count = nonNegativeInt(value)
    if (count === null) return "—"
    if (count >= 1000000) return (count / 1000000).toFixed(1) + "M"
    if (count >= 1000) return (count / 1000).toFixed(1) + "k"
    return String(count)
  }

  function usageTools(usage) {
    return usage && typeof usage === "object" ? Object.keys(usage).sort().filter(function(tool) {
      return usage[tool] && typeof usage[tool] === "object"
    }) : []
  }

  function usageSummary(usage) {
    return usageTools(usage).map(function(tool) {
      return tool + " " + formatTokens(usage[tool].total_tokens) + " tok"
    }).join(" · ")
  }

  function modelSummary(models) {
    return models && typeof models === "object" ? Object.keys(models).sort().map(function(model) {
      return model + " " + formatTokens(models[model])
    }).join(" · ") : ""
  }

  function usageBreakdown(usage) {
    return usageTools(usage).map(function(tool) {
      const item = usage[tool]
      const models = modelSummary(item.models)
      // Keep exact counts in details; only the summary and model list are compact.
      function exact(value) { const count = nonNegativeInt(value); return count === null ? "—" : String(count) }
      return tool + ": input " + exact(item.input_tokens) + " · output " + exact(item.output_tokens)
        + " · total " + exact(item.total_tokens) + " tok · calls " + exact(item.calls)
        + (models ? "\nmodels: " + models : "")
    }).join("\n")
  }

  function reportKey(report, index) {
    return JSON.stringify([report.unix, report.goal, index])
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
    const requests = reviewStrings(verdict.issues).concat(reviewStrings(verdict.notes))
      .filter(function(request, index, all) { return all.indexOf(request) === index })
    const clean = verdict.approved === true && requests.length === 0
    const label = clean ? "approved"
      : verdict.approved === true ? "legacy approval with change requests" : "changes requested"
    return { clean: clean, label: label + (clean ? "" : " · " + requests.length
      + (requests.length === 1 ? " request" : " requests")) }
  }

  function reviewRoundLabel(entry) {
    const round = nonNegativeInt(entry.round)
    return round !== null && round > 0 ? "round " + round : "round unknown"
  }

  function reviewTimestamp(verdict) {
    const unix = nonNegativeInt(verdict.unix)
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

  function reportTime(report, now) {
    if (typeof report.unix !== "number" || !isFinite(report.unix)) return "—"
    const seconds = Math.max(0, Math.floor(now - report.unix))
    if (seconds < 60) return "just now"
    if (seconds < 3600) return Math.floor(seconds / 60) + "m ago"
    if (seconds < 86400) return Math.floor(seconds / 3600) + "h ago"
    return Math.floor(seconds / 86400) + "d ago"
  }

  function reportDuration(seconds) {
    const count = nonNegativeInt(seconds)
    return count === null ? "—" : Math.floor(count / 60) + "m " + (count % 60) + "s"
  }

  function reportCommits(commits) {
    const lines = []
    // ListView can expose nested arrays as QML sequences, for which isArray is false.
    if (commits && typeof commits.length === "number") {
      for (let i = 0; i < commits.length; i++) {
        const commit = commits[i]
        if (!commit || !(commit.sha || commit.message || commit.title)) continue
        lines.push(((commit.sha || "—") + " " + (commit.message || commit.title || "")).trim())
      }
    }
    return lines.join("\n")
  }

  function historyRows(history, filter) {
    const rows = []
    const entries = history || []
    let previousGoal = ""
    entries.forEach(function(entry) {
      const matches = filter === "all"
        || (filter === "runs"
            && ["run", "stage", "plan", "queue", "update"].indexOf(entry.kind) !== -1)
        || (filter === "git" && entry.kind === "git")
        || (filter === "reviews" && (entry.kind === "review" || entry.kind === "check"))
        || (filter === "errors" && entry.kind === "error")
      if (!matches) return
      if (entry.goal && entry.goal !== previousGoal)
        rows.push({ kind: "sep", goal: entry.goal })
      rows.push({ kind: "entry", event: entry })
      previousGoal = entry.goal || ""
    })
    return rows
  }

  function historyTime(entry, now) {
    let prefix = ""
    if (typeof entry.unix === "number") {
      const date = new Date(entry.unix * 1000)
      const today = new Date(now * 1000)
      const day = new Date(date.getFullYear(), date.getMonth(), date.getDate())
      const todayStart = new Date(today.getFullYear(), today.getMonth(), today.getDate())
      if (day < todayStart)
        prefix = (date.getMonth() < 9 ? "0" : "") + (date.getMonth() + 1)
          + "-" + (date.getDate() < 10 ? "0" : "") + date.getDate() + " "
    }
    return prefix + entry.t
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
      if (manualEntry) manualField.forceActiveFocus()
      else keyHandler.forceActiveFocus()
    }
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
    running: window.visible && root.busy
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
          stageList.positionViewAtIndex(root.selectedStageIndex, ListView.Contain)
        }

        function selectReport(index) {
          if (root.reports.length === 0) return
          const selected = Math.max(0, Math.min(index, root.reports.length - 1))
          root.selectedReportKey = root.reportKey(root.reports[selected], selected)
          reportList.positionViewAtIndex(selected, ListView.Contain)
          reportList.readingY = reportList.contentY
        }

        function scrollOutput(direction) {
          const view = root.liveTab ? liveOutput : root.reportsVisible ? reportList : historyList
          view.cancelFlick()
          if (view !== reportList) view.followTail = false
          const top = view.originY
          const bottom = top + Math.max(0, view.contentHeight - view.height)
          view.contentY = Math.max(top, Math.min(bottom,
            view.contentY + direction * view.height / 2))
          if (view !== reportList && direction > 0 && view.contentY >= bottom) {
            view.followTail = true
            view.scrollToTail()
          }
          if (view === historyList) historyList.readingY = view.contentY
          if (view === reportList) reportList.readingY = view.contentY
        }

        function scrollDiff(amount) {
          diffList.cancelFlick()
          const top = diffList.originY
          const bottom = top + Math.max(0, diffList.contentHeight - diffList.height)
          diffList.contentY = Math.max(top, Math.min(bottom, diffList.contentY + amount))
        }

        Keys.onPressed: event => {
          const prefix = pendingKey
          pendingKey = ""
          const question = (event.key === Qt.Key_Question
            && (event.modifiers === Qt.NoModifier || event.modifiers === Qt.ShiftModifier))
            || (event.key === Qt.Key_Slash && event.modifiers === Qt.ShiftModifier)
            || (event.key === Qt.Key_F1 && event.modifiers === Qt.NoModifier)
          // Modal normal mode owns every key; focused text fields handle insert mode.
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
                diffList.cancelFlick()
                diffList.positionViewAtEnd()
              } else if (event.key === Qt.Key_R && !root.diffPending) {
                root.refreshDiff()
              }
            } else if (event.modifiers === Qt.ControlModifier) {
              if (event.key === Qt.Key_D || event.key === Qt.Key_U)
                scrollDiff((event.key === Qt.Key_D ? 1 : -1) * diffList.height / 2)
            } else if (event.modifiers === Qt.NoModifier) {
              if (event.key === Qt.Key_Q) root.diffOpen = false
              else if (event.key === Qt.Key_J || event.key === Qt.Key_K)
                scrollDiff(event.key === Qt.Key_J ? 40 : -40)
              else if (event.key === Qt.Key_G) {
                if (prefix === "g") {
                  diffList.cancelFlick()
                  diffList.positionViewAtBeginning()
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
                chooserList.moveSelection(event.key === Qt.Key_J ? 1 : -1)
              else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter)
                chooserList.activateSelection()
              else if (event.key === Qt.Key_Slash || event.key === Qt.Key_I)
                filterField.forceActiveFocus()
            }
          } else if (question) {
            root.helpOpen = true
            event.accepted = true
          } else if (event.key === Qt.Key_I && event.modifiers === Qt.NoModifier) {
            goalField.forceActiveFocus()
            event.accepted = true
          } else if (event.key === Qt.Key_I && event.modifiers === Qt.ShiftModifier) {
            feedbackField.forceActiveFocus()
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
            } else if (event.modifiers === Qt.ControlModifier
                       && (event.key === Qt.Key_D || event.key === Qt.Key_U)) {
              scrollOutput(event.key === Qt.Key_D ? 1 : -1)
              event.accepted = true
            } else if (event.modifiers === Qt.NoModifier) {
              // Keep action guards identical to their PanelButton.enabled bindings.
              if (event.key === Qt.Key_P) {
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
                  const selected = root.reports.findIndex(function(report, index) {
                    return root.reportKey(report, index) === root.selectedReportKey
                  })
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
                  if (!root.reports.some(function(report, index) {
                    return root.reportKey(report, index) === root.selectedReportKey
                  })) selectReport(0)
                  root.expandedReportKey = root.expandedReportKey === root.selectedReportKey
                    ? "" : root.selectedReportKey
                } else if (root.selectedStageIndex >= 0 && root.selectedStageIndex < stages.length) {
                  const row = stageList.itemAtIndex(root.selectedStageIndex)
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
      }

      Column {
        anchors.fill: parent
        anchors.margins: Style.space(16)
        anchors.bottomMargin: Style.space(16) + keyboardHint.height + Style.space(10)
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
            Text {
              width: parent.width
              text: root.engineState ? root.engineState.goal : ""
              textFormat: Text.PlainText
              color: root.foreground
              wrapMode: Text.Wrap
              maximumLineCount: 2
              elide: Text.ElideRight
              font.family: root.fontFamily
              font.pixelSize: root.fs(12)
              font.bold: true
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
              readonly property string planUsage: root.usageSummary(root.plan ? root.plan.usage : null)
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
            Text {
              visible: root.agentActive
              width: parent.width
              text: root.agentActive ? root.agent.last_line : ""
              textFormat: Text.PlainText
              color: root.mutedForeground
              elide: Text.ElideRight
              font.family: root.fontFamily
              font.pixelSize: root.fs(11)
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
            label: "implementer: "
              + (root.engineState ? root.engineState.settings.implementer : "…")
            onClicked: root.cycleTool("implementer")
          }
          PanelButton {
            label: "reviewer: "
              + (root.engineState ? root.engineState.settings.reviewer : "…")
            onClicked: root.cycleTool("reviewer")
          }
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
            ListView {
              id: chatList
              anchors.fill: parent
              anchors.margins: Style.space(8)
              clip: true
              spacing: Style.space(6)
              boundsBehavior: Flickable.StopAtBounds
              model: root.chat
              property bool followTail: true
              property real readingY: 0

              function scrollToTail() {
                if (followTail && !moving) positionViewAtEnd()
              }
              function restoreReadingPosition() {
                // Polling replaces the array; preserve older messages while reading.
                if (!followTail && !moving)
                  contentY = Math.max(originY, Math.min(readingY,
                    originY + contentHeight - height))
              }
              onContentYChanged: {
                if (moving) {
                  followTail = atYEnd
                  readingY = contentY
                }
              }
              onMovementEnded: {
                followTail = atYEnd
                readingY = contentY
              }
              onModelChanged: {
                Qt.callLater(restoreReadingPosition)
                Qt.callLater(scrollToTail)
              }
              onContentHeightChanged: Qt.callLater(scrollToTail)
              onHeightChanged: Qt.callLater(scrollToTail)
              onVisibleChanged: if (visible) Qt.callLater(scrollToTail)

              delegate: Text {
                required property var modelData
                width: chatList.width
                text: (modelData.role === "user" ? "You: " : "Forge: ") + modelData.text
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: modelData.role === "user" ? root.accent : root.foreground
                font.bold: modelData.role === "user"
                font.family: root.fontFamily
                font.pixelSize: root.fs(12)
              }
            }
          }
        }

        Text {
          visible: root.localError !== ""
          text: root.localError
          color: root.urgent
          font.family: root.fontFamily
          font.pixelSize: root.fs(11)
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
              Text {
                id: queueGoal
                width: Math.max(0, parent.width - queueGlyph.width - parent.spacing
                  - (queueControls.visible ? queueControls.width + parent.spacing : 0))
                anchors.verticalCenter: parent.verticalCenter
                text: queueRow.modelData.goal
                textFormat: Text.PlainText
                elide: Text.ElideRight
                maximumLineCount: 1
                color: queueRow.active ? root.accent : root.foreground
                font.family: root.fontFamily
                font.pixelSize: root.fs(12)
                font.bold: queueRow.active
              }
              Row {
                id: queueControls
                visible: queueRow.modelData.status === "queued"
                  || queueRow.modelData.status === "failed" || queueRow.modelData.status === "blocked"
                spacing: Style.space(4)
                PanelButton {
                  label: "↑"
                  visible: queueRow.modelData.status === "queued"
                  enabled: root.engineOnline && root.queue.slice(0, queueRow.index)
                    .some(function(item) { return item.status === "queued" })
                  onClicked: root.act("/api/queue/move", { id: queueRow.modelData.id, dir: "up" })
                }
                PanelButton {
                  label: "↓"
                  visible: queueRow.modelData.status === "queued"
                  enabled: root.engineOnline && root.queue.slice(queueRow.index + 1)
                    .some(function(item) { return item.status === "queued" })
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

        // ------------------------------------------------- stages
        Flow {
          visible: root.editingPlan
          width: parent.width
          spacing: Style.space(8)
          PanelButton {
            label: "Add stage"
            enabled: !root.editPending
            onClicked: root.addEditStage()
          }
          PanelButton {
            id: savePlanButton
            label: root.editPending ? "Saving…" : "Save"
            primary: true
            enabled: root.editingPlan && root.editValid && !root.editPending
              && root.engineOnline && !root.busy && !root.queueActive
            onClicked: root.savePlanEdit()
          }
          PanelButton {
            label: "Cancel"
            enabled: !root.editPending
            onClicked: root.cancelPlanEdit()
          }
          Text {
            text: "Editing plan · title and instructions required"
            color: root.mutedForeground
            font.family: root.fontFamily
            font.pixelSize: root.fs(11)
          }
        }
        Rectangle {
          width: parent.width
          height: Math.max(Style.space(100), parent.height * 0.34
            - (queueSection.visible ? queueSection.height + Style.space(10) : 0)
            - (chatSection.visible ? chatSection.height + Style.space(10) : 0))
          color: root.surface
          radius: 4
          ListView {
            id: stageList
            anchors.fill: parent
            anchors.margins: Style.space(8)
            clip: true
            spacing: Style.space(6)
            model: root.displayedStages
            delegate: Rectangle {
              id: stageRow
              required property var modelData
              required property int index
              readonly property bool expanded: root.expandedStageId === modelData.id
              readonly property bool editable: root.editingPlan && modelData.status !== "committed"
              function focusEditor() {
                if (stageEditor.item && !root.editPending) stageEditor.item.focusTitle()
              }
              readonly property double elapsedSecs: modelData.status === "in_progress"
                && typeof modelData.started_unix === "number"
                ? Math.max(0, Math.floor(root.agentNow - modelData.started_unix))
                : typeof modelData.duration_secs === "number"
                  ? Math.max(0, Math.floor(modelData.duration_secs)) : -1
              readonly property var reviewHistory: root.stageReviews(modelData)
              readonly property var lastReview: reviewHistory.length > 0
                ? reviewHistory[reviewHistory.length - 1] : null
              readonly property var lastDecision: lastReview
                ? root.reviewDecision(lastReview.verdict) : null
              readonly property string activity: root.stageActivity(modelData)
              width: stageList.width
              height: editable ? stageEditor.height : stageContent.implicitHeight
              radius: 3
              color: index === root.selectedStageIndex
                ? Qt.darker(root.accent, 2.8) : "transparent"
              TapHandler {
                enabled: !stageRow.editable
                onTapped: root.expandedStageId = stageRow.expanded
                  ? -1 : stageRow.modelData.id
              }
              Column {
                id: stageContent
                visible: !stageRow.editable
                width: stageRow.width
                spacing: 2
                Flow {
                  width: stageRow.width
                  spacing: Style.space(8)
                  Row {
                    width: Math.min(implicitWidth, stageRow.width)
                    spacing: Style.space(4)
                    Text {
                      id: stageGlyph
                      text: stageRow.modelData.status === "committed" ? "✓"
                        : stageRow.modelData.status === "in_progress" ? "●"
                        : stageRow.modelData.status === "blocked" ? "!" : "·"
                      color: stageRow.modelData.status === "committed" ? root.success
                        : stageRow.modelData.status === "in_progress" ? root.working
                        : stageRow.modelData.status === "blocked" ? root.urgent
                        : root.mutedForeground
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(12)
                      font.bold: true
                    }
                    Text {
                      width: Math.min(implicitWidth,
                        stageRow.width - stageGlyph.width - Style.space(4))
                      text: stageRow.modelData.id + ". " + stageRow.modelData.title
                      textFormat: Text.PlainText
                      color: root.foreground
                      wrapMode: Text.Wrap
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(12)
                      font.bold: true
                    }
                  }
                  Text {
                    width: Math.min(implicitWidth, stageRow.width)
                    text: root.editingPlan && stageRow.modelData.status === "committed"
                      ? "committed — locked" : stageRow.modelData.status === "blocked"
                      ? "blocked · review budget exhausted" : stageRow.modelData.status
                      + (stageRow.modelData.sha ? " " + stageRow.modelData.sha : "")
                    color: stageRow.modelData.status === "committed" ? root.success
                      : stageRow.modelData.status === "in_progress" ? root.working
                      : stageRow.modelData.status === "blocked" ? root.urgent
                      : root.mutedForeground
                    wrapMode: Text.Wrap
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(11)
                  }
                  Text {
                    visible: text !== ""
                    width: Math.min(implicitWidth, stageRow.width)
                    text: stageRow.activity
                    textFormat: Text.PlainText
                    color: root.working
                    wrapMode: Text.Wrap
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(11)
                    font.bold: true
                  }
                  Text {
                    visible: stageRow.reviewHistory.length > 0
                    width: Math.min(implicitWidth, stageRow.width)
                    text: stageRow.lastReview
                      ? "last completed review · " + root.reviewRoundLabel(stageRow.lastReview)
                        + ": " + stageRow.lastDecision.label
                      : ""
                    textFormat: Text.PlainText
                    color: stageRow.lastDecision && stageRow.lastDecision.clean
                      ? (stageRow.activity ? root.mutedForeground : root.success) : root.urgent
                    wrapMode: Text.Wrap
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(11)
                  }
                  Text {
                    visible: stageRow.elapsedSecs >= 0
                    text: visible ? Math.floor(stageRow.elapsedSecs / 60) + "m "
                      + (stageRow.elapsedSecs % 60) + "s" : ""
                    color: root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(11)
                  }
                }
                Text {
                  text: stageRow.modelData.commit
                  color: root.mutedForeground
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(11)
                  font.italic: true
                }
                Text {
                  visible: !stageRow.expanded
                  width: stageList.width
                  text: stageRow.modelData.instructions
                  color: root.mutedForeground
                  wrapMode: Text.Wrap
                  maximumLineCount: 3
                  elide: Text.ElideRight
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(11)
                }
                Text {
                  visible: stageRow.expanded
                  width: stageRow.width
                  text: "instructions:\n" + (stageRow.modelData.instructions || "")
                  textFormat: Text.PlainText
                  color: root.mutedForeground
                  wrapMode: Text.Wrap
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(11)
                }
                Text {
                  visible: stageRow.expanded
                  width: stageRow.width
                  text: "acceptance criteria:\n" + (stageRow.modelData.acceptance || "")
                  textFormat: Text.PlainText
                  color: root.mutedForeground
                  wrapMode: Text.Wrap
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(11)
                }
                Repeater {
                  model: stageRow.reviewHistory
                  delegate: Column {
                    id: reviewRound
                    required property var modelData
                    readonly property var verdict: modelData.verdict
                    readonly property var decision: root.reviewDecision(verdict)
                    readonly property var issues: root.reviewStrings(verdict.issues)
                    readonly property var notes: root.reviewStrings(verdict.notes)
                    readonly property var checks: root.reviewStrings(verdict.checks)
                    visible: stageRow.expanded
                    width: stageRow.width
                    spacing: 2
                    Text {
                      width: stageRow.width
                      text: "review " + root.reviewRoundLabel(reviewRound.modelData) + " — "
                        + reviewRound.decision.label
                      textFormat: Text.PlainText
                      color: reviewRound.decision.clean ? root.success : root.urgent
                      wrapMode: Text.Wrap
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(11)
                    }
                    Text {
                      visible: text !== ""
                      width: stageRow.width
                      text: root.reviewTimestamp(reviewRound.verdict)
                      textFormat: Text.PlainText
                      color: root.mutedForeground
                      wrapMode: Text.Wrap
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(11)
                    }
                    Text {
                      visible: text !== ""
                      width: stageRow.width
                      text: reviewRound.verdict.summary || ""
                      textFormat: Text.PlainText
                      color: root.mutedForeground
                      wrapMode: Text.Wrap
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(11)
                    }
                    Text {
                      visible: reviewRound.issues.length > 0
                      width: stageRow.width
                      text: "change requests:\n• " + reviewRound.issues.join("\n• ")
                      textFormat: Text.PlainText
                      color: root.urgent
                      wrapMode: Text.Wrap
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(11)
                    }
                    Text {
                      visible: reviewRound.notes.length > 0
                      width: stageRow.width
                      text: "legacy notes (change requests):\n• " + reviewRound.notes.join("\n• ")
                      textFormat: Text.PlainText
                      color: root.mutedForeground
                      wrapMode: Text.Wrap
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(11)
                    }
                    Text {
                      visible: reviewRound.checks.length > 0
                      width: stageRow.width
                      text: "verified: " + reviewRound.checks.join("; ")
                      textFormat: Text.PlainText
                      color: root.mutedForeground
                      wrapMode: Text.Wrap
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(11)
                    }
                  }
                }
                Text {
                  visible: stageRow.expanded && text !== ""
                  width: stageRow.width
                  text: root.usageSummary(stageRow.modelData.usage)
                  textFormat: Text.PlainText
                  color: root.mutedForeground
                  wrapMode: Text.Wrap
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(11)
                }
              }
              Loader {
                id: stageEditor
                active: stageRow.editable
                width: stageRow.width
                sourceComponent: Column {
                  width: stageEditor.width
                  spacing: Style.space(6)
                  function focusTitle() { titleEditor.focusField() }
                  Flow {
                    width: parent.width
                    spacing: Style.space(8)
                    Text {
                      text: stageRow.modelData.id === undefined ? "New stage"
                        : "Stage " + stageRow.modelData.id
                      color: root.foreground
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(12)
                      font.bold: true
                    }
                    PanelButton {
                      label: "↑ Up"
                      enabled: !root.editPending && root.editableNeighbor(stageRow.index, -1) >= 0
                      onClicked: root.moveEditStage(stageRow.index, -1)
                    }
                    PanelButton {
                      label: "↓ Down"
                      enabled: !root.editPending && root.editableNeighbor(stageRow.index, 1) >= 0
                      onClicked: root.moveEditStage(stageRow.index, 1)
                    }
                    PanelButton {
                      label: "Delete"
                      enabled: !root.editPending
                      labelColor: root.urgent
                      onClicked: root.deleteEditStage(stageRow.index)
                    }
                  }
                  PlanEditField {
                    id: titleEditor
                    width: parent.width
                    label: "Title"
                    value: stageRow.modelData.title
                    onEdited: value => root.changeStageField(stageRow.index, "title", value)
                  }
                  PlanEditField {
                    width: parent.width
                    label: "Instructions"
                    value: stageRow.modelData.instructions
                    multiline: true
                    onEdited: value => root.changeStageField(stageRow.index, "instructions", value)
                  }
                  PlanEditField {
                    width: parent.width
                    label: "Acceptance"
                    value: stageRow.modelData.acceptance
                    multiline: true
                    onEdited: value => root.changeStageField(stageRow.index, "acceptance", value)
                  }
                  PlanEditField {
                    width: parent.width
                    label: "Commit"
                    value: stageRow.modelData.commit
                    onEdited: value => root.changeStageField(stageRow.index, "commit", value)
                  }
                  Item { width: 1; height: Style.space(6) }
                }
              }
            }
            Text {
              visible: !root.editingPlan && root.plan === null
              text: "no plan yet"
              color: root.mutedForeground
              font.family: root.fontFamily
              font.pixelSize: root.fs(12)
            }
          }
        }

        // ------------------------------------------- live / history
        Row {
          spacing: Style.space(8)
          PanelButton {
            label: "Live"
            primary: root.liveTab
            onClicked: root.liveTab = true
          }
          PanelButton {
            label: "History"
            primary: !root.liveTab
            onClicked: root.liveTab = false
          }
        }

        Rectangle {
          width: parent.width
          height: Math.max(0, parent.height - y)
          color: root.surface
          radius: 4
          Flickable {
            id: liveOutput
            visible: root.liveTab
            anchors.fill: parent
            anchors.margins: Style.space(8)
            clip: true
            contentWidth: width
            contentHeight: liveText.height
            flickableDirection: Flickable.VerticalFlick
            boundsBehavior: Flickable.StopAtBounds
            property bool followTail: true

            function scrollToTail() {
              if (followTail && !moving)
                contentY = Math.max(0, contentHeight - height)
            }
            onContentYChanged: {
              if (moving) followTail = atYEnd
            }
            onMovementEnded: followTail = atYEnd
            onContentHeightChanged: Qt.callLater(scrollToTail)
            onHeightChanged: Qt.callLater(scrollToTail)
            onVisibleChanged: if (visible) Qt.callLater(scrollToTail)

            Text {
              id: liveText
              width: liveOutput.width
              text: root.agentLog
              textFormat: Text.PlainText
              color: root.mutedForeground
              wrapMode: Text.Wrap
              font.family: root.fontFamily
              font.pixelSize: root.fs(10)
            }
          }
          Row {
            id: historyFilters
            visible: !root.liveTab
            anchors.top: parent.top
            anchors.left: parent.left
            anchors.margins: Style.space(8)
            spacing: Style.space(4)
            Repeater {
              model: root.hasReports
                ? ["all", "runs", "git", "reviews", "errors", "reports"]
                : ["all", "runs", "git", "reviews", "errors"]
              delegate: PanelButton {
                required property string modelData
                label: modelData
                primary: root.historyFilter === modelData
                onClicked: root.historyFilter = modelData
              }
            }
          }
          ListView {
            id: historyList
            visible: !root.liveTab && !root.reportsVisible
            anchors.top: historyFilters.bottom
            anchors.bottom: parent.bottom
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.margins: Style.space(8)
            clip: true
            boundsBehavior: Flickable.StopAtBounds
            model: root.historyRows(root.engineState ? root.engineState.history : [],
                                    root.historyFilter)
            property bool followTail: true
            property real readingY: 0

            function scrollToTail() {
              if (followTail && !moving) positionViewAtEnd()
            }
            function restoreReadingPosition() {
              // Replacing a JavaScript array resets ListView's position on every poll.
              if (!followTail && !moving)
                contentY = Math.max(originY, Math.min(readingY,
                  originY + contentHeight - height))
            }
            onContentYChanged: {
              if (moving) {
                followTail = atYEnd
                readingY = contentY
              }
            }
            onMovementEnded: {
              followTail = atYEnd
              readingY = contentY
            }
            onModelChanged: Qt.callLater(restoreReadingPosition)
            onCountChanged: Qt.callLater(scrollToTail)
            onContentHeightChanged: Qt.callLater(scrollToTail)
            onHeightChanged: Qt.callLater(scrollToTail)
            onVisibleChanged: if (visible) Qt.callLater(scrollToTail)

            delegate: Text {
              id: historyRow
              required property var modelData
              readonly property bool separator: modelData.kind === "sep"
              readonly property var event: separator ? null : modelData.event
              width: historyList.width
              topPadding: separator ? Style.space(6) : 0
              bottomPadding: separator ? Style.space(4) : 0
              text: separator ? "— goal: " + modelData.goal
                : root.historyTime(event, root.agentNow) + "  [" + event.kind + "]  "
                  + event.text
              textFormat: Text.PlainText
              color: separator ? root.accent
                : event.kind === "error" ? root.urgent
                : event.kind === "git" ? root.success
                : (event.kind === "review" || event.kind === "check") ? root.working
                : root.mutedForeground
              wrapMode: Text.Wrap
              font.family: root.fontFamily
              font.pixelSize: root.fs(10)
            }
          }
          ListView {
            id: reportList
            visible: root.reportsVisible
            anchors.top: historyFilters.bottom
            anchors.bottom: parent.bottom
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.margins: Style.space(8)
            clip: true
            spacing: Style.space(6)
            boundsBehavior: Flickable.StopAtBounds
            model: root.reports
            property real readingY: 0

            function restoreReadingPosition() {
              if (!moving) contentY = Math.max(originY, Math.min(readingY,
                originY + Math.max(0, contentHeight - height)))
            }
            onContentYChanged: if (moving) readingY = contentY
            onMovementEnded: readingY = contentY
            onModelChanged: Qt.callLater(restoreReadingPosition)
            onHeightChanged: Qt.callLater(restoreReadingPosition)
            onVisibleChanged: if (visible) Qt.callLater(restoreReadingPosition)

            delegate: Rectangle {
              id: reportRow
              required property var modelData
              required property int index
              readonly property string key: root.reportKey(modelData, index)
              readonly property bool expanded: root.expandedReportKey === key
              readonly property int commitCount: modelData.commits && typeof modelData.commits.length === "number"
                ? modelData.commits.length : 0
              width: reportList.width
              height: reportContent.implicitHeight + Style.space(8)
              radius: 3
              color: key === root.selectedReportKey ? Qt.darker(root.accent, 2.8) : "transparent"
              TapHandler {
                onTapped: {
                  keyHandler.forceActiveFocus()
                  root.selectedReportKey = reportRow.key
                  root.expandedReportKey = reportRow.expanded ? "" : reportRow.key
                }
              }
              Column {
                id: reportContent
                x: Style.space(4)
                y: Style.space(4)
                width: parent.width - Style.space(8)
                spacing: Style.space(4)
                Text {
                  width: parent.width
                  text: (reportRow.expanded ? "▾ " : "▸ ") + (reportRow.modelData.goal || "Untitled task")
                  textFormat: Text.PlainText
                  color: root.foreground
                  elide: Text.ElideRight
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(11)
                }
                Text {
                  width: parent.width
                  text: root.reportTime(reportRow.modelData, root.agentNow)
                    + " · " + root.reportDuration(reportRow.modelData.duration_secs)
                    + " · " + reportRow.commitCount + (reportRow.commitCount === 1 ? " commit" : " commits")
                  textFormat: Text.PlainText
                  color: root.mutedForeground
                  wrapMode: Text.Wrap
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(10)
                }
                Text {
                  visible: text !== ""
                  width: parent.width
                  text: root.usageSummary(reportRow.modelData.usage)
                  textFormat: Text.PlainText
                  color: root.mutedForeground
                  wrapMode: Text.Wrap
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(10)
                }
                Text {
                  visible: reportRow.expanded
                  width: parent.width
                  text: "goal: " + (reportRow.modelData.goal || "Untitled task")
                    + "\ncommits:\n" + (root.reportCommits(reportRow.modelData.commits) || "No commits")
                  textFormat: Text.PlainText
                  color: root.mutedForeground
                  wrapMode: Text.Wrap
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(10)
                }
                Text {
                  visible: reportRow.expanded && text !== ""
                  width: parent.width
                  text: root.usageBreakdown(reportRow.modelData.usage)
                  textFormat: Text.PlainText
                  color: root.mutedForeground
                  wrapMode: Text.Wrap
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(10)
                }
                Text {
                  visible: reportRow.expanded && text !== ""
                  width: parent.width
                  text: {
                    const usage = root.usageBreakdown(reportRow.modelData.planner_usage)
                    return usage ? "planner usage:\n" + usage : ""
                  }
                  textFormat: Text.PlainText
                  color: root.mutedForeground
                  wrapMode: Text.Wrap
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(10)
                }
              }
            }
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

      // ------------------------------------------------ diff viewer
      Rectangle {
        visible: root.diffOpen
        anchors.fill: parent
        color: Qt.rgba(0, 0, 0, 0.55)
        MouseArea { anchors.fill: parent; onClicked: root.diffOpen = false }

        Rectangle {
          anchors.centerIn: parent
          width: parent.width * 0.82
          height: parent.height * 0.82
          radius: 6
          color: root.surface
          border.width: 1
          border.color: Qt.darker(root.foreground, 3)
          MouseArea { anchors.fill: parent }

          Column {
            anchors.fill: parent
            anchors.margins: Style.space(12)
            spacing: Style.space(8)

            Row {
              width: parent.width
              spacing: Style.space(8)
              Text {
                width: parent.width - refreshDiffButton.width - closeDiffButton.width
                  - parent.spacing * 2
                anchors.verticalCenter: parent.verticalCenter
                text: "Uncommitted diff"
                color: root.foreground
                font.family: root.fontFamily
                font.pixelSize: root.fs(12)
                font.bold: true
                elide: Text.ElideRight
              }
              PanelButton {
                id: refreshDiffButton
                label: "Refresh"
                enabled: root.engineOnline && !root.diffPending
                onClicked: root.refreshDiff()
              }
              PanelButton {
                id: closeDiffButton
                label: "Close"
                onClicked: root.diffOpen = false
              }
            }

            Text {
              visible: root.diffError !== ""
              width: parent.width
              text: root.diffError
              color: root.urgent
              font.family: root.fontFamily
              font.pixelSize: root.fs(10)
              wrapMode: Text.WrapAnywhere
            }

            ListView {
              id: diffList
              width: parent.width
              height: parent.height - y
              clip: true
              boundsBehavior: Flickable.StopAtBounds
              model: root.diffText === "" ? [] : root.diffText.split("\n")

              delegate: Text {
                id: diffLine
                required property string modelData
                readonly property bool header: modelData.indexOf("@@") === 0
                  || modelData.indexOf("diff ") === 0
                width: diffList.width
                text: modelData === "" ? " " : modelData
                textFormat: Text.PlainText
                color: header ? root.info
                  : modelData.indexOf("+") === 0 && modelData.indexOf("+++") !== 0
                    ? root.success
                  : modelData.indexOf("-") === 0 && modelData.indexOf("---") !== 0
                    ? root.urgent : root.foreground
                font.family: root.fontFamily
                font.pixelSize: root.fs(10)
                font.bold: header
                wrapMode: Text.WrapAnywhere
              }

              Text {
                visible: root.diffText === "" && root.diffError === ""
                width: parent.width
                text: root.diffPending ? "Loading diff…" : "no uncommitted changes"
                color: root.mutedForeground
                font.family: root.fontFamily
                font.pixelSize: root.fs(10)
                wrapMode: Text.WrapAnywhere
              }
            }
          }
        }
      }

      // ------------------------------------------------ project chooser
      Rectangle {
        visible: root.chooserOpen
        anchors.fill: parent
        color: Qt.rgba(0, 0, 0, 0.55)
        MouseArea { anchors.fill: parent; onClicked: root.chooserOpen = false }

        Rectangle {
          anchors.centerIn: parent
          width: parent.width * 0.82
          height: parent.height * 0.82
          radius: 6
          color: root.surface
          border.width: 1
          border.color: Qt.darker(root.foreground, 3)
          MouseArea { anchors.fill: parent }

          Column {
            anchors.fill: parent
            anchors.margins: Style.space(12)
            spacing: Style.space(8)

            Row {
              width: parent.width
              spacing: Style.space(8)
              Rectangle {
                width: parent.width - closeChooserButton.width - parent.spacing
                height: Style.space(28)
                color: root.background
                radius: 4
                border.width: 1
                border.color: filterField.activeFocus
                  ? root.accent : Qt.darker(root.foreground, 3)
                TextInput {
                  id: filterField
                  onAccepted: {
                    chooserList.resetSelection()
                    chooserList.activateSelection()
                  }
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
                  anchors.fill: parent
                  anchors.margins: Style.space(6)
                  verticalAlignment: TextInput.AlignVCenter
                  color: root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(12)
                  clip: true
                  Text {
                    visible: filterField.text === "" && !filterField.activeFocus
                    text: "filter projects…"
                    color: root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(12)
                  }
                }
              }
              PanelButton {
                id: closeChooserButton
                label: "Close"
                onClicked: root.chooserOpen = false
              }
            }

            ListView {
              id: chooserList
              width: parent.width
              height: parent.height - y - (root.manualEntry ? Style.space(38) : 0)
              clip: true
              spacing: 2
              model: root.chooserRows(root.projectsData, filterField.text)
              currentIndex: -1
              onModelChanged: resetSelection()

              function isSelectable(row) {
                return row && (row.kind === "local" || row.kind === "remote" || row.kind === "path")
              }

              function moveSelection(direction) {
                const rows = model || []
                for (let index = currentIndex + direction;
                     index >= 0 && index < rows.length; index += direction) {
                  if (!isSelectable(rows[index])) continue
                  currentIndex = index
                  positionViewAtIndex(index, ListView.Contain)
                  return
                }
              }

              function resetSelection() {
                currentIndex = -1
                moveSelection(1)
              }

              function activateSelection() {
                const row = model && model[currentIndex]
                if (isSelectable(row)) root.chooseRow(row)
              }

              delegate: Rectangle {
                id: chooserRow
                required property var modelData
                required property int index
                readonly property bool selectable: chooserList.isSelectable(modelData)
                width: chooserList.width
                height: rowText.implicitHeight + Style.space(10)
                radius: 4
                color: selectable && (chooserRowArea.containsMouse || chooserList.currentIndex === index)
                  ? Qt.darker(root.accent, 2.8) : "transparent"

                Row {
                  anchors.verticalCenter: parent.verticalCenter
                  x: Style.space(6)
                  spacing: Style.space(8)
                  Text {
                    id: rowText
                    text: chooserRow.modelData.kind === "local"
                      || chooserRow.modelData.kind === "remote"
                      ? chooserRow.modelData.name : chooserRow.modelData.label
                    color: chooserRow.modelData.kind === "header" ? root.accent
                      : chooserRow.modelData.kind === "local"
                        || chooserRow.modelData.kind === "remote"
                        ? root.foreground : root.mutedForeground
                    font.family: root.fontFamily
                    font.bold: chooserRow.modelData.kind === "header"
                    font.pixelSize: root.fs(
                      chooserRow.modelData.kind === "note" ? 10 : 12)
                  }
                  Text {
                    visible: chooserRow.modelData.kind === "local"
                    text: chooserRow.modelData.path || ""
                    color: root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(10)
                    anchors.verticalCenter: parent.verticalCenter
                  }
                  Text {
                    visible: chooserRow.modelData.kind === "remote"
                    text: (chooserRow.modelData.isPrivate ? "private · " : "")
                      + (chooserRow.modelData.cloned ? "cloned" : "will clone")
                    color: chooserRow.modelData.cloned
                      ? root.success : root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(10)
                    anchors.verticalCenter: parent.verticalCenter
                  }
                }
                MouseArea {
                  id: chooserRowArea
                  anchors.fill: parent
                  hoverEnabled: true
                  enabled: chooserRow.selectable
                  onClicked: root.chooseRow(chooserRow.modelData)
                }
              }
            }

            Row {
              id: manualRow
              visible: root.manualEntry
              width: parent.width
              spacing: Style.space(8)
              Rectangle {
                width: parent.width - manualSetButton.width - parent.spacing
                height: Style.space(28)
                color: root.background
                radius: 4
                border.width: 1
                border.color: manualField.activeFocus
                  ? root.accent : Qt.darker(root.foreground, 3)
                TextInput {
                  id: manualField
                  onAccepted: manualSetButton.clicked()
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
                  anchors.fill: parent
                  anchors.margins: Style.space(6)
                  verticalAlignment: TextInput.AlignVCenter
                  color: root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(12)
                  clip: true
                }
              }
              PanelButton {
                id: manualSetButton
                label: "Set"
                onClicked: {
                  root.act("/api/project", { path: manualField.text })
                  root.chooserOpen = false
                }
              }
            }
          }
        }
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
                    { key: "1 / 2 / 3 / 4 / 5", description: "History: All / Runs / Git / Reviews / Errors" },
                    { key: "6", description: "History: Reports (when available)" },
                    { key: "p", description: "Create plan from goal" },
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

  component PlanEditField: Column {
    id: editField
    required property string label
    required property string value
    property bool multiline: false
    signal edited(string value)
    spacing: Style.space(3)
    enabled: !root.editPending

    function focusField() { field.forceActiveFocus() }

    Text {
      text: editField.label
      color: root.mutedForeground
      font.family: root.fontFamily
      font.pixelSize: root.fs(11)
    }
    Rectangle {
      width: parent.width
      height: Math.min(Math.max(Style.space(editField.multiline ? 48 : 26),
        field.contentHeight + Style.space(12)), Style.space(editField.multiline ? 96 : 26))
      color: root.background
      radius: 4
      border.width: 1
      border.color: field.activeFocus ? root.accent : Qt.darker(root.foreground, 3)
      Flickable {
        id: fieldFlick
        anchors.fill: parent
        anchors.margins: Style.space(6)
        clip: true
        contentWidth: field.width
        contentHeight: field.height
        flickableDirection: Flickable.VerticalFlick
        boundsBehavior: Flickable.StopAtBounds

        function ensureCursorVisible() {
          if (!field.activeFocus) return
          const cursor = field.cursorRectangle
          if (contentY > cursor.y) contentY = cursor.y
          else if (contentY + height < cursor.y + cursor.height)
            contentY = cursor.y + cursor.height - height
          contentY = Math.max(0, Math.min(contentY, contentHeight - height))
          // Keep the active field visible even when its stage exceeds the viewport.
          const top = editField.mapToItem(stageList.contentItem, 0, 0).y
          if (top < stageList.contentY) stageList.contentY = top
          else if (top + editField.height > stageList.contentY + stageList.height)
            stageList.contentY = top + editField.height - stageList.height
        }
        onHeightChanged: Qt.callLater(ensureCursorVisible)
        TextEdit {
          id: field
          width: fieldFlick.width
          height: Math.max(contentHeight, fieldFlick.height)
          text: editField.value
          textFormat: TextEdit.PlainText
          wrapMode: TextEdit.Wrap
          selectByMouse: true
          color: root.foreground
          font.family: root.fontFamily
          font.pixelSize: root.fs(12)
          onTextChanged: if (activeFocus) editField.edited(text)
          onActiveFocusChanged: {
            if (activeFocus) {
              root.editFocusedField = field
              fieldFlick.ensureCursorVisible()
            } else if (root.editFocusedField === field) root.editFocusedField = null
          }
          onCursorRectangleChanged: fieldFlick.ensureCursorVisible()
          Keys.onPressed: event => {
            if (event.key === Qt.Key_F1) {
              root.helpOpen = true
              keyHandler.forceActiveFocus()
              event.accepted = true
            } else if (!editField.multiline
                       && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter)) {
              event.accepted = true
            }
          }
          Keys.onEscapePressed: event => {
            keyHandler.forceActiveFocus()
            event.accepted = true
          }
        }
      }
    }
  }

  component PanelButton: Rectangle {
    id: button
    property string label: ""
    property bool primary: false
    property bool enabled: true
    property color labelColor: primary && enabled ? root.background : root.foreground
    signal clicked()

    implicitWidth: buttonText.implicitWidth + Style.space(18)
    width: implicitWidth
    height: buttonText.implicitHeight + Style.space(10)
    radius: 4
    color: button.primary && button.enabled ? root.accent : root.surface
    border.width: button.primary && button.enabled ? 0 : 1
    border.color: Qt.darker(root.foreground, 3)
    opacity: button.enabled ? (buttonArea.containsMouse ? 0.85 : 1.0) : 0.45

    Text {
      id: buttonText
      anchors.centerIn: parent
      width: Math.max(0, button.width - Style.space(18))
      text: button.label
      textFormat: Text.PlainText
      elide: Text.ElideRight
      color: button.labelColor
      font.family: root.fontFamily
      font.pixelSize: root.fs(11)
    }
    MouseArea {
      id: buttonArea
      anchors.fill: parent
      hoverEnabled: true
      enabled: button.enabled
      onClicked: button.clicked()
    }
  }
}
