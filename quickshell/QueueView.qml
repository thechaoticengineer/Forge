pragma ComponentBehavior: Bound

import QtQuick
import "PanelDetails.js" as PanelDetails

// Interface: the queued goals, the Start queue guard and the move rule enter
// as properties; Start queue, ↑, ↓ and × leave as signals carrying the goal id
// and direction, which Panel.qml sends to /api/queue/start, move and remove.
// Imports no shell modules, so qmltestrunner loads it on its own.
Item {
  id: view

  required property var queue
  required property bool engineOnline
  required property bool startEnabled
  // function(index, step): whether the goal at index may swap places with its neighbour.
  required property var canMove
  property string detailScope: ""
  // Palette and font sizes; Panel.qml passes its shared theme object.
  property var theme: null
  property color foreground: theme ? theme.foreground : "#dddddd"
  property color mutedForeground: theme ? theme.mutedForeground : "#aaaaaa"
  property color background: theme ? theme.background : "#202020"
  property color surface: theme ? theme.surface : "#282828"
  property color accent: theme ? theme.accent : "#6699ff"
  property color urgent: theme ? theme.urgent : "#ff6666"
  property color success: theme ? theme.success : foreground
  property color working: theme ? theme.working : foreground
  property string fontFamily: theme ? theme.fontFamily : "monospace"
  property real fontSize10: theme ? theme.fontSize10 : 10
  property real fontSize11: theme ? theme.fontSize11 : 11
  property real fontSize12: theme ? theme.fontSize12 : 12
  property real spacing: 8
  property real horizontalPadding: 18
  property real verticalPadding: 10
  property alias listView: queueList

  signal startRequested()
  signal moveRequested(var id, string dir)
  signal removeRequested(var id)
  signal copyRequested(string original)
  signal leaveRequested()
  signal detailRevealed(var control)
  signal detailInspected(var control)

  Row {
    id: queueHeader

    anchors.top: parent.top
    anchors.left: parent.left
    anchors.right: parent.right
    spacing: view.spacing

    Text {
      width: parent.width - startQueueButton.width - parent.spacing
      anchors.verticalCenter: parent.verticalCenter
      text: "Queue (" + view.queue.length + ")"
      color: view.foreground
      font.family: view.fontFamily
      font.pixelSize: view.fontSize12
      font.bold: true
    }

    QueueButton {
      id: startQueueButton

      objectName: "startQueue"
      label: "Start queue"
      primary: true
      enabled: view.startEnabled
      onClicked: view.startRequested()
    }
  }

  Text {
    objectName: "queueEmpty"
    visible: view.queue.length === 0
    anchors.top: queueHeader.bottom
    anchors.topMargin: view.spacing
    anchors.left: parent.left
    anchors.right: parent.right
    text: "The queue is empty. Add a goal from Overview with Add to queue."
    textFormat: Text.PlainText
    wrapMode: Text.Wrap
    color: view.mutedForeground
    font.family: view.fontFamily
    font.pixelSize: view.fontSize11
  }

  ListView {
    id: queueList

    anchors.top: queueHeader.bottom
    anchors.topMargin: view.spacing
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.bottom: parent.bottom
    clip: true
    spacing: view.spacing / 2
    boundsBehavior: Flickable.StopAtBounds
    model: view.queue
    delegate: Row {
      id: queueRow

      required property var modelData
      required property int index
      readonly property bool active: modelData.status === "planning"
        || modelData.status === "awaiting_approval" || modelData.status === "running"
      readonly property bool stopped: modelData.status === "blocked" || modelData.status === "failed"

      objectName: "queueRow"
      width: queueList.width
      height: Math.max(queueGoal.implicitHeight, queueControls.implicitHeight)
      spacing: view.spacing

      Text {
        id: queueGlyph

        width: view.fontSize12
        anchors.verticalCenter: parent.verticalCenter
        text: queueRow.active ? "●" : queueRow.modelData.status === "done" ? "✓"
          : queueRow.stopped ? "!" : "·"
        color: queueRow.active ? view.accent : queueRow.stopped ? view.urgent : view.mutedForeground
        font.family: view.fontFamily
        font.pixelSize: view.fontSize12
        font.bold: true
      }

      DetailFields {
        id: queueGoal

        width: Math.max(0, parent.width - queueGlyph.width - parent.spacing
          - (queueControls.visible ? queueControls.width + parent.spacing : 0))
        scope: view.detailScope
        entries: [PanelDetails.field("text", "Goal", queueRow.modelData.goal, false)]
        foreground: view.mutedForeground
        mutedForeground: view.mutedForeground
        background: view.background
        urgent: view.urgent
        fontFamily: view.fontFamily
        fontSize: view.fontSize11
        onCopyRequested: original => view.copyRequested(original)
        onLeaveRequested: view.leaveRequested()
        onFocusRevealed: control => view.detailRevealed(control)
        onInspecting: view.detailInspected(queueGoal)
      }

      Row {
        id: queueControls

        visible: queueRow.modelData.status === "queued" || queueRow.stopped
        spacing: view.spacing / 2

        QueueButton {
          objectName: "queueUp"
          label: "↑"
          visible: queueRow.modelData.status === "queued"
          enabled: view.engineOnline && view.canMove(queueRow.index, -1)
          onClicked: view.moveRequested(queueRow.modelData.id, "up")
        }
        QueueButton {
          objectName: "queueDown"
          label: "↓"
          visible: queueRow.modelData.status === "queued"
          enabled: view.engineOnline && view.canMove(queueRow.index, 1)
          onClicked: view.moveRequested(queueRow.modelData.id, "down")
        }
        QueueButton {
          objectName: "queueRemove"
          label: "×"
          enabled: view.engineOnline
          onClicked: view.removeRequested(queueRow.modelData.id)
        }
      }
    }
  }

  component QueueButton: PanelViewButton {
    foreground: view.foreground
    background: view.background
    surface: view.surface
    accent: view.accent
    fontFamily: view.fontFamily
    fontSize: view.fontSize11
    horizontalPadding: view.horizontalPadding
    verticalPadding: view.verticalPadding
  }
}
