pragma ComponentBehavior: Bound

import QtQuick

// Interface: the Activity tab. Root-owned log/history models and report
// projections enter as properties and pass straight to AgentOutputView, which
// fills the whole view; tab/filter/selection actions leave as its signals.
// `output` exposes the AgentOutputView so Panel.qml keeps routing the keyboard
// and reading positions to its Live, History and Reports lists.
Item {
  id: activity

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
  property alias output: agentOutput

  signal liveTabRequested(bool live)
  signal historyFilterRequested(string filter)
  signal reportSelected(string key)
  signal reportExpansionRequested(string key)
  signal leaveRequested
  signal detailRevealed(var control)
  signal detailInspected(var control)

  AgentOutputView {
    id: agentOutput

    anchors.fill: parent
    // The lists take the whole view height, not a share of a longer page.
    panelHeight: activity.height
    liveTab: activity.liveTab
    historyFilter: activity.historyFilter
    hasReports: activity.hasReports
    reportsVisible: activity.reportsVisible
    logError: activity.logError
    liveModel: activity.liveModel
    historyModel: activity.historyModel
    reportIndex: activity.reportIndex
    projectViewRevision: activity.projectViewRevision
    selectedReportKey: activity.selectedReportKey
    expandedReportKey: activity.expandedReportKey
    now: activity.now
    detailScope: activity.detailScope
    foreground: activity.foreground
    mutedForeground: activity.mutedForeground
    background: activity.background
    surface: activity.surface
    accent: activity.accent
    urgent: activity.urgent
    success: activity.success
    working: activity.working
    fontFamily: activity.fontFamily
    fontSize10: activity.fontSize10
    fontSize11: activity.fontSize11
    onLiveTabRequested: live => activity.liveTabRequested(live)
    onHistoryFilterRequested: filter => activity.historyFilterRequested(filter)
    onReportSelected: key => activity.reportSelected(key)
    onReportExpansionRequested: key => activity.reportExpansionRequested(key)
    onLeaveRequested: activity.leaveRequested()
    onDetailRevealed: control => activity.detailRevealed(control)
    onDetailInspected: control => activity.detailInspected(control)
  }
}
