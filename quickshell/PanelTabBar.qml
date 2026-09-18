pragma ComponentBehavior: Bound

import QtQuick
import "PanelNavigation.js" as PanelNavigation

// Fixed tab bar under the header. The selected tab is underlined in the accent
// color; clicking a tab asks the panel to show that view.
Item {
  id: bar

  property string currentTab: "overview"
  property int queueCount: 0

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

  signal tabRequested(string id)

  implicitHeight: tabRow.height + 1
  height: implicitHeight

  Row {
    id: tabRow
    spacing: 2

    Repeater {
      model: PanelNavigation.tabs
      delegate: Item {
        id: tab
        required property string modelData
        readonly property bool selected: bar.currentTab === modelData
        readonly property string label: PanelNavigation.tabLabel(modelData, bar.queueCount)
        objectName: "tab_" + modelData
        width: tabText.implicitWidth + 20
        height: tabText.implicitHeight + 12

        Text {
          id: tabText
          anchors.centerIn: parent
          text: tab.label
          textFormat: Text.PlainText
          color: tab.selected ? bar.accent : tabArea.containsMouse ? bar.foreground : bar.mutedForeground
          font.family: bar.fontFamily
          font.pixelSize: bar.fontSize12
          font.bold: tab.selected
        }

        Rectangle {
          visible: tab.selected
          anchors.left: parent.left
          anchors.right: parent.right
          anchors.bottom: parent.bottom
          anchors.leftMargin: 6
          anchors.rightMargin: 6
          height: 2
          color: bar.accent
        }

        MouseArea {
          id: tabArea
          anchors.fill: parent
          hoverEnabled: true
          cursorShape: Qt.PointingHandCursor
          onClicked: bar.tabRequested(tab.modelData)
        }
      }
    }
  }

  Rectangle {
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.bottom: parent.bottom
    height: 1
    color: Qt.darker(bar.foreground, 3)
  }
}
