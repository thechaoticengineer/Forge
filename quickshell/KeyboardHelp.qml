pragma ComponentBehavior: Bound

import QtQuick

// Interface: the keyboard help overlay. The rows ({key, description}; an empty
// key starts a section) enter as model, fed by PanelNavigation.helpRows(); a
// click outside the card leaves as closeRequested(). Panel.qml's keyHandler owns
// the keys that close it. Imports no shell modules.
Rectangle {
  id: help

  property var model: []
  // Palette and font sizes; Panel.qml passes its shared theme object.
  property var theme: null
  property color foreground: theme ? theme.foreground : "#dddddd"
  property color mutedForeground: theme ? theme.mutedForeground : "#aaaaaa"
  property color surface: theme ? theme.surface : "#282828"
  property color accent: theme ? theme.accent : "#6699ff"
  property string fontFamily: theme ? theme.fontFamily : "monospace"
  property real fontSize10: theme ? theme.fontSize10 : 10
  property real fontSize11: theme ? theme.fontSize11 : 11
  property real titleFontSize: 16
  property real spacing: 8

  signal closeRequested()

  color: Qt.rgba(0, 0, 0, 0.55)
  MouseArea {
    anchors.fill: parent
    onClicked: help.closeRequested()
    onWheel: wheel => { wheel.accepted = true }
  }

  Rectangle {
    anchors.centerIn: parent
    width: parent.width * 0.92
    height: parent.height * 0.9
    radius: 6
    color: help.surface
    border.width: 1
    border.color: Qt.darker(help.foreground, 3)
    MouseArea {
      anchors.fill: parent
      onWheel: wheel => { wheel.accepted = true }
    }

    Column {
      anchors.fill: parent
      anchors.margins: help.spacing * 1.5
      spacing: help.spacing

      Text {
        text: "Keyboard · :help"
        color: help.accent
        font.family: help.fontFamily
        font.pixelSize: help.titleFontSize
        font.bold: true
      }

      Text {
        width: parent.width
        text: "Normal mode uses shortcuts. Focus a text field for insert mode; Escape returns to normal. Actions follow the buttons’ enabled state."
        wrapMode: Text.Wrap
        color: help.mutedForeground
        font.family: help.fontFamily
        font.pixelSize: help.fontSize11
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
          spacing: help.spacing * 5 / 8

          Repeater {
            model: help.model

            delegate: Row {
              id: helpRow
              required property var modelData
              readonly property bool heading: modelData.key === ""
              width: helpRows.width
              spacing: help.spacing * 5 / 4

              Text {
                visible: !helpRow.heading
                width: helpRows.width * 0.36
                text: helpRow.modelData.key
                wrapMode: Text.Wrap
                color: help.accent
                font.family: help.fontFamily
                font.pixelSize: help.fontSize11
              }
              Text {
                width: helpRow.heading ? helpRows.width
                  : helpRows.width * 0.64 - helpRow.spacing
                topPadding: helpRow.heading ? help.spacing * 3 / 4 : 0
                text: helpRow.modelData.description
                wrapMode: Text.Wrap
                color: help.foreground
                font.family: help.fontFamily
                font.pixelSize: help.fontSize11
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
        color: help.mutedForeground
        font.family: help.fontFamily
        font.pixelSize: help.fontSize10
      }
    }
  }
}
