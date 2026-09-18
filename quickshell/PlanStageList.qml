pragma ComponentBehavior: Bound

import QtQuick
import "PlanReview.js" as PlanReview

// Interface: the compact stage list of the Plan tab. An optional summary line, one
// OverviewStageRow per stage (status icon, "N. title", commit hash or status,
// duration, ›) and one plan review strip. A click on a row leaves as
// stageOpened(index), a click on the strip as planReviewRequested(). Stages, the
// selection, the clock and the plan review enter as properties. It shows no
// routing, model or review policy. Imports no shell modules, so qmltestrunner
// loads it on its own.
Column {
  id: list

  required property var stages
  required property int selectedIndex
  required property real now
  required property var planReview
  // The current step of the stage being worked on; replaces its status word.
  property string activityText: ""
  property string summaryText: ""
  // Palette and font sizes; Panel.qml passes its shared theme object.
  property var theme: null
  property color foreground: theme ? theme.foreground : "#dddddd"
  property color mutedForeground: theme ? theme.mutedForeground : "#aaaaaa"
  property color background: theme ? theme.background : "#202020"
  property color surface: theme ? theme.surface : "#282828"
  property color accent: theme ? theme.accent : "#6699ff"
  property color urgent: theme ? theme.urgent : "#ff6666"
  property color success: theme ? theme.success : "#4faf72"
  property color working: theme ? theme.working : "#d5a542"
  property string fontFamily: theme ? theme.fontFamily : "monospace"
  property real fontSize10: theme ? theme.fontSize10 : 10
  property real fontSize11: theme ? theme.fontSize11 : 11
  property real fontSize12: theme ? theme.fontSize12 : 12
  property real gap: 8

  readonly property string reviewStripText: PlanReview.planReviewStripText(planReview)

  signal stageOpened(int index)
  signal planReviewRequested

  // The row item of a stage, so the panel can reveal the selected one.
  function rowItem(index) {
    return rows.itemAt(index)
  }

  spacing: 2

  Text {
    visible: list.summaryText !== ""
    width: parent.width
    height: visible ? implicitHeight + list.gap : 0
    text: list.summaryText
    textFormat: Text.PlainText
    maximumLineCount: 1
    elide: Text.ElideRight
    color: list.mutedForeground
    font.family: list.fontFamily
    font.pixelSize: list.fontSize11
  }

  Repeater {
    id: rows

    model: list.stages.length
    delegate: OverviewStageRow {
      id: stageRow

      required property int index
      objectName: "planStageRow"
      width: list.width
      stage: list.stages[index] || ({})
      number: stage.id !== undefined ? stage.id : index + 1
      activityText: stage.status === "in_progress" ? list.activityText : ""
      now: list.now
      selected: index === list.selectedIndex
      foreground: list.foreground
      mutedForeground: list.mutedForeground
      accent: list.accent
      urgent: list.urgent
      success: list.success
      working: list.working
      fontFamily: list.fontFamily
      fontSize11: list.fontSize11
      fontSize12: list.fontSize12
      spacing: list.gap
      onClicked: list.stageOpened(index)
    }
  }

  Item {
    id: strip

    objectName: "planReviewStrip"
    visible: list.planReview !== null && list.reviewStripText !== ""
    width: parent.width
    height: visible ? Math.max(stripText.implicitHeight, detailsText.implicitHeight) + 14 : 0

    Rectangle {
      anchors.fill: parent
      radius: 4
      color: list.surface
      opacity: stripArea.containsMouse ? 1 : 0.7
    }

    Text {
      id: stripText

      anchors.left: parent.left
      anchors.leftMargin: list.gap
      anchors.right: detailsText.left
      anchors.rightMargin: list.gap
      anchors.verticalCenter: parent.verticalCenter
      text: list.reviewStripText
      textFormat: Text.PlainText
      maximumLineCount: 1
      elide: Text.ElideRight
      color: list.mutedForeground
      font.family: list.fontFamily
      font.pixelSize: list.fontSize10
    }

    Text {
      id: detailsText

      anchors.right: parent.right
      anchors.rightMargin: list.gap
      anchors.verticalCenter: parent.verticalCenter
      text: "details ›"
      textFormat: Text.PlainText
      color: list.accent
      font.family: list.fontFamily
      font.pixelSize: list.fontSize10
    }

    MouseArea {
      id: stripArea

      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onClicked: list.planReviewRequested()
    }
  }
}
