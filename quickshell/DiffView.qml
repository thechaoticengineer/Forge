pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import qs.Commons

// Interface: diff state and palette enter as properties; close, refresh, and
// focus handoff leave as signals. listView is exposed for top-level key routing.
Rectangle {
  id: view

  required property bool open
  required property bool engineOnline
  required property bool pending
  required property string diffText
  required property string errorText
  required property color foreground
  required property color mutedForeground
  required property color background
  required property color surface
  required property color accent
  required property color urgent
  required property color success
  required property color info
  required property string fontFamily
  required property real fontSize10
  required property real fontSize11
  property alias listView: diffList

  signal closeRequested
  signal refreshRequested
  signal leaveRequested
  signal detailInspected(var control)

  visible: open
  color: Qt.rgba(0, 0, 0, 0.55)

  MouseArea {
    anchors.fill: parent
    onClicked: view.closeRequested()
  }

  Rectangle {
    anchors.centerIn: parent
    width: parent.width * 0.82
    height: parent.height * 0.82
    radius: 6
    color: view.surface
    border.width: 1
    border.color: Qt.darker(view.foreground, 3)

    MouseArea {
      anchors.fill: parent
    }

    Column {
      anchors.fill: parent
      anchors.margins: Style.space(12)
      spacing: Style.space(8)

      Row {
        width: parent.width
        spacing: Style.space(8)

        Text {
          width: parent.width - refreshDiffButton.width - closeDiffButton.width - parent.spacing * 2
          anchors.verticalCenter: parent.verticalCenter
          text: "Uncommitted diff"
          color: view.foreground
          font.family: view.fontFamily
          font.pixelSize: view.fontSize11
          font.bold: true
          elide: Text.ElideRight
        }

        ViewButton {
          id: refreshDiffButton

          label: "Refresh"
          enabled: view.engineOnline && !view.pending
          onClicked: view.refreshRequested()
        }

        ViewButton {
          id: closeDiffButton

          label: "Close"
          onClicked: view.closeRequested()
        }
      }

      DetailFields {
        id: diffError

        visible: view.errorText !== ""
        width: parent.width
        entries: [{key: "text", label: "Diff error", text: view.errorText, error: true}]
        foreground: view.mutedForeground
        mutedForeground: view.mutedForeground
        background: view.background
        urgent: view.urgent
        fontFamily: view.fontFamily
        fontSize: view.fontSize11
        onCopyRequested: original => Quickshell.clipboardText = original
        onLeaveRequested: view.leaveRequested()
        onInspecting: view.detailInspected(diffError)
      }

      ListView {
        id: diffList

        width: parent.width
        height: parent.height - y
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: view.diffText === "" ? [] : view.diffText.split("\n")

        delegate: Text {
          id: diffLine

          required property string modelData
          readonly property bool header: modelData.indexOf("@@") === 0 || modelData.indexOf("diff ") === 0
          width: diffList.width
          text: modelData === "" ? " " : modelData
          textFormat: Text.PlainText
          color: header ? view.info : modelData.indexOf("+") === 0 && modelData.indexOf("+++") !== 0 ? view.success : modelData.indexOf("-") === 0 && modelData.indexOf("---") !== 0 ? view.urgent : view.foreground
          font.family: view.fontFamily
          font.pixelSize: view.fontSize10
          font.bold: header
          wrapMode: Text.WrapAnywhere
        }

        Text {
          visible: view.diffText === "" && view.errorText === ""
          width: parent.width
          text: view.pending ? "Loading diff…" : "no uncommitted changes"
          color: view.mutedForeground
          font.family: view.fontFamily
          font.pixelSize: view.fontSize10
          wrapMode: Text.WrapAnywhere
        }
      }
    }
  }

  component ViewButton: PanelViewButton {
    foreground: view.foreground
    background: view.background
    surface: view.surface
    accent: view.accent
    fontFamily: view.fontFamily
    fontSize: view.fontSize11
  }
}
