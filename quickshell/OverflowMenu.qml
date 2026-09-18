pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls

// The ⋯ menu: rare or destructive actions that do not belong to one view. It
// owns its open state, closes on a choice, on Escape or on a click outside, and
// a disabled item emits nothing.
Item {
  id: menu

  property var items: []
  property bool open: false

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

  signal itemChosen(string id)

  onOpenChanged: {
    if (open) menuPopup.open()
    else menuPopup.close()
  }
  Component.onCompleted: if (open) menuPopup.open()

  Popup {
    id: menuPopup
    width: menu.width
    padding: 6
    focus: false
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside
    enter: Transition {}
    exit: Transition {}
    onClosed: menu.open = false
    background: Rectangle {
      color: menu.surface
      radius: 4
      border.width: 1
      border.color: Qt.darker(menu.foreground, 3)
    }

    contentItem: Column {
      spacing: 2

      Repeater {
        model: menu.items
        delegate: Rectangle {
          id: menuRow
          required property var modelData
          objectName: "overflowItem_" + modelData.id
          enabled: modelData.enabled === true
          width: menuPopup.availableWidth
          height: menuText.implicitHeight + 10
          radius: 3
          color: rowArea.containsMouse ? menu.background : "transparent"
          opacity: enabled ? 1 : 0.45

          Text {
            id: menuText
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.leftMargin: 8
            anchors.rightMargin: 8
            anchors.verticalCenter: parent.verticalCenter
            text: menuRow.modelData.label
            textFormat: Text.PlainText
            elide: Text.ElideRight
            color: menu.foreground
            font.family: menu.fontFamily
            font.pixelSize: menu.fontSize11
          }

          MouseArea {
            id: rowArea
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: {
              const id = menuRow.modelData.id
              menu.open = false
              menu.itemChosen(id)
            }
          }
        }
      }
    }
  }
}
