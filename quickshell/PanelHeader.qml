pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import "PanelNavigation.js" as PanelNavigation

// Fixed panel header: FORGE, the project switcher, background activity, the phase
// badge, the current step and the ⋯ button. It never scrolls with a view.
Item {
  id: header

  property string projectName: ""
  property string projectPath: ""
  property var sessions: []
  property string activeProject: ""
  property string phase: ""
  property bool engineOnline: false
  property bool busy: false
  property string stepText: ""
  property bool backgroundBusy: false
  property int activeProjectCount: 0

  property color foreground: "white"
  property color mutedForeground: "gray"
  property color background: "black"
  property color surface: "black"
  property color accent: "orange"
  property color urgent: "red"
  property color success: "green"
  property color working: "yellow"
  property string fontFamily: "monospace"
  property real fontSize10: 10
  property real fontSize11: 11
  property real fontSize12: 12
  property real titleFontSize: 18
  property real spacing: 10

  readonly property bool switcherOpen: switcherPopup.visible
  readonly property color phaseColor: phase === "failed" || phase === "blocked" ? urgent
    : busy ? working : phase === "done" ? success : mutedForeground

  signal projectSelected(string path)
  signal changeProjectRequested()
  signal overflowRequested()

  function openSwitcher() { switcherPopup.open() }
  function closeSwitcher() { switcherPopup.close() }

  implicitHeight: Math.max(forgeTitle.implicitHeight, projectSwitcher.height, overflowButton.height)
  height: implicitHeight

  Row {
    id: headerRow
    anchors.left: parent.left
    anchors.right: overflowButton.left
    anchors.rightMargin: header.spacing
    anchors.verticalCenter: parent.verticalCenter
    spacing: header.spacing

    Text {
      id: forgeTitle
      objectName: "forgeTitle"
      anchors.verticalCenter: parent.verticalCenter
      text: "FORGE"
      color: header.accent
      font.family: header.fontFamily
      font.pixelSize: header.titleFontSize
      font.bold: true
    }

    Rectangle {
      id: projectSwitcher
      objectName: "projectSwitcher"
      anchors.verticalCenter: parent.verticalCenter
      width: Math.min(switcherText.implicitWidth + 16, header.width * 0.4)
      height: switcherText.implicitHeight + 10
      radius: 4
      color: header.surface
      border.width: 1
      border.color: header.switcherOpen ? header.accent : Qt.darker(header.foreground, 3)

      Text {
        id: switcherText
        anchors.centerIn: parent
        width: Math.max(0, parent.width - 16)
        text: header.projectName + " ▾"
          + (header.sessions.length > 1 ? " · " + header.sessions.length + " projects" : "")
        textFormat: Text.PlainText
        elide: Text.ElideRight
        color: header.foreground
        font.family: header.fontFamily
        font.pixelSize: header.fontSize11
      }

      MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: {
          if (header.switcherOpen) header.closeSwitcher()
          else header.openSwitcher()
        }
      }

      Popup {
        id: switcherPopup
        y: projectSwitcher.height + 4
        width: Math.max(280, projectSwitcher.width)
        padding: 6
        focus: false
        closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutsideParent
        enter: Transition {}
        exit: Transition {}
        background: Rectangle {
          color: header.surface
          radius: 4
          border.width: 1
          border.color: Qt.darker(header.foreground, 3)
        }

        contentItem: Column {
          spacing: 2

          Text {
            width: switcherPopup.availableWidth
            text: header.projectPath
            visible: text !== ""
            textFormat: Text.PlainText
            elide: Text.ElideMiddle
            color: header.mutedForeground
            font.family: header.fontFamily
            font.pixelSize: header.fontSize10
            bottomPadding: 4
          }

          Repeater {
            model: header.sessions
            delegate: Rectangle {
              id: sessionRow
              required property var modelData
              readonly property bool current: modelData.project === header.activeProject
              readonly property bool needsAttention: modelData.phase === "blocked" || modelData.phase === "failed"
              readonly property string label: modelData.name + " " + PanelNavigation.sessionMarker(modelData)
              objectName: "sessionRow"
              enabled: header.engineOnline
              width: switcherPopup.availableWidth
              height: sessionText.implicitHeight + 10
              radius: 3
              color: current ? header.accent : sessionArea.containsMouse ? header.background : "transparent"
              opacity: enabled ? 1 : 0.45

              Text {
                id: sessionText
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.leftMargin: 8
                anchors.rightMargin: 8
                anchors.verticalCenter: parent.verticalCenter
                text: sessionRow.label
                textFormat: Text.PlainText
                elide: Text.ElideRight
                color: !sessionRow.current && sessionRow.needsAttention ? header.urgent
                  : sessionRow.current ? header.background : header.foreground
                font.family: header.fontFamily
                font.pixelSize: header.fontSize11
              }

              MouseArea {
                id: sessionArea
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                  header.closeSwitcher()
                  header.projectSelected(sessionRow.modelData.project)
                }
              }
            }
          }

          Rectangle {
            id: changeProjectRow
            readonly property string label: "Change project…"
            objectName: "changeProjectRow"
            enabled: header.engineOnline
            width: switcherPopup.availableWidth
            height: changeProjectText.implicitHeight + 10
            radius: 3
            color: changeProjectArea.containsMouse ? header.background : "transparent"
            opacity: enabled ? 1 : 0.45

            Text {
              id: changeProjectText
              anchors.left: parent.left
              anchors.leftMargin: 8
              anchors.verticalCenter: parent.verticalCenter
              text: changeProjectRow.label
              color: header.foreground
              font.family: header.fontFamily
              font.pixelSize: header.fontSize11
            }

            MouseArea {
              id: changeProjectArea
              anchors.fill: parent
              hoverEnabled: true
              cursorShape: Qt.PointingHandCursor
              onClicked: {
                header.closeSwitcher()
                header.changeProjectRequested()
              }
            }
          }
        }
      }
    }

    Text {
      id: projectActivity
      visible: header.backgroundBusy
      anchors.verticalCenter: parent.verticalCenter
      text: header.activeProjectCount + " project" + (header.activeProjectCount === 1 ? "" : "s") + " active"
      color: header.mutedForeground
      font.family: header.fontFamily
      font.pixelSize: header.fontSize10
    }

    Rectangle {
      id: phaseBadge
      objectName: "phaseBadge"
      anchors.verticalCenter: parent.verticalCenter
      width: phaseText.implicitWidth + 16
      height: phaseText.implicitHeight + 6
      radius: height / 2
      color: "transparent"
      border.width: 1
      border.color: header.phaseColor

      Text {
        id: phaseText
        anchors.centerIn: parent
        text: header.engineOnline ? header.phase : "engine offline"
        color: header.phase === "failed" || header.phase === "blocked" || header.busy || header.phase === "done"
          ? header.phaseColor : header.foreground
        font.family: header.fontFamily
        font.pixelSize: header.fontSize11
      }
    }

    Text {
      id: currentStep
      objectName: "currentStep"
      visible: text !== ""
      anchors.verticalCenter: parent.verticalCenter
      width: Math.max(0, headerRow.width - forgeTitle.width - projectSwitcher.width - phaseBadge.width
        - (projectActivity.visible ? projectActivity.width + headerRow.spacing : 0)
        - 3 * headerRow.spacing)
      text: header.stepText
      textFormat: Text.PlainText
      elide: Text.ElideRight
      color: header.mutedForeground
      font.family: header.fontFamily
      font.pixelSize: header.fontSize12
    }
  }

  Rectangle {
    id: overflowButton
    objectName: "overflowButton"
    readonly property string label: "⋯"
    anchors.right: parent.right
    anchors.verticalCenter: parent.verticalCenter
    width: overflowText.implicitWidth + 16
    height: overflowText.implicitHeight + 6
    radius: 4
    color: overflowArea.containsMouse ? header.surface : "transparent"

    Text {
      id: overflowText
      anchors.centerIn: parent
      text: overflowButton.label
      color: header.foreground
      font.family: header.fontFamily
      font.pixelSize: header.fontSize12 * 1.4
      font.bold: true
    }

    MouseArea {
      id: overflowArea
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onClicked: header.overflowRequested()
    }
  }
}
