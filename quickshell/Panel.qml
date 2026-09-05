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
  readonly property string phase: engineState ? engineState.phase : "offline"
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
  property string agentLog: ""
  property double agentLogOffset: 0
  property string logSession: ""
  property bool agentLogPending: false

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
    api("GET", "/api/agent_log?offset=" + agentLogOffset, null, function(resp) {
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
    api("GET", "/api/diff", null, function(resp) {
      root.diffPending = false
      if (resp && typeof resp.diff === "string") {
        root.diffError = ""
        root.diffText = resp.diff
      } else {
        root.diffError = "Unable to load diff"
      }
    })
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
      if (!visible && !root.closingFromHost && root.shell
          && typeof root.shell.hide === "function")
        root.shell.hide(root.pluginId)
    }

    Rectangle {
      anchors.fill: parent
      color: root.background

      Column {
        anchors.fill: parent
        anchors.margins: Style.space(16)
        spacing: Style.space(10)

        // ---------------------------------------------------- header
        Row {
          width: parent.width
          spacing: Style.space(10)

          Text {
            text: "FORGE"
            color: root.accent
            font.family: root.fontFamily
            font.pixelSize: root.fs(18)
            font.bold: true
          }
          Rectangle {
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

        // ------------------------------------------------ live agent
        Rectangle {
          id: agentCard
          visible: root.agentActive
          width: parent.width
          height: agentSummary.implicitHeight + Style.space(16)
          color: root.surface
          radius: 4

          Row {
            anchors.fill: parent
            anchors.margins: Style.space(8)
            spacing: Style.space(10)
            Text {
              id: agentSummary
              width: Math.min(implicitWidth, parent.width * 0.45)
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
              text: root.agentElapsed()
              color: root.foreground
              font.family: root.fontFamily
              font.pixelSize: root.fs(11)
            }
            Text {
              width: Math.max(0, parent.width - agentSummary.width - agentTime.width
                - parent.spacing * 2)
              text: root.agentActive ? root.agent.last_line : ""
              textFormat: Text.PlainText
              color: root.mutedForeground
              elide: Text.ElideRight
              font.family: root.fontFamily
              font.pixelSize: root.fs(11)
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
            enabled: !root.busy && root.engineOnline
            onClicked: root.openChooser()
          }
        }

        Row {
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
            label: "checker: "
              + (root.engineState ? root.engineState.settings.checker : "…")
            onClicked: root.cycleTool("checker")
          }
          PanelButton {
            label: "push at end: "
              + (root.engineState && root.engineState.settings.auto_push ? "yes" : "no")
            onClicked: root.act("/api/settings", {
              auto_push: !(root.engineState && root.engineState.settings.auto_push) })
          }
        }

        // ---------------------------------------------------- goal
        Rectangle {
          width: parent.width
          height: Style.space(52)
          color: root.surface
          radius: 4
          border.width: 1
          border.color: goalField.activeFocus
            ? root.accent : Qt.darker(root.foreground, 3)
          TextEdit {
            id: goalField
            anchors.fill: parent
            anchors.margins: Style.space(6)
            wrapMode: TextEdit.Wrap
            color: root.foreground
            font.family: root.fontFamily
            font.pixelSize: root.fs(12)
            Text {
              visible: goalField.text === "" && !goalField.activeFocus
              text: "What should be built?"
              color: root.mutedForeground
              font.family: root.fontFamily
              font.pixelSize: root.fs(12)
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
            enabled: root.phase === "running"
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
        }

        Text {
          visible: root.localError !== ""
          text: root.localError
          color: root.urgent
          font.family: root.fontFamily
          font.pixelSize: root.fs(11)
        }

        // ------------------------------------------------- stages
        Rectangle {
          width: parent.width
          height: parent.height * 0.34
          color: root.surface
          radius: 4
          ListView {
            id: stageList
            anchors.fill: parent
            anchors.margins: Style.space(8)
            clip: true
            spacing: Style.space(6)
            model: root.plan ? root.plan.stages : []
            delegate: Column {
              id: stageRow
              required property var modelData
              readonly property bool expanded: root.expandedStageId === modelData.id
              readonly property double elapsedSecs: modelData.status === "in_progress"
                && typeof modelData.started_unix === "number"
                ? Math.max(0, Math.floor(root.agentNow - modelData.started_unix))
                : typeof modelData.duration_secs === "number"
                  ? Math.max(0, Math.floor(modelData.duration_secs)) : -1
              readonly property var checkerIssues: modelData.last_verdict
                && modelData.last_verdict.issues ? modelData.last_verdict.issues : []
              width: stageList.width
              spacing: 2
              TapHandler {
                onTapped: root.expandedStageId = stageRow.expanded
                  ? -1 : stageRow.modelData.id
              }
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
              Text {
                visible: stageRow.expanded && stageRow.checkerIssues.length > 0
                width: stageRow.width
                text: "checker issues:\n• " + stageRow.checkerIssues.join("\n• ")
                textFormat: Text.PlainText
                color: root.urgent
                wrapMode: Text.Wrap
                font.family: root.fontFamily
                font.pixelSize: root.fs(11)
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
          ListView {
            id: historyList
            visible: !root.liveTab
            anchors.fill: parent
            anchors.margins: Style.space(8)
            clip: true
            model: root.engineState ? root.engineState.history : []
            onCountChanged: positionViewAtEnd()
            delegate: Text {
              id: historyRow
              required property var modelData
              width: historyList.width
              text: historyRow.modelData.t + "  [" + historyRow.modelData.kind + "]  "
                + historyRow.modelData.text
              color: historyRow.modelData.kind === "error" ? root.urgent
                : historyRow.modelData.kind === "git" ? root.success
                : historyRow.modelData.kind === "check" ? root.working
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
    signal clicked()

    width: buttonText.implicitWidth + Style.space(18)
    height: buttonText.implicitHeight + Style.space(10)
    radius: 4
    color: button.primary && button.enabled ? root.accent : root.surface
    border.width: button.primary && button.enabled ? 0 : 1
    border.color: Qt.darker(root.foreground, 3)
    opacity: button.enabled ? (buttonArea.containsMouse ? 0.85 : 1.0) : 0.45

    Text {
      id: buttonText
      anchors.centerIn: parent
      text: button.label
      color: button.primary && button.enabled ? root.background : root.foreground
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
