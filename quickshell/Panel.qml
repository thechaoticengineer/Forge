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
  readonly property bool queueActive: engineState !== null && engineState.queue_active === true
  readonly property bool hasQueuedGoals: queue.some(function(item) { return item.status === "queued" })
  readonly property string phase: engineState ? engineState.phase : "offline"
  // Only the displayed project's work gates its controls, never background sessions.
  readonly property bool busy: phase === "planning" || phase === "running"
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

  readonly property var agent: engineState ? engineState.agent : null
  readonly property bool agentActive: agent !== null && agent !== undefined
    && agent.role !== ""
  readonly property string agentSession: agentActive
    ? JSON.stringify([engineState.project, engineState.run_started_unix,
                      agent.started_unix, agent.role, agent.tool, agent.model]) : ""
  property double agentNow: Date.now() / 1000
  property bool liveTab: false
  property string historyFilter: "all"
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
    goalField.text = goalDrafts[project] || ""
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
  }

  onBusyChanged: liveTab = busy
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
        if (done) done(null)
        return
      }
      root.engineOnline = true
      let parsed = null
      try { parsed = JSON.parse(xhr.responseText) } catch (e) {}
      if (parsed && parsed.error) root.localError = parsed.error
      if (done) done(parsed)
    }
    xhr.send(body ? JSON.stringify(body) : null)
  }

  function refresh() {
    api("GET", "/api/state", null, function(resp) {
      if (resp) root.engineState = resp
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

  function act(path, body) {
    localError = ""
    api("POST", path, body || {}, function() { root.refresh() })
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

        function selectStage(index) {
          const stages = root.plan && root.plan.stages ? root.plan.stages : []
          if (stages.length === 0) return
          root.selectedStageIndex = Math.max(0, Math.min(index, stages.length - 1))
          stageList.positionViewAtIndex(root.selectedStageIndex, ListView.Contain)
        }

        Keys.onPressed: event => {
          const prefix = pendingKey
          pendingKey = ""
          if (event.key === Qt.Key_I && event.modifiers === Qt.NoModifier) {
            goalField.forceActiveFocus()
            event.accepted = true
          } else if (event.key === Qt.Key_Escape) {
            if (root.diffOpen) root.diffOpen = false
            else if (root.chooserOpen) root.chooserOpen = false
            event.accepted = true
          } else if (!root.diffOpen && !root.chooserOpen) {
            const stages = root.plan && root.plan.stages ? root.plan.stages : []
            if (event.key === Qt.Key_G && event.modifiers === Qt.ShiftModifier) {
              selectStage(stages.length - 1)
              event.accepted = true
            } else if (event.modifiers === Qt.NoModifier) {
              if (event.key === Qt.Key_J || event.key === Qt.Key_K) {
                selectStage(root.selectedStageIndex < 0 ? 0
                  : root.selectedStageIndex + (event.key === Qt.Key_J ? 1 : -1))
                event.accepted = true
              } else if (event.key === Qt.Key_G) {
                if (prefix === "g") selectStage(0)
                else pendingKey = "g"
                event.accepted = true
              } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter
                         || event.key === Qt.Key_O || event.key === Qt.Key_Space) {
                if (root.selectedStageIndex >= 0 && root.selectedStageIndex < stages.length) {
                  const stageId = stages[root.selectedStageIndex].id
                  root.expandedStageId = root.expandedStageId === stageId ? -1 : stageId
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
              - 2 * parent.spacing)
            elide: Text.ElideRight
            text: root.engineState && root.engineState.current_stage !== null
              ? "stage " + root.engineState.current_stage + ": "
                + root.engineState.current_step
              : (root.engineState ? root.engineState.current_step : "")
            color: root.mutedForeground
            font.family: root.fontFamily
            font.pixelSize: root.fs(12)
            anchors.verticalCenter: parent.verticalCenter
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
                text: root.engineState ? root.engineState.current_step : ""
                textFormat: Text.PlainText
                color: root.working
                elide: Text.ElideRight
                font.family: root.fontFamily
                font.pixelSize: root.fs(11)
              }
            }
            Text {
              visible: root.phase !== "planning"
                || (root.engineState !== null && root.engineState.run_started_unix > 0)
              width: parent.width
              text: (root.phase !== "planning"
                  ? agentCard.committedStages + "/" + agentCard.stages.length + " stages committed" : "")
                + (root.engineState && root.engineState.run_started_unix > 0
                  ? (root.phase !== "planning" ? " · " : "")
                    + "run " + Math.floor(agentCard.runSeconds / 60) + "m "
                    + (agentCard.runSeconds % 60) + "s" : "")
              textFormat: Text.PlainText
              color: root.mutedForeground
              elide: Text.ElideRight
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
                text: "What should be built?"
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
            label: "Create plan"
            primary: true
            enabled: !root.busy && goalField.text.trim() !== ""
            onClicked: root.act("/api/plan", { goal: goalField.text })
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
            label: "Plan is OK — approve"
            enabled: !root.busy && root.plan !== null && root.plan.status === "draft"
            onClicked: root.act("/api/approve")
          }
          PanelButton {
            label: "Start implementing"
            primary: true
            enabled: !root.busy && root.plan !== null
              && (root.plan.status === "approved" || root.plan.status === "done")
            onClicked: root.act("/api/run")
          }
          PanelButton {
            label: "Stop"
            enabled: root.phase === "running" || root.queueActive
            onClicked: root.act("/api/stop")
          }
          PanelButton {
            label: "Discard plan"
            enabled: !root.busy && root.plan !== null
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
              enabled: root.engineOnline && !root.busy && !root.queueActive && root.hasQueuedGoals
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
                spacing: Style.space(4)
                PanelButton {
                  label: "↑"
                  enabled: root.engineOnline && root.queue.slice(0, queueRow.index)
                    .some(function(item) { return item.status === "queued" })
                  onClicked: root.act("/api/queue/move", { id: queueRow.modelData.id, dir: "up" })
                }
                PanelButton {
                  label: "↓"
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
        Rectangle {
          width: parent.width
          height: Math.max(Style.space(100), parent.height * 0.34
            - (queueSection.visible ? queueSection.height + Style.space(10) : 0))
          color: root.surface
          radius: 4
          ListView {
            id: stageList
            anchors.fill: parent
            anchors.margins: Style.space(8)
            clip: true
            spacing: Style.space(6)
            model: root.plan ? root.plan.stages : []
            delegate: Rectangle {
              id: stageRow
              required property var modelData
              required property int index
              readonly property bool expanded: root.expandedStageId === modelData.id
              readonly property double elapsedSecs: modelData.status === "in_progress"
                && typeof modelData.started_unix === "number"
                ? Math.max(0, Math.floor(root.agentNow - modelData.started_unix))
                : typeof modelData.duration_secs === "number"
                  ? Math.max(0, Math.floor(modelData.duration_secs)) : -1
              readonly property var reviewerIssues: modelData.last_verdict
                && modelData.last_verdict.issues ? modelData.last_verdict.issues : []
              readonly property var reviewHistory: modelData.reviews || []
              readonly property var lastReview: reviewHistory.length > 0
                ? reviewHistory[reviewHistory.length - 1] : null
              width: stageList.width
              height: stageContent.implicitHeight
              radius: 3
              color: index === root.selectedStageIndex
                ? Qt.darker(root.accent, 2.8) : "transparent"
              TapHandler {
                onTapped: root.expandedStageId = stageRow.expanded
                  ? -1 : stageRow.modelData.id
              }
              Column {
                id: stageContent
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
                    text: stageRow.modelData.status
                      + (stageRow.modelData.sha ? " " + stageRow.modelData.sha : "")
                      + (stageRow.modelData.rounds > 1
                         ? " (round " + stageRow.modelData.rounds + ")" : "")
                    color: stageRow.modelData.status === "committed" ? root.success
                      : stageRow.modelData.status === "in_progress" ? root.working
                      : stageRow.modelData.status === "blocked" ? root.urgent
                      : root.mutedForeground
                    wrapMode: Text.Wrap
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(11)
                  }
                  Text {
                    visible: stageRow.reviewHistory.length > 0
                    width: Math.min(implicitWidth, stageRow.width)
                    text: stageRow.lastReview
                      ? "review: " + (stageRow.lastReview.approved ? "approved"
                        : (stageRow.lastReview.issues || []).length + " issue(s)")
                        + (stageRow.reviewHistory.length > 1
                          ? " · " + stageRow.reviewHistory.length + " rounds" : "")
                      : ""
                    textFormat: Text.PlainText
                    color: stageRow.lastReview && stageRow.lastReview.approved
                      ? root.success : root.urgent
                    wrapMode: Text.Wrap
                    font.family: root.fontFamily
                    font.pixelSize: root.fs(11)
                  }
                  Text {
                    visible: text !== ""
                    width: Math.min(implicitWidth, stageRow.width)
                    text: root.engineState
                      && stageRow.modelData.id === root.engineState.current_stage
                      ? root.engineState.current_step || "" : ""
                    textFormat: Text.PlainText
                    color: root.working
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
                    visible: stageRow.expanded
                    width: stageRow.width
                    spacing: 2
                    Text {
                      width: stageRow.width
                      text: "review round " + reviewRound.modelData.round + " — "
                        + (reviewRound.modelData.approved ? "approved" : "rejected")
                      textFormat: Text.PlainText
                      color: reviewRound.modelData.approved ? root.success : root.urgent
                      wrapMode: Text.Wrap
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(11)
                    }
                    Text {
                      visible: text !== ""
                      width: stageRow.width
                      text: reviewRound.modelData.summary || ""
                      textFormat: Text.PlainText
                      color: root.mutedForeground
                      wrapMode: Text.Wrap
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(11)
                    }
                    Text {
                      visible: (reviewRound.modelData.issues || []).length > 0
                      width: stageRow.width
                      text: "• " + (reviewRound.modelData.issues || []).join("\n• ")
                      textFormat: Text.PlainText
                      color: root.urgent
                      wrapMode: Text.Wrap
                      font.family: root.fontFamily
                      font.pixelSize: root.fs(11)
                    }
                  }
                }
                Text {
                  visible: stageRow.expanded && stageRow.reviewHistory.length === 0
                    && stageRow.reviewerIssues.length > 0
                  width: stageRow.width
                  text: "reviewer issues:\n• " + stageRow.reviewerIssues.join("\n• ")
                  textFormat: Text.PlainText
                  color: root.urgent
                  wrapMode: Text.Wrap
                  font.family: root.fontFamily
                  font.pixelSize: root.fs(11)
                }
              }
            }
            Text {
              visible: root.plan === null
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
          height: parent.height
            - y  // fill the remaining space
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
              model: ["all", "runs", "git", "reviews", "errors"]
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
            visible: !root.liveTab
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
              delegate: Rectangle {
                id: chooserRow
                required property var modelData
                readonly property bool selectable: modelData.kind === "local"
                  || modelData.kind === "remote" || modelData.kind === "path"
                width: chooserList.width
                height: rowText.implicitHeight + Style.space(10)
                radius: 4
                color: chooserRowArea.containsMouse && selectable
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
