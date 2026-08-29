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
  property bool editingDraft: false
  property string localDraftError: ""

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
    if (shell && typeof shell.hide === "function") shell.hide(pluginId)
    else window.visible = false
  }

  function moveSelection(delta) {
    if (editingDraft) return
    var count = sectionModel.count
    selectedSection = ((selectedSection + delta) % count + count) % count
  }

  function beginDraftEntry() {
    if (!engine.connected || engine.requestPending) return
    selectedSection = 0
    editingDraft = true
    localDraftError = ""
    engine.requestError = ""
    Qt.callLater(function() { repositoryField.forceActiveFocus() })
  }

  function cancelDraftEntry() {
    if (engine.requestPending) return
    editingDraft = false
    localDraftError = ""
    repositoryField.focus = false
    goalField.focus = false
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  function submitDraft() {
    var repository = repositoryField.text.trim()
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

  function draftError() {
    return localDraftError || engine.requestError
  }

  function footerHelp() {
    if (editingDraft) return "Tab  Next field    Enter  Continue or create    Esc  Cancel"
    if (selectedSection === 0 && engine.activeRun) return "j/k  Navigate    n  New draft    r  Reconnect    Esc  Close"
    if (selectedSection === 0) return "j/k  Navigate    Enter  Create draft    r  Reconnect    Esc  Close"
    return "j/k or arrows  Navigate    r  Reconnect    Esc  Close"
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
      repositoryField.text = ""
      goalField.text = ""
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
      description: "Explicit tasks, dependencies, and acceptance criteria will appear here."
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
        onMoveRequested: function(dx, dy) {
          if (dy !== 0) root.moveSelection(dy)
          else if (dx !== 0) root.moveSelection(dx)
        }
        onActivateRequested: {
          if (root.selectedSection === 0 && !root.editingDraft) root.beginDraftEntry()
        }
        onCloseRequested: root.requestClose()
        onTextKey: function(text) {
          if (text === "r") engine.reconnect()
          else if (text === "n") root.beginDraftEntry()
        }

        Column {
          anchors.fill: parent
          anchors.margins: Style.space(20)
          spacing: Style.space(18)

          Row {
            width: parent.width
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
            width: parent.width
            height: 1
            color: Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.18)
          }

          Row {
            width: parent.width
            height: parent.height - Style.space(126)
            spacing: Style.space(18)

            ListView {
              id: sectionList
              width: Math.min(Style.space(220), parent.width * 0.32)
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
                  ? Qt.rgba(root.accent.r, root.accent.g, root.accent.b, 0.18)
                  : "transparent"
                border.width: index === root.selectedSection ? 1 : 0
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
              spacing: Style.space(14)

              Text {
                text: sectionModel.get(root.selectedSection).title
                color: root.foreground
                font.family: root.fontFamily
                font.pixelSize: Style.font.title
                font.bold: true
              }

              Text {
                width: parent.width
                text: sectionModel.get(root.selectedSection).description
                color: root.mutedForeground
                font.family: root.fontFamily
                font.pixelSize: Style.font.body
                wrapMode: Text.WordWrap
              }

              Rectangle {
                width: parent.width
                height: contentPane.parent.height - Style.space(92)
                radius: Style.cornerRadius
                color: root.surface
                border.width: 1
                border.color: Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.16)

                Column {
                  anchors.fill: parent
                  anchors.margins: Style.space(16)
                  spacing: Style.space(10)

                  Text {
                    text: {
                      if (root.selectedSection !== 0) return "Planned capability"
                      if (!engine.connected) return "Start the Rust engine to connect"
                      if (root.editingDraft) return "Create a durable draft run"
                      if (engine.activeRun) return "Draft saved"
                      return "No active run"
                    }
                    color: root.foreground
                    font.family: root.fontFamily
                    font.pixelSize: Style.font.body
                    font.bold: true
                  }

                  Text {
                    visible: root.selectedSection !== 0
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
                    visible: root.selectedSection === 0 && root.editingDraft
                    width: parent.width
                    spacing: Style.space(8)

                    Text {
                      text: "Repository"
                      color: root.foreground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      font.bold: true
                    }

                    TextField {
                      id: repositoryField
                      width: parent.width
                      enabled: !engine.requestPending
                      placeholderText: "/absolute/path/to/repository"
                      foreground: root.foreground
                      accent: root.accent

                      Keys.onPressed: function(event) {
                        if (event.key === Qt.Key_Escape) {
                          root.cancelDraftEntry(); event.accepted = true
                        } else if (event.key === Qt.Key_Tab || event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                          goalField.forceActiveFocus(); event.accepted = true
                        }
                      }
                    }

                    Text {
                      text: "Goal"
                      color: root.foreground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      font.bold: true
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
                          repositoryField.forceActiveFocus(); event.accepted = true
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
                        text: "Cancel"
                        enabled: !engine.requestPending
                        foreground: root.foreground
                        accent: root.accent
                        onClicked: root.cancelDraftEntry()
                      }
                    }

                    Text {
                      visible: root.draftError() !== ""
                      width: parent.width
                      text: root.draftError()
                      color: root.urgent
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      wrapMode: Text.WordWrap
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
                      text: "This draft survives engine and shell restarts. Planning and agent assignment are the next workflow stage and are not implemented yet."
                      color: root.mutedForeground
                      font.family: root.fontFamily
                      font.pixelSize: Style.font.bodySmall
                      wrapMode: Text.WordWrap
                    }

                    Button {
                      text: "New draft"
                      foreground: root.foreground
                      accent: root.accent
                      onClicked: root.beginDraftEntry()
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
                }
              }
            }
          }

          Text {
            text: root.footerHelp()
            color: root.mutedForeground
            font.family: root.fontFamily
            font.pixelSize: Style.font.bodySmall
          }
        }
      }
    }
  }
}
