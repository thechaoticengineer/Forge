pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import qs.Commons
import "PanelDetails.js" as PanelDetails
import "ReportFormat.js" as ReportFormat

// Interface: architecture and plan-review projections enter as properties;
// review loading/expansion and focus handoff leave as signals.
Column {
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

  signal planReviewExpansionRequested(bool expanded)
  signal planReviewLoadRequested
  signal leaveRequested
  signal detailRevealed(var control)
  signal detailInspected(var control)

  width: parent ? parent.width : 0
  spacing: Style.space(6)

  Column {
    visible: view.plan !== null || (view.engineState && view.engineState.architect_activity) || (view.architecture && view.architecture.context_status === "error")
    width: parent.width
    spacing: Style.space(4)

    ArchitectureDetails {
      id: architectureCard

      objectName: "architectureDetails"
      width: parent.width
      scope: view.detailScope
      foreground: view.foreground
      mutedForeground: view.mutedForeground
      background: view.surface
      accent: view.accent
      urgent: view.urgent
      fontFamily: view.fontFamily
      fontSize: view.fontSize12
      onCopyRequested: original => Quickshell.clipboardText = original
      onLeaveRequested: view.leaveRequested()
      onFocusRevealed: control => view.detailRevealed(control)
      onInspecting: view.detailInspected(architectureCard)
      entries: PanelDetails.architecture(view.architecture, view.engineState ? view.engineState.architect_activity : null, view.engineState ? view.engineState.persistence_error : "")
    }

    Text {
      width: parent.width
      text: ReportFormat.architectUsageText(view.plan)
      visible: text !== ""
      textFormat: Text.PlainText
      color: view.mutedForeground
      font.family: view.fontFamily
      font.pixelSize: view.fontSize12
      wrapMode: Text.Wrap
    }
  }

  Column {
    id: planReviewSection

    objectName: "planReviewSection"
    visible: view.planReview !== null
    width: parent.width
    spacing: Style.space(6)
    readonly property var review: {
      const version = view.planReviewVersion
      return Object.assign({}, view.planReviewView || {})
    }

    DetailFields {
      id: reviewFields

      width: parent.width
      scope: view.planReviewScope
      entries: [PanelDetails.status("plan-review", view.planReviewStatusText(view.planReview))].concat(planReviewSection.review.complete ? [] : [PanelDetails.status("plan-review-preview", "Shortened preview · full change requests have not been loaded.")])
      foreground: view.mutedForeground
      mutedForeground: view.mutedForeground
      background: view.background
      urgent: view.urgent
      fontFamily: view.fontFamily
      fontSize: view.fontSize11
      onCopyRequested: original => Quickshell.clipboardText = original
      onLeaveRequested: view.leaveRequested()
      onFocusRevealed: control => view.detailRevealed(control)
      onInspecting: view.detailInspected(reviewFields)
    }

    CompactDetail {
      id: planReviewRequests

      objectName: "planReviewRequests"
      width: parent.width
      metadata: "Plan review change requests"
      originalText: planReviewSection.review.text || ""
      textComplete: planReviewSection.review.complete === true
      loading: planReviewSection.review.pending === true
      detailError: planReviewSection.review.error || "Complete requests have not been loaded."
      expanded: view.planReviewExpanded
      foreground: view.foreground
      mutedForeground: view.mutedForeground
      background: view.surface
      fontFamily: view.fontFamily
      fontSize: view.fontSize12
      onExpansionRequested: expanded => {
        view.planReviewExpansionRequested(expanded)
        if (expanded && !textComplete)
          view.planReviewLoadRequested()
      }
      onLoadRequested: view.planReviewLoadRequested()
      onCopyRequested: original => Quickshell.clipboardText = original
      onLeaveRequested: view.leaveRequested()
      onFocusRevealed: control => view.detailRevealed(control)
      onInspecting: view.detailInspected(planReviewRequests)
    }
  }
}
