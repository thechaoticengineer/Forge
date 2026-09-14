pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import Quickshell
import qs.Commons
import "PanelDetails.js" as PanelDetails
import "ModelRouting.js" as ModelRouting
import "ReportFormat.js" as ReportFormat
import "UsageFormat.js" as UsageFormat

// Interface: indexed immutable report projections and selection state enter as
// properties; selection/expansion and focus handoff leave as signals. listView
// is exposed for top-level keyboard routing and scroll restoration.
ListView {
  id: reportList

  required property bool reportsVisible
  required property var reportIndex
  required property int projectViewRevision
  required property string selectedReportKey
  required property string expandedReportKey
  required property real now
  required property string detailScope
  required property color foreground
  required property color mutedForeground
  required property color background
  required property color surface
  required property color accent
  required property color urgent
  required property string fontFamily
  required property real fontSize10
  required property real fontSize11
  property alias listView: reportList

  signal reportSelected(string key)
  signal reportExpansionRequested(string key)
  signal leaveRequested
  signal detailRevealed(var control)
  signal detailInspected(var control)

  visible: reportsVisible
  clip: true
  spacing: Style.space(6)
  boundsBehavior: Flickable.StopAtBounds
  model: reportEntries
  cacheBuffer: contentHeight
  property real readingY: 0
  property string readingKey: ""
  property real readingOffset: 0

  function captureReading() {
    readingY = contentY
    const index = indexAt(1, contentY + 1)
    const row = itemAtIndex(index)
    if (row) {
      readingKey = row.key
      readingOffset = contentY - row.y
    }
  }

  function sync() {
    PanelDetails.reconcile(reportEntries, reportList.reportIndex.rows)
    Qt.callLater(restoreReadingPosition)
  }

  function restoreReadingPosition() {
    if (moving)
      return
    let target = readingY
    for (let i = 0; i < count; ++i) {
      const row = itemAtIndex(i)
      if (row && row.key === readingKey) {
        target = row.y + readingOffset
        break
      }
    }
    contentY = Math.max(originY, Math.min(target, originY + Math.max(0, contentHeight - height)))
  }

  ListModel {
    id: reportEntries
  }

  onReportIndexChanged: sync()
  onProjectViewRevisionChanged: {
    reportEntries.clear()
    readingKey = ""
    sync()
  }
  Component.onCompleted: sync()
  onContentYChanged: {
    if (moving)
      captureReading()
  }
  onMovementEnded: captureReading()
  onModelChanged: Qt.callLater(restoreReadingPosition)
  onContentHeightChanged: Qt.callLater(restoreReadingPosition)
  onWidthChanged: Qt.callLater(restoreReadingPosition)
  onHeightChanged: Qt.callLater(restoreReadingPosition)
  onVisibleChanged: {
    if (visible)
      Qt.callLater(restoreReadingPosition)
  }

  delegate: Rectangle {
    id: reportRow

    required property var model
    required property int index
    readonly property var modelData: JSON.parse(model.text)
    readonly property string key: model.key
    readonly property bool expanded: reportList.expandedReportKey === key
    property bool detailsLoaded: false
    readonly property int commitCount: modelData.commits && typeof modelData.commits.length === "number" ? modelData.commits.length : 0
    width: reportList.width
    height: reportContent.implicitHeight + Style.space(8)
    radius: 3
    color: key === reportList.selectedReportKey ? Qt.darker(reportList.accent, 2.8) : "transparent"

    Column {
      id: reportContent

      x: Style.space(4)
      y: Style.space(4)
      width: parent.width - Style.space(8)
      spacing: Style.space(4)

      Button {
        objectName: "reportToggle"
        width: Math.min(implicitWidth, parent.width)
        text: reportRow.expanded ? "▾ Collapse report" : "▸ Expand report"
        onClicked: {
          reportList.reportSelected(reportRow.key)
          reportList.captureReading()
          reportList.reportExpansionRequested(reportRow.expanded ? "" : reportRow.key)
        }
      }

      ViewDetail {
        objectName: "reportGoal"
        width: parent.width
        metadata: "Goal"
        originalText: reportRow.modelData.goal || ""
      }

      Text {
        width: parent.width
        text: ReportFormat.reportTime(reportRow.modelData, reportList.now) + " · " + ReportFormat.reportDuration(reportRow.modelData.duration_secs) + " · " + reportRow.commitCount + (reportRow.commitCount === 1 ? " commit" : " commits")
        textFormat: Text.PlainText
        color: reportList.mutedForeground
        wrapMode: Text.Wrap
        font.family: reportList.fontFamily
        font.pixelSize: reportList.fontSize10
      }

      Text {
        visible: text !== ""
        width: parent.width
        text: UsageFormat.usageSummary(reportRow.modelData.usage)
        textFormat: Text.PlainText
        color: reportList.mutedForeground
        wrapMode: Text.Wrap
        font.family: reportList.fontFamily
        font.pixelSize: reportList.fontSize10
      }

      Loader {
        id: reportDetailsLoader

        visible: reportRow.expanded
        width: parent.width
        active: reportRow.expanded || reportRow.detailsLoaded
        onLoaded: reportRow.detailsLoaded = true
        sourceComponent: ViewFields {
          objectName: "reportDetails"
          width: reportDetailsLoader.width
          entries: PanelDetails.report(reportRow.modelData, {
            stageModelStatus: ModelRouting.stageModelStatus,
            stageModelErrors: ModelRouting.stageModelErrors,
            stageModelDetails: ModelRouting.stageModelDetails,
            architectUsageText: ReportFormat.architectUsageText,
            usageBreakdown: UsageFormat.usageBreakdown
          })
          onInspecting: reportList.captureReading()
        }
      }
    }
  }

  component ViewFields: DetailFields {
    id: fields

    scope: reportList.detailScope
    foreground: reportList.mutedForeground
    mutedForeground: reportList.mutedForeground
    background: reportList.background
    urgent: reportList.urgent
    fontFamily: reportList.fontFamily
    fontSize: reportList.fontSize11
    onCopyRequested: original => Quickshell.clipboardText = original
    onLeaveRequested: reportList.leaveRequested()
    onFocusRevealed: control => reportList.detailRevealed(control)
    onInspecting: reportList.detailInspected(fields)
  }

  component ViewDetail: ViewFields {
    property string originalText: ""
    property string metadata: ""
    property bool error: false
    entries: [PanelDetails.field("text", metadata, originalText, error)]
  }

  // id: keyboardHint — stable end marker for report subtree regression tests.
}
