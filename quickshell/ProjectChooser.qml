pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import qs.Commons

// Interface: discovery rows, chooser state, and palette enter as properties;
// selection and modal/focus actions leave as signals. Focusable controls and
// list navigation are exposed for the root keyboard router.
Rectangle {
  id: view

  required property bool open
  required property bool manualEntry
  required property var rowsForFilter
  required property color foreground
  required property color mutedForeground
  required property color background
  required property color surface
  required property color accent
  required property color urgent
  required property color success
  required property string fontFamily
  required property real fontSize10
  required property real fontSize11
  required property real fontSize12
  property alias chooserList: chooserList
  property alias filterField: filterField
  property alias manualField: manualField

  signal closeRequested
  signal rowChosen(var row)
  signal manualPathRequested(string path)
  signal helpRequested
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

        Rectangle {
          width: parent.width - closeChooserButton.width - parent.spacing
          height: Style.space(28)
          color: view.background
          radius: 4
          border.width: 1
          border.color: filterField.activeFocus ? view.accent : Qt.darker(view.foreground, 3)

          TextInput {
            id: filterField

            onAccepted: {
              chooserList.resetSelection()
              chooserList.activateSelection()
            }
            Keys.onPressed: event => {
              if (event.key === Qt.Key_F1) {
                view.helpRequested()
                view.leaveRequested()
                event.accepted = true
              }
            }
            Keys.onEscapePressed: event => {
              view.leaveRequested()
              event.accepted = true
            }
            anchors.fill: parent
            anchors.margins: Style.space(6)
            verticalAlignment: TextInput.AlignVCenter
            color: view.foreground
            font.family: view.fontFamily
            font.pixelSize: view.fontSize12
            clip: true

            Text {
              visible: filterField.text === "" && !filterField.activeFocus
              text: "filter projects…"
              color: view.mutedForeground
              font.family: view.fontFamily
              font.pixelSize: view.fontSize12
            }
          }
        }

        ViewButton {
          id: closeChooserButton

          label: "Close"
          onClicked: view.closeRequested()
        }
      }

      ListView {
        id: chooserList

        width: parent.width
        height: parent.height - y - (view.manualEntry ? Style.space(38) : 0)
        clip: true
        spacing: 2
        model: view.rowsForFilter(filterField.text)
        currentIndex: -1
        onModelChanged: resetSelection()

        function isSelectable(row) {
          return row && (row.kind === "local" || row.kind === "remote" || row.kind === "path")
        }

        function moveSelection(direction) {
          const currentRows = model || []
          for (let index = currentIndex + direction; index >= 0 && index < currentRows.length; index += direction) {
            if (!isSelectable(currentRows[index]))
              continue
            currentIndex = index
            positionViewAtIndex(index, ListView.Contain)
            return
          }
        }

        function resetSelection() {
          currentIndex = -1
          moveSelection(1)
        }

        function activateSelection() {
          const row = model && model[currentIndex]
          if (isSelectable(row))
            view.rowChosen(row)
        }

        delegate: Rectangle {
          id: chooserRow

          required property var modelData
          required property int index
          readonly property bool selectable: chooserList.isSelectable(modelData)
          width: chooserList.width
          height: (chooserError.visible ? chooserError.implicitHeight : rowText.implicitHeight) + Style.space(10)
          radius: 4
          color: selectable && (chooserRowArea.containsMouse || chooserList.currentIndex === index) ? Qt.darker(view.accent, 2.8) : "transparent"

          DetailFields {
            id: chooserError

            visible: chooserRow.modelData.kind === "note"
            width: parent.width - Style.space(12)
            x: Style.space(6)
            y: Style.space(5)
            entries: [{key: "text", label: "Project discovery error", text: visible ? chooserRow.modelData.label : "", error: true}]
            foreground: view.mutedForeground
            mutedForeground: view.mutedForeground
            background: view.background
            urgent: view.urgent
            fontFamily: view.fontFamily
            fontSize: view.fontSize11
            onCopyRequested: original => Quickshell.clipboardText = original
            onLeaveRequested: view.leaveRequested()
            onInspecting: view.detailInspected(chooserError)
          }

          Row {
            visible: !chooserError.visible
            anchors.verticalCenter: parent.verticalCenter
            x: Style.space(6)
            spacing: Style.space(8)

            Text {
              id: rowText

              text: chooserRow.modelData.kind === "local" || chooserRow.modelData.kind === "remote" ? chooserRow.modelData.name : chooserRow.modelData.label
              color: chooserRow.modelData.kind === "header" ? view.accent : chooserRow.modelData.kind === "local" || chooserRow.modelData.kind === "remote" ? view.foreground : view.mutedForeground
              font.family: view.fontFamily
              font.bold: chooserRow.modelData.kind === "header"
              font.pixelSize: chooserRow.modelData.kind === "note" ? view.fontSize10 : view.fontSize12
            }

            Text {
              visible: chooserRow.modelData.kind === "local"
              text: chooserRow.modelData.path || ""
              color: view.mutedForeground
              font.family: view.fontFamily
              font.pixelSize: view.fontSize10
              anchors.verticalCenter: parent.verticalCenter
            }

            Text {
              visible: chooserRow.modelData.kind === "remote"
              text: (chooserRow.modelData.isPrivate ? "private · " : "") + (chooserRow.modelData.cloned ? "cloned" : "will clone")
              color: chooserRow.modelData.cloned ? view.success : view.mutedForeground
              font.family: view.fontFamily
              font.pixelSize: view.fontSize10
              anchors.verticalCenter: parent.verticalCenter
            }
          }

          MouseArea {
            id: chooserRowArea

            anchors.fill: parent
            hoverEnabled: true
            enabled: chooserRow.selectable
            onClicked: view.rowChosen(chooserRow.modelData)
          }
        }
      }

      Row {
        id: manualRow

        visible: view.manualEntry
        width: parent.width
        spacing: Style.space(8)

        Rectangle {
          width: parent.width - manualSetButton.width - parent.spacing
          height: Style.space(28)
          color: view.background
          radius: 4
          border.width: 1
          border.color: manualField.activeFocus ? view.accent : Qt.darker(view.foreground, 3)

          TextInput {
            id: manualField

            onAccepted: manualSetButton.clicked()
            Keys.onPressed: event => {
              if (event.key === Qt.Key_F1) {
                view.helpRequested()
                view.leaveRequested()
                event.accepted = true
              }
            }
            Keys.onEscapePressed: event => {
              view.leaveRequested()
              event.accepted = true
            }
            anchors.fill: parent
            anchors.margins: Style.space(6)
            verticalAlignment: TextInput.AlignVCenter
            color: view.foreground
            font.family: view.fontFamily
            font.pixelSize: view.fontSize12
            clip: true
          }
        }

        ViewButton {
          id: manualSetButton

          label: "Set"
          onClicked: view.manualPathRequested(manualField.text)
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
