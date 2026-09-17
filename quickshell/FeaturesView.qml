pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls

// Interface: discovered feature rows, pending/error state and palette enter
// as properties; refresh, close and opening a feature in an editor leave as
// signals. listView is exposed for the root keyboard router. Deliberately
// avoids qs.Commons (unlike DiffView/ProjectChooser) so this component loads
// standalone under qmltestrunner, matching DiscussionChat's convention.
Rectangle {
  id: view

  required property bool open
  required property bool pending
  required property string errorText
  required property var rows
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
  property alias listView: featuresList

  signal closeRequested()
  signal refreshRequested()
  signal openRequested(var feature)
  signal leaveRequested()

  visible: view.open
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
      anchors.margins: 12
      spacing: 8

      Row {
        width: parent.width
        spacing: 8

        Text {
          width: parent.width - refreshFeaturesButton.width - closeFeaturesButton.width - parent.spacing * 2
          anchors.verticalCenter: parent.verticalCenter
          text: "Features"
          color: view.foreground
          font.family: view.fontFamily
          font.pixelSize: view.fontSize11
          font.bold: true
          elide: Text.ElideRight
        }

        ViewButton {
          id: refreshFeaturesButton

          label: "Refresh"
          enabled: !view.pending
          onClicked: view.refreshRequested()
        }

        ViewButton {
          id: closeFeaturesButton

          label: "Close"
          onClicked: view.closeRequested()
        }
      }

      Text {
        objectName: "featuresError"
        visible: view.errorText !== ""
        width: parent.width
        text: view.errorText
        wrapMode: Text.Wrap
        color: view.urgent
        font.family: view.fontFamily
        font.pixelSize: view.fontSize10
      }

      Text {
        objectName: "featuresEmptyState"
        visible: view.rows.length === 0 && view.errorText === ""
        width: parent.width
        text: "No feature specs in docs/features/"
        wrapMode: Text.Wrap
        color: view.mutedForeground
        font.family: view.fontFamily
        font.pixelSize: view.fontSize10
      }

      ListView {
        id: featuresList

        objectName: "featuresList"
        width: parent.width
        height: parent.height - y
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        model: view.rows
        currentIndex: -1
        onModelChanged: resetSelection()

        function moveSelection(direction) {
          const count = (model || []).length
          if (count === 0) return
          let index = currentIndex + direction
          if (index < 0) index = 0
          else if (index >= count) index = count - 1
          currentIndex = index
          positionViewAtIndex(index, ListView.Contain)
        }

        function resetSelection() {
          currentIndex = (model || []).length > 0 ? 0 : -1
        }

        function activateSelection() {
          const row = model && model[currentIndex]
          if (row) view.openRequested(row)
        }

        delegate: Rectangle {
          id: featureRow

          required property var modelData
          required property int index
          width: featuresList.width
          height: rowColumn.implicitHeight + 12
          radius: 4
          color: featuresList.currentIndex === index ? Qt.darker(view.accent, 2.8) : "transparent"

          Column {
            id: rowColumn

            x: 6
            y: 6
            width: parent.width - 12 - openFeatureButton.width - 8
            spacing: 2

            Row {
              spacing: 8

              Text {
                objectName: "featureTitle"
                text: featureRow.modelData.title
                color: view.foreground
                font.family: view.fontFamily
                font.bold: true
                font.pixelSize: view.fontSize11
              }

              Text {
                objectName: "featureSlug"
                text: featureRow.modelData.slug
                color: view.mutedForeground
                font.family: view.fontFamily
                font.pixelSize: view.fontSize10
              }

              Text {
                objectName: "featureStatus"
                text: featureRow.modelData.status
                color: featureRow.modelData.valid ? view.success : view.urgent
                font.family: view.fontFamily
                font.bold: true
                font.pixelSize: view.fontSize10
              }
            }

            Text {
              objectName: "featureReasons"
              visible: !featureRow.modelData.valid && featureRow.modelData.reasons.length > 0
              width: rowColumn.width
              text: featureRow.modelData.reasons.join("; ")
              wrapMode: Text.Wrap
              color: view.mutedForeground
              font.family: view.fontFamily
              font.pixelSize: view.fontSize10
            }
          }

          ViewButton {
            id: openFeatureButton

            objectName: "featureOpenButton"
            anchors.right: parent.right
            anchors.rightMargin: 6
            anchors.verticalCenter: parent.verticalCenter
            label: "Open in nvim"
            onClicked: view.openRequested(featureRow.modelData)
          }
        }
      }
    }
  }

  component ViewButton: Rectangle {
    id: button

    property string label: ""

    signal clicked()

    implicitWidth: buttonText.implicitWidth + 18
    width: implicitWidth
    height: buttonText.implicitHeight + 10
    radius: 4
    color: view.surface
    border.width: 1
    border.color: Qt.darker(view.foreground, 3)
    opacity: button.enabled ? (buttonArea.containsMouse ? 0.85 : 1.0) : 0.45

    Text {
      id: buttonText

      anchors.centerIn: parent
      text: button.label
      color: view.foreground
      font.family: view.fontFamily
      font.pixelSize: view.fontSize11
    }

    MouseArea {
      id: buttonArea

      anchors.fill: parent
      hoverEnabled: true
      onClicked: button.clicked()
    }
  }
}
