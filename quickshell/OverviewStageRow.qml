import QtQuick

// Interface: one compact, single-line stage row: status icon, "N. title", the
// commit short hash or a status word, the duration and ›. The stage enters as a
// property; a click anywhere on the row leaves as clicked(). Shows no routing or
// review policy. Imports no shell modules, so qmltestrunner loads it on its own.
Item {
  id: row

  required property var stage
  required property int number
  // Replaces the status word, e.g. the current step of the stage being worked on.
  property string activityText: ""
  property real now: Date.now() / 1000
  property bool selected: false
  required property color foreground
  required property color mutedForeground
  required property color accent
  required property color urgent
  required property color success
  required property color working
  required property string fontFamily
  required property real fontSize11
  required property real fontSize12
  property real spacing: 8

  signal clicked()

  readonly property string status: stage && stage.status ? stage.status : "pending"
  readonly property bool committed: status === "committed"
  readonly property bool active: status === "in_progress"
  readonly property bool attention: status === "blocked" || status === "failed"
  // The same elapsed/duration rule as the Plan stage list.
  readonly property double elapsedSecs: active && typeof stage.started_unix === "number"
    ? Math.max(0, Math.floor(now - stage.started_unix))
    : stage && typeof stage.duration_secs === "number"
      ? Math.max(0, Math.floor(stage.duration_secs)) : -1
  readonly property color statusColor: committed ? success : active ? working
    : attention ? urgent : mutedForeground

  implicitHeight: Math.max(titleText.implicitHeight, iconText.implicitHeight) + 8
  height: implicitHeight

  Rectangle {
    anchors.fill: parent
    radius: 4
    color: row.accent
    opacity: rowArea.containsMouse || row.selected ? 0.08 : 0
  }

  Text {
    id: iconText

    x: 4
    width: row.fontSize12 + 4
    anchors.verticalCenter: parent.verticalCenter
    text: row.committed ? "✓" : row.active ? "●" : row.attention ? "!" : "·"
    color: row.statusColor
    font.family: row.fontFamily
    font.pixelSize: row.fontSize12
    font.bold: true
  }

  Text {
    id: titleText

    anchors.left: iconText.right
    anchors.leftMargin: row.spacing / 2
    anchors.right: detailText.left
    anchors.rightMargin: row.spacing
    anchors.verticalCenter: parent.verticalCenter
    text: row.number + ". " + (row.stage && row.stage.title ? row.stage.title : "")
    textFormat: Text.PlainText
    maximumLineCount: 1
    elide: Text.ElideRight
    color: row.committed || row.active || row.attention ? row.foreground : row.mutedForeground
    font.family: row.fontFamily
    font.pixelSize: row.fontSize11
  }

  Text {
    id: detailText

    anchors.right: durationText.left
    anchors.rightMargin: row.spacing
    anchors.verticalCenter: parent.verticalCenter
    width: Math.min(implicitWidth, row.width * 0.3)
    text: row.committed && row.stage.sha ? row.stage.sha
      : row.activityText !== "" ? row.activityText : row.status.replace(/_/g, " ")
    textFormat: Text.PlainText
    maximumLineCount: 1
    elide: Text.ElideRight
    color: row.committed || row.active ? row.working : row.attention ? row.urgent : row.mutedForeground
    font.family: row.fontFamily
    font.pixelSize: row.fontSize11
  }

  Text {
    id: durationText

    anchors.right: chevron.left
    anchors.rightMargin: row.spacing
    anchors.verticalCenter: parent.verticalCenter
    width: implicitWidth
    text: row.elapsedSecs >= 0 ? Math.floor(row.elapsedSecs / 60) + "m "
      + (row.elapsedSecs % 60) + "s" : ""
    textFormat: Text.PlainText
    color: row.mutedForeground
    font.family: row.fontFamily
    font.pixelSize: row.fontSize11
  }

  Text {
    id: chevron

    anchors.right: parent.right
    anchors.rightMargin: 4
    anchors.verticalCenter: parent.verticalCenter
    text: "›"
    color: row.mutedForeground
    font.family: row.fontFamily
    font.pixelSize: row.fontSize12
  }

  MouseArea {
    id: rowArea

    anchors.fill: parent
    hoverEnabled: true
    cursorShape: Qt.PointingHandCursor
    onClicked: row.clicked()
  }
}
