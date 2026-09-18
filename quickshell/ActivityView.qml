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
  property alias output: agentOutput

  signal liveTabRequested(bool live)
  signal historyFilterRequested(string filter)
  signal reportSelected(string key)
  signal reportExpansionRequested(string key)
  signal leaveRequested
  signal detailRevealed(var control)
  signal detailInspected(var control)

  // Ctrl+d / Ctrl+u: half a page in the shown list; the live and history lists
  // follow their tail again when scrolled to the bottom.
  function scrollOutput(direction) {
    // Indexed from a list, so the linter does not narrow the three list types to Flickable.
    const view = [agentOutput.liveOutput, agentOutput.reportList, agentOutput.historyList][
      activity.liveTab ? 0 : activity.reportsVisible ? 1 : 2]
    view.cancelFlick()
    if (view !== agentOutput.reportList) view.followTail = false
    const top = view.originY
    const bottom = top + Math.max(0, view.contentHeight - view.height)
    view.contentY = Math.max(top, Math.min(bottom,
      view.contentY + direction * view.height / 2))
    if (view !== agentOutput.reportList && direction > 0 && view.contentY >= bottom) {
      view.followTail = true
      view.scrollToTail()
    }
    if (view !== agentOutput.reportList) view.captureReading()
    if (view === agentOutput.reportList) agentOutput.reportList.captureReading()
  }

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
