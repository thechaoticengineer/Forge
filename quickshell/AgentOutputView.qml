pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import qs.Commons

// Interface: root-owned log/history models and report projections enter as
// properties; tab/filter/selection actions leave as signals. The three list
// aliases preserve top-level keyboard routing and reading-position control.
Column {
  id: view

  required property bool liveTab
  required property string historyFilter
  required property bool hasReports
  required property bool reportsVisible
  required property string logError
  required property var liveModel
  required property var historyModel
  required property var reportIndex
  required property int projectViewRevision
  required property string selectedReportKey
  required property string expandedReportKey
  required property real now
  required property real panelHeight
  required property string detailScope
  required property color foreground
  required property color mutedForeground
  required property color background
  required property color surface
  required property color accent
  required property color urgent
  required property color success
  required property color working
  required property string fontFamily
  required property real fontSize10
  required property real fontSize11
  property alias liveOutput: liveOutput
  property alias historyList: historyList
  property alias reportList: reports.listView
  property alias outputFrame: outputFrame

  signal liveTabRequested(bool live)
  signal historyFilterRequested(string filter)
  signal reportSelected(string key)
  signal reportExpansionRequested(string key)
  signal leaveRequested
  signal detailRevealed(var control)
  signal detailInspected(var control)

  width: parent ? parent.width : 0
  spacing: Style.space(8)

  Row {
    spacing: Style.space(8)

    ViewButton {
      label: "Live"
      primary: view.liveTab
      onClicked: view.liveTabRequested(true)
    }

    ViewButton {
      label: "History"
      primary: !view.liveTab
      onClicked: view.liveTabRequested(false)
    }
  }

  DetailFields {
    id: logErrorText

    visible: view.liveTab && view.logError !== ""
    width: parent.width
    scope: view.detailScope
    entries: [{key: "text", label: "Log error", text: view.logError, error: true}]
    foreground: view.mutedForeground
    mutedForeground: view.mutedForeground
    background: view.background
    urgent: view.urgent
    fontFamily: view.fontFamily
    fontSize: view.fontSize11
    onCopyRequested: original => Quickshell.clipboardText = original
    onLeaveRequested: view.leaveRequested()
    onFocusRevealed: control => view.detailRevealed(control)
    onInspecting: view.detailInspected(logErrorText)
  }

  Rectangle {
    id: outputFrame

    width: parent.width
    height: Math.max(Style.space(180), view.panelHeight * 0.3)
    color: view.surface
    radius: 4

    DetailList {
      id: liveOutput

      visible: view.liveTab
      anchors.fill: parent
      anchors.margins: Style.space(8)
      model: view.liveModel
      foreground: view.mutedForeground
      mutedForeground: view.mutedForeground
      background: view.surface
      urgent: view.urgent
      accent: view.accent
      success: view.success
      working: view.working
      fontFamily: view.fontFamily
      fontSize: view.fontSize10
      onCopyRequested: original => Quickshell.clipboardText = original
      onLeaveRequested: view.leaveRequested()
    }

    Row {
      id: historyFilters

      visible: !view.liveTab
      anchors.top: parent.top
      anchors.left: parent.left
      anchors.margins: Style.space(8)
      spacing: Style.space(4)

      Repeater {
        model: view.hasReports ? ["all", "runs", "git", "reviews", "errors", "reports"] : ["all", "runs", "git", "reviews", "errors"]

        delegate: ViewButton {
          required property string modelData
          label: modelData
          primary: view.historyFilter === modelData
          onClicked: view.historyFilterRequested(modelData)
        }
      }
    }

    DetailList {
      id: historyList

      visible: !view.liveTab && !view.reportsVisible
      anchors.top: historyFilters.bottom
      anchors.bottom: parent.bottom
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.margins: Style.space(8)
      model: view.historyModel
      history: true
      now: view.now
      foreground: view.mutedForeground
      mutedForeground: view.mutedForeground
      background: view.surface
      urgent: view.urgent
      accent: view.accent
      success: view.success
      working: view.working
      fontFamily: view.fontFamily
      fontSize: view.fontSize10
      onCopyRequested: original => Quickshell.clipboardText = original
      onLeaveRequested: view.leaveRequested()
    }

    ReportsView {
      id: reports

      anchors.top: historyFilters.bottom
      anchors.bottom: parent.bottom
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.margins: Style.space(8)
      reportsVisible: view.reportsVisible
      reportIndex: view.reportIndex
      projectViewRevision: view.projectViewRevision
      selectedReportKey: view.selectedReportKey
      expandedReportKey: view.expandedReportKey
      now: view.now
      detailScope: view.detailScope
      foreground: view.foreground
      mutedForeground: view.mutedForeground
      background: view.background
      surface: view.surface
      accent: view.accent
      urgent: view.urgent
      fontFamily: view.fontFamily
      fontSize10: view.fontSize10
      fontSize11: view.fontSize11
      onReportSelected: key => view.reportSelected(key)
      onReportExpansionRequested: key => view.reportExpansionRequested(key)
      onLeaveRequested: view.leaveRequested()
      onDetailRevealed: control => view.detailRevealed(control)
      onDetailInspected: control => view.detailInspected(control)
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
