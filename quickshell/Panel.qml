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

  // Engine snapshot, refreshed by polling http://127.0.0.1:8734/api/state.
  property var engineState: null
  property bool engineOnline: false
  property string apiBase: "http://127.0.0.1:8734"
  property string localError: ""

  readonly property string pluginId: manifest && manifest.id
    ? manifest.id : "dev.omarchy-ai-build-orchestrator"
  readonly property color foreground: Color.foreground
  readonly property color mutedForeground: Qt.darker(Color.foreground, 1.45)
  readonly property color background: Color.background
  readonly property color surface: Color.popups.background
  readonly property color accent: Color.accent
  readonly property color urgent: Color.urgent
  readonly property string fontFamily: Style.font.family

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

  function open(payloadJson) {
    closingFromHost = false
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
    interval: 2000
    repeat: true
    running: window.visible
    onTriggered: root.refresh()
  }

  FloatingWindow {
    id: window
    visible: false
    title: "Forge"
    color: root.background
    implicitWidth: 760
    implicitHeight: 620
    minimumSize: Qt.size(560, 440)

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
            font.pixelSize: Style.fontSize(18)
            font.bold: true
          }
          Rectangle {
            width: phaseText.implicitWidth + Style.space(16)
            height: phaseText.implicitHeight + Style.space(6)
            radius: height / 2
            color: "transparent"
            border.width: 1
            border.color: root.phase === "failed" || root.phase === "blocked"
              ? root.urgent : root.accent
            Text {
              id: phaseText
              anchors.centerIn: parent
              text: root.engineOnline ? root.phase : "engine offline"
              color: root.phase === "failed" || root.phase === "blocked"
                ? root.urgent : root.foreground
              font.family: root.fontFamily
              font.pixelSize: Style.fontSize(11)
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
            font.pixelSize: Style.fontSize(12)
            anchors.verticalCenter: parent.verticalCenter
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
              font.pixelSize: Style.fontSize(13)
              font.bold: true
            }
            Text {
              width: parent.width
              text: root.engineState ? root.engineState.project : ""
              color: root.mutedForeground
              elide: Text.ElideMiddle
              font.family: root.fontFamily
              font.pixelSize: Style.fontSize(10)
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
            font.pixelSize: Style.fontSize(12)
            Text {
              visible: goalField.text === "" && !goalField.activeFocus
              text: "What should be built?"
              color: root.mutedForeground
              font.family: root.fontFamily
              font.pixelSize: Style.fontSize(12)
            }
          }
        }

        Row {
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
        }

        Text {
          visible: root.localError !== ""
          text: root.localError
          color: root.urgent
          font.family: root.fontFamily
          font.pixelSize: Style.fontSize(11)
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
              width: stageList.width
              spacing: 2
              Row {
                spacing: Style.space(8)
                Text {
                  text: stageRow.modelData.id + ". " + stageRow.modelData.title
                  color: root.foreground
                  font.family: root.fontFamily
                  font.pixelSize: Style.fontSize(12)
                  font.bold: true
                }
                Text {
                  text: stageRow.modelData.status
                    + (stageRow.modelData.sha ? " " + stageRow.modelData.sha : "")
                    + (stageRow.modelData.rounds > 1
                       ? " (round " + stageRow.modelData.rounds + ")" : "")
                  color: stageRow.modelData.status === "committed" ? root.accent
                    : stageRow.modelData.status === "blocked" ? root.urgent
                    : root.mutedForeground
                  font.family: root.fontFamily
                  font.pixelSize: Style.fontSize(11)
                }
              }
              Text {
                text: stageRow.modelData.commit
                color: root.accent
                font.family: root.fontFamily
                font.pixelSize: Style.fontSize(11)
              }
              Text {
                width: stageList.width
                text: stageRow.modelData.instructions
                color: root.mutedForeground
                wrapMode: Text.Wrap
                maximumLineCount: 3
                elide: Text.ElideRight
                font.family: root.fontFamily
                font.pixelSize: Style.fontSize(11)
              }
            }
            Text {
              visible: root.plan === null
              text: "no plan yet"
              color: root.mutedForeground
              font.family: root.fontFamily
              font.pixelSize: Style.fontSize(12)
            }
          }
        }

        // ------------------------------------------------- history
        Rectangle {
          width: parent.width
          height: parent.height
            - y  // fill the remaining space
          color: root.surface
          radius: 4
          ListView {
            id: historyList
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
                : historyRow.modelData.kind === "git" ? root.accent
                : root.mutedForeground
              wrapMode: Text.Wrap
              font.family: root.fontFamily
              font.pixelSize: Style.fontSize(10)
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
                  font.pixelSize: Style.fontSize(12)
                  clip: true
                  Text {
                    visible: filterField.text === "" && !filterField.activeFocus
                    text: "filter projects…"
                    color: root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: Style.fontSize(12)
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
                    font.pixelSize: Style.fontSize(
                      chooserRow.modelData.kind === "note" ? 10 : 12)
                  }
                  Text {
                    visible: chooserRow.modelData.kind === "local"
                    text: chooserRow.modelData.path || ""
                    color: root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: Style.fontSize(10)
                    anchors.verticalCenter: parent.verticalCenter
                  }
                  Text {
                    visible: chooserRow.modelData.kind === "remote"
                    text: (chooserRow.modelData.isPrivate ? "private · " : "")
                      + (chooserRow.modelData.cloned ? "cloned" : "will clone")
                    color: chooserRow.modelData.cloned
                      ? root.accent : root.mutedForeground
                    font.family: root.fontFamily
                    font.pixelSize: Style.fontSize(10)
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
                  font.pixelSize: Style.fontSize(12)
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
    color: button.primary ? root.accent : root.surface
    border.width: button.primary ? 0 : 1
    border.color: Qt.darker(root.foreground, 3)
    opacity: button.enabled ? (buttonArea.containsMouse ? 0.85 : 1.0) : 0.35

    Text {
      id: buttonText
      anchors.centerIn: parent
      text: button.label
      color: button.primary ? root.background : root.foreground
      font.family: root.fontFamily
      font.pixelSize: Style.fontSize(11)
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
