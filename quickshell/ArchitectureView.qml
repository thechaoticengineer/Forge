pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls

// Interface: the Architecture tab. The architecture card (context, guidance,
// risks, decisions), the role token totals (tokens used per role) and the plan review status, fix
// commits and history enter as the same projections ArchitectureReviewView
// takes; review loading/expansion and focus handoff leave as its signals.
// The view scrolls on its own when its content is taller than the window.
Flickable {
  id: view

  required property var engineState
  required property var plan
  required property var architecture
  required property var planReview
  required property var planReviewView
  required property int planReviewVersion
  required property string planReviewScope
  required property bool planReviewExpanded
  required property string detailScope
  required property var planReviewStatusText
  required property color foreground
  required property color mutedForeground
  required property color background
  required property color surface
  required property color accent
  required property color urgent
  required property string fontFamily
  required property real fontSize11
  required property real fontSize12
  property real scrollBarSpace: 16
  readonly property bool empty: architectureReview.implicitHeight <= 0

  signal planReviewExpansionRequested(bool expanded)
  signal planReviewLoadRequested
  signal leaveRequested
  signal detailRevealed(var control)
  signal detailInspected(var control)

  clip: true
  contentWidth: width
  contentHeight: view.empty ? emptyText.implicitHeight : architectureReview.implicitHeight
  flickableDirection: Flickable.VerticalFlick
  boundsBehavior: Flickable.StopAtBounds
  ScrollBar.vertical: ScrollBar {
    policy: ScrollBar.AsNeeded
  }

  Text {
    id: emptyText

    visible: view.empty
    width: view.width
    text: "No architecture context yet. It appears once a plan exists or the architect runs."
    textFormat: Text.PlainText
    wrapMode: Text.Wrap
    color: view.mutedForeground
    font.family: view.fontFamily
    font.pixelSize: view.fontSize11
  }

  ArchitectureReviewView {
    id: architectureReview

    width: view.width - view.scrollBarSpace
    engineState: view.engineState
    plan: view.plan
    architecture: view.architecture
    planReview: view.planReview
    planReviewView: view.planReviewView
    planReviewVersion: view.planReviewVersion
    planReviewScope: view.planReviewScope
    planReviewExpanded: view.planReviewExpanded
    detailScope: view.detailScope
    planReviewStatusText: view.planReviewStatusText
    foreground: view.foreground
    mutedForeground: view.mutedForeground
    background: view.background
    surface: view.surface
    accent: view.accent
    urgent: view.urgent
    fontFamily: view.fontFamily
    fontSize11: view.fontSize11
    fontSize12: view.fontSize12
    onPlanReviewExpansionRequested: expanded => view.planReviewExpansionRequested(expanded)
    onPlanReviewLoadRequested: view.planReviewLoadRequested()
    onLeaveRequested: view.leaveRequested()
    onDetailRevealed: control => view.detailRevealed(control)
    onDetailInspected: control => view.detailInspected(control)
  }
}
