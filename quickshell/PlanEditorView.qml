pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Controls as QQC
import Quickshell
import qs.Commons
import "ModelRouting.js" as ModelRouting
import "PanelDetails.js" as PanelDetails
import "PlanEdit.js" as PlanEdit
import "UsageFormat.js" as UsageFormat

// Interface: plan/edit projections and review callbacks enter as properties;
// edit, expansion, review-loading, and focus actions leave as signals. The list,
// frame, and save control aliases preserve root keyboard routing and focus.
Column {
  id: view

  required property bool editingPlan
  required property bool editPending
  required property bool editValid
  required property bool queueActive
  required property bool engineOnline
  required property bool busy
  required property var displayedStages
  required property var editStages
  required property var plan
  required property var stageSnapshot
  required property int expandedStageId
  required property int selectedStageIndex
  required property bool stageRoutingExpanded
  required property var stageReviewBlocks
  required property real agentNow
  required property real panelHeight
  required property var editFocusedField
  required property var fs
  required property var stageActivity
  required property var stageDetailScope
  required property var reviewView
  required property var reviewGateText
  required property var reviewDecision
  required property var reviewFields
  required property var reviewRoundLabel
  required property var reviewTimestamp
  required property var stageReviewIncompleteRange
  required property var stageReviewHasHeldPreview
  required property var ensureStageReviewsLoaded
  required property var reconcileStageReviewPresentation
  required property color foreground
  required property color mutedForeground
  required property color background
  required property color surface
  required property color accent
  required property color urgent
  required property color success
  required property color working
  required property string fontFamily
  property alias stageList: stageList
  property alias stageFrame: stageFrame
  property alias saveButton: savePlanButton

  signal savePlanEdit
  signal cancelPlanEdit
  signal addEditStage
  signal moveEditStage(int index, int direction)
  signal deleteEditStage(int index)
  signal changeStageField(int index, string field, var value)
  signal changeModelConstraint(int index, string key, string value)
  signal loadStageReviews(int stageId, var cursor, var end)
  signal retryStageReviews(var stage)
  signal expandedStageRequested(int stageId)
  signal stageRoutingExpandedRequested(bool expanded)
  signal editFocusChanged(var field)
  signal helpRequested
  signal leaveRequested
  signal detailRevealed(var control)
  signal detailInspected(var control)

        // ------------------------------------------------- stages
        Flow {
          visible: view.editingPlan
          width: parent.width
          spacing: Style.space(8)
          ViewButton {
            label: "Add stage"
            enabled: !view.editPending
            onClicked: view.addEditStage()
          }
          ViewButton {
            id: savePlanButton
            label: view.editPending ? "Saving…" : "Save"
            primary: true
            enabled: view.editingPlan && view.editValid && !view.editPending
              && view.engineOnline && !view.busy && !view.queueActive
            onClicked: view.savePlanEdit()
          }
          ViewButton {
            label: "Cancel"
            enabled: !view.editPending
            onClicked: view.cancelPlanEdit()
          }
          Text {
            text: "Editing plan · title and instructions required"
            color: view.mutedForeground
            font.family: view.fontFamily
            font.pixelSize: view.fs(11)
          }
        }
        Rectangle {
          id: stageFrame
          width: parent.width
          height: view.editingPlan
            ? Math.max(Style.space(260), view.panelHeight * 0.45)
            : stageList.contentHeight + Style.space(16)
          color: view.surface
          radius: 4
          ListView {
            id: stageList
            anchors.fill: parent
            anchors.margins: Style.space(8)
            anchors.rightMargin: Style.space(view.editingPlan ? 22 : 8)
            clip: true
            interactive: view.editingPlan
            boundsBehavior: Flickable.StopAtBounds
            ScrollBar.vertical: ScrollBar {
              policy: view.editingPlan ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
            }
            spacing: Style.space(6)
            // An integer model keeps delegates alive when polling replaces the
            // plan/stages array. Live roles update without replacing open editors.
            model: view.displayedStages.length
            delegate: Rectangle {
              id: stageRow
              readonly property var modelData: view.displayedStages[index] || ({})
              required property int index
              readonly property bool expanded: view.expandedStageId === modelData.id
              readonly property bool editable: view.editingPlan && modelData.status !== "committed"
              function focusEditor() {
                if (stageEditor.item && !view.editPending) stageEditor.item.focusTitle()
              }
              readonly property double elapsedSecs: modelData.status === "in_progress"
                && typeof modelData.started_unix === "number"
                ? Math.max(0, Math.floor(view.agentNow - modelData.started_unix))
                : typeof modelData.duration_secs === "number"
                  ? Math.max(0, Math.floor(modelData.duration_secs)) : -1
              readonly property var reviewView: view.reviewView(modelData)
              readonly property var reviewHistory: reviewView.rows
              readonly property string detailScope: view.stageDetailScope(modelData)
              readonly property var prose: view.stageSnapshot && view.stageSnapshot.key === detailScope
                ? view.stageSnapshot : null
              readonly property string reviewMessage: reviewView.error || view.stageReviewBlocks[detailScope] || ""
              readonly property bool reviewRetryAvailable: reviewMessage !== "" && (!!reviewView.retry
                || view.stageReviewIncompleteRange(reviewView) !== null || reviewView.older > 0)
              readonly property var lastReview: reviewHistory.length > 0
                ? reviewHistory[reviewHistory.length - 1] : null
              readonly property var lastDecision: lastReview
                ? view.reviewDecision(lastReview.verdict) : null
              readonly property string activity: view.stageActivity(modelData)
              width: stageList.width
              height: editable ? stageEditor.height : stageContent.implicitHeight
              radius: 3
              color: index === view.selectedStageIndex
                ? Qt.darker(view.accent, 2.8) : "transparent"
              Column {
                id: stageContent
                visible: !stageRow.editable
                width: stageRow.width
                spacing: 2
                QQC.Button {
                  id: stageToggle
                  objectName: "stageToggle"
                  width: stageRow.width
                  implicitHeight: Math.max(32, headerLabel.implicitHeight + 12)
                  padding: 6
                  focusPolicy: Qt.StrongFocus
                  Accessible.name: (stageRow.expanded ? "Collapse stage " : "Expand stage ") + stageRow.modelData.id
                  contentItem: Row {
                    spacing: Style.space(4)
                    Text {
                      text: stageRow.expanded ? "▾" : "▸"
                      textFormat: Text.PlainText
                      wrapMode: Text.Wrap
                      color: view.foreground
                      font.family: view.fontFamily
                      font.pixelSize: view.fs(12)
                    }
                    Text {
                      id: stageGlyph
                      text: stageRow.modelData.status === "committed" ? "✓"
                        : stageRow.modelData.status === "in_progress" ? "●"
                        : stageRow.modelData.status === "blocked" ? "!" : "·"
                      textFormat: Text.PlainText
                      wrapMode: Text.Wrap
                      color: stageRow.modelData.status === "committed" ? view.success
                        : stageRow.modelData.status === "in_progress" ? view.working
                        : stageRow.modelData.status === "blocked" ? view.urgent : view.mutedForeground
                      font.family: view.fontFamily
                      font.pixelSize: view.fs(12)
                      font.bold: true
                    }
                    Text {
                      id: headerLabel
                      width: stageToggle.availableWidth - stageGlyph.width - stageGlyph.x - Style.space(4)
                      text: stageRow.modelData.id + ". " + stageRow.modelData.title
                      textFormat: Text.PlainText
                      color: view.foreground
                      wrapMode: Text.Wrap
                      font.family: view.fontFamily
                      font.pixelSize: view.fs(12)
                      font.bold: true
                    }
                  }
                  background: Rectangle {
                    radius: 4
                    color: stageToggle.hovered || stageToggle.down ? Qt.alpha(view.foreground, 0.06) : "transparent"
                    border.width: stageToggle.visualFocus ? 1 : 0
                    border.color: view.accent
                  }
                  onClicked: view.expandedStageRequested(stageRow.expanded ? -1 : stageRow.modelData.id)
                  onActiveFocusChanged: if (activeFocus && focusReason !== Qt.MouseFocusReason
                    && focusReason !== Qt.PopupFocusReason) view.detailRevealed(stageToggle)
                  Keys.priority: Keys.AfterItem
                  Keys.onPressed: event => {
                    if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) stageToggle.clicked()
                    else if (event.key === Qt.Key_Escape) view.leaveRequested()
                    if (event.key !== Qt.Key_Tab && event.key !== Qt.Key_Backtab) event.accepted = true
                  }
                }
                Flow {
                  width: stageRow.width
                  spacing: Style.space(8)
                  Text {
                    width: Math.min(implicitWidth, stageRow.width)
                    textFormat: Text.PlainText
                    text: (view.editingPlan && stageRow.modelData.status === "committed"
                      ? "committed — locked" : stageRow.modelData.status === "blocked"
                      ? (stageRow.modelData.review_gate && stageRow.modelData.review_gate.status === "scope_blocked"
                         ? "blocked · stage cannot be built as written"
                         : stageRow.modelData.review_gate && stageRow.modelData.review_gate.status === "design_blocked"
                         ? "blocked · pen.dev export unavailable"
                         : stageRow.modelData.review_gate && stageRow.modelData.review_gate.status === "exhausted"
                         ? "blocked · fix rounds exhausted" : "blocked") : stageRow.modelData.status)
                      + (stageRow.modelData.sha ? " " + stageRow.modelData.sha : "")
                    color: stageRow.modelData.status === "committed" ? view.success
                      : stageRow.modelData.status === "in_progress" ? view.working
                      : stageRow.modelData.status === "blocked" ? view.urgent
                      : view.mutedForeground
                    wrapMode: Text.Wrap
                    font.family: view.fontFamily
                    font.pixelSize: view.fs(11)
                  }
                  Text {
                    visible: text !== ""
                    width: Math.min(implicitWidth, stageRow.width)
                    text: stageRow.activity
                    textFormat: Text.PlainText
                    color: view.working
                    wrapMode: Text.Wrap
                    font.family: view.fontFamily
                    font.pixelSize: view.fs(11)
                    font.bold: true
                  }
                  Text {
                    visible: !!stageRow.modelData.review_gate
                    width: stageRow.width
                    text: view.reviewGateText(stageRow.modelData)
                    textFormat: Text.PlainText
                    color: view.mutedForeground
                    wrapMode: Text.Wrap
                    font.family: view.fontFamily
                    font.pixelSize: view.fs(11)
                  }
                  Text {
                    visible: stageRow.reviewHistory.length > 0
                    width: Math.min(implicitWidth, stageRow.width)
                    text: stageRow.lastReview && stageRow.lastDecision
                      ? "historical review · " + view.reviewRoundLabel(stageRow.lastReview)
                        + ": " + stageRow.lastDecision.label
                        + (stageRow.modelData.last_verdict_valid === false ? " · obsolete for current work" : "")
                      : ""
                    textFormat: Text.PlainText
                    color: stageRow.modelData.last_verdict_valid === false ? view.mutedForeground
                      : stageRow.lastDecision && stageRow.lastDecision.optionalNotes ? view.working
                      : stageRow.lastDecision && stageRow.lastDecision.clean
                      ? (stageRow.activity ? view.mutedForeground : view.success) : view.urgent
                    wrapMode: Text.Wrap
                    font.family: view.fontFamily
                    font.pixelSize: view.fs(11)
                  }
                  Text {
                    width: stageRow.width
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    visible: stageRow.elapsedSecs >= 0
                    text: visible ? Math.floor(stageRow.elapsedSecs / 60) + "m "
                      + (stageRow.elapsedSecs % 60) + "s" : ""
                    color: view.mutedForeground
                    font.family: view.fontFamily
                    font.pixelSize: view.fs(11)
                  }
                }
                Text {
                  width: stageRow.width
                  text: ModelRouting.stageModelStatus(stageRow.modelData)
                  textFormat: Text.PlainText
                  color: view.mutedForeground
                  wrapMode: Text.Wrap
                  font.family: view.fontFamily
                  font.pixelSize: view.fs(11)
                }
                Text {
                  visible: text !== ""
                  width: stageRow.width
                  text: ModelRouting.stageModelErrors(stageRow.modelData)
                  textFormat: Text.PlainText
                  color: view.urgent
                  wrapMode: Text.Wrap
                  font.family: view.fontFamily
                  font.pixelSize: view.fs(11)
                }
                Loader {
                  width: stageRow.width
                  active: stageRow.expanded && !stageRow.editable && stageRow.prose !== null
                  // An inactive Loader retains its height after destroying its item.
                  // Exclude it from the Column when the stage details are closed.
                  visible: active
                  sourceComponent: Column {
                    width: stageRow.width
                    spacing: 2
                    StageProseField {
                      width: stageRow.width
                      label: "Commit"
                      originalText: stageRow.prose ? stageRow.prose.commit : ""
                    }
                    Column {
                      width: stageRow.width
                      spacing: 2
                      Repeater {
                        model: stageRow.prose ? stageRow.prose.rationale : []
                        delegate: StageProseField {
                          required property var modelData
                          width: stageRow.width
                          label: modelData.label
                          originalText: modelData.text
                        }
                      }
                      QQC.Button {
                        id: stageRoutingToggle
                        objectName: "stageRoutingToggle"
                        width: stageRow.width
                        implicitHeight: Math.max(32, routingLabel.implicitHeight + 12)
                        padding: 6
                        focusPolicy: Qt.StrongFocus
                        Accessible.name: (view.stageRoutingExpanded ? "Collapse " : "Expand ") + "model agreement and routing details"
                        contentItem: Text {
                          id: routingLabel
                          text: (view.stageRoutingExpanded ? "▾  " : "▸  ") + "Model agreement and routing details"
                          textFormat: Text.PlainText
                          wrapMode: Text.Wrap
                          color: view.foreground
                          font.family: view.fontFamily
                          font.pixelSize: view.fs(11)
                        }
                        background: Rectangle {
                          radius: 4
                          color: stageRoutingToggle.hovered || stageRoutingToggle.down ? Qt.alpha(view.foreground, 0.06) : "transparent"
                          border.width: stageRoutingToggle.visualFocus ? 1 : 0
                          border.color: view.accent
                        }
                        onClicked: view.stageRoutingExpandedRequested(!view.stageRoutingExpanded)
                        onActiveFocusChanged: if (activeFocus && focusReason !== Qt.MouseFocusReason
                          && focusReason !== Qt.PopupFocusReason) view.detailRevealed(stageRoutingToggle)
                        Keys.priority: Keys.AfterItem
                        Keys.onPressed: event => {
                          if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) stageRoutingToggle.clicked()
                          else if (event.key === Qt.Key_Escape) view.leaveRequested()
                          if (event.key !== Qt.Key_Tab && event.key !== Qt.Key_Backtab) event.accepted = true
                        }
                      }
                      Repeater {
                        model: view.stageRoutingExpanded && stageRow.prose ? stageRow.prose.diagnostics : []
                        delegate: StageProseField {
                          required property var modelData
                          width: stageRow.width
                          label: modelData.label
                          originalText: modelData.text
                        }
                      }
                    }
                    StageProseField {
                      width: stageRow.width
                      label: "Review policy rationale"
                      originalText: stageRow.prose ? stageRow.prose.policyRationale : ""
                    }
                    StageProseField {
                      width: stageRow.width
                      label: "Instructions"
                      originalText: stageRow.prose ? stageRow.prose.instructions : ""
                    }
                    StageProseField {
                      width: stageRow.width
                      label: "Acceptance criteria"
                      originalText: stageRow.prose ? stageRow.prose.acceptance : ""
                    }
                    Column {
                      id: stageReviews
                      objectName: "stageHistoricalReviews"
                      readonly property var currentRows: stageRow.reviewHistory
                      readonly property string currentScope: stageRow.reviewView.scope.key
                      readonly property bool planEditing: view.editingPlan
                      readonly property bool heldPreview: view.stageReviewHasHeldPreview(reviewPresentation, stageRow.reviewView)
                      // A single queued update coalesces publications and runs
                      // against current delegate state, outside binding evaluation.
                      onCurrentRowsChanged: reviewUpdate.restart()
                      onCurrentScopeChanged: reviewUpdate.restart()
                      onPlanEditingChanged: reviewUpdate.restart()
                      Timer {
                        id: reviewUpdate
                        interval: 0
                        running: true
                        onTriggered: {
                          view.ensureStageReviewsLoaded(stageRow.modelData)
                          view.reconcileStageReviewPresentation(stageRow.modelData, reviewPresentation, reviewRepeater)
                        }
                      }
                      ListModel { id: reviewPresentation; dynamicRoles: true }
                      width: stageRow.width
                      spacing: 2
                      Text {
                        visible: stageRow.expanded && stageRow.reviewHistory.length > 0
                        width: stageRow.width
                        text: "Historical reviews · showing " + stageRow.reviewHistory.length
                          + " of " + stageRow.reviewView.scope.count
                          + (stageRow.reviewHistory.some(function(r) { return !r.complete }) ? " · previews require full-text loading" : "")
                        textFormat: Text.PlainText
                        color: view.mutedForeground
                        wrapMode: Text.Wrap
                        font.family: view.fontFamily
                        font.pixelSize: view.fs(11)
                      }
                      Text {
                        objectName: "stageReviewStatus"
                        visible: text !== ""
                        width: stageRow.width
                        text: stageRow.reviewMessage || (stageRow.reviewView.pending ? "Loading complete reviews…"
                          : view.stageReviewIncompleteRange(stageRow.reviewView) ? "Complete reviews have not been loaded."
                          : stageReviews.heldPreview ? "Selected preview retained; complete reviews are shown separately." : "")
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: stageRow.reviewMessage ? view.urgent : view.mutedForeground
                        font.family: view.fontFamily
                        font.pixelSize: view.fs(11)
                      }
                      Flow {
                        width: stageRow.width
                        spacing: Style.space(6)
                        Button {
                          objectName: "stageReviewsOlder"
                          visible: stageRow.reviewView.older > 0
                          width: Math.min(implicitWidth, stageRow.width)
                          text: "Load older reviews (" + stageRow.reviewView.older + ")"
                          enabled: !stageRow.reviewView.pending
                          onClicked: view.loadStageReviews(stageRow.modelData.id,
                            Math.max(0, stageRow.reviewView.older - 8), stageRow.reviewView.older)
                        }
                        Button {
                          objectName: "stageReviewsRetry"
                          visible: stageRow.reviewRetryAvailable
                          width: Math.min(implicitWidth, stageRow.width)
                          text: "Retry reviews"
                          enabled: !stageRow.reviewView.pending
                          onClicked: view.retryStageReviews(stageRow.modelData)
                        }
                      }
                      Repeater {
                        id: reviewRepeater
                        model: reviewPresentation
                        delegate: Column {
                          id: reviewRound
                          required property var record
                          required property string sourceScope
                          readonly property var modelData: record
                          readonly property var verdict: modelData.verdict
                          readonly property var decision: view.reviewDecision(verdict)
                          visible: stageRow.expanded
                          width: stageRow.width
                          spacing: 2
                          Text {
                            width: stageRow.width
                            text: "Historical · " + (reviewRound.verdict.role || "reviewer") + " review " + view.reviewRoundLabel(reviewRound.modelData) + " — "
                              + reviewRound.decision.label
                              + (reviewRound.sourceScope !== stageReviews.currentScope
                                ? " · selected text held from an earlier publication" : "")
                            textFormat: Text.PlainText
                            color: reviewRound.decision.optionalNotes ? view.working
                              : reviewRound.decision.clean
                                ? (stageRow.activity ? view.mutedForeground : view.success) : view.urgent
                            wrapMode: Text.Wrap
                            font.family: view.fontFamily
                            font.pixelSize: view.fs(11)
                          }
                          Text {
                            visible: text !== ""
                            width: stageRow.width
                            text: view.reviewTimestamp(reviewRound.verdict)
                            textFormat: Text.PlainText
                            color: view.mutedForeground
                            wrapMode: Text.Wrap
                            font.family: view.fontFamily
                            font.pixelSize: view.fs(11)
                          }
                          StageProseField {
                            width: stageRow.width
                            label: reviewRound.modelData.complete ? "Summary" : "Summary preview · feedback may be omitted"
                            originalText: reviewRound.verdict.summary || ""
                            onHasSelectionChanged: if (!hasSelection) reviewUpdate.restart()
                          }
                          Repeater {
                            // Shortened feedback is never offered as complete. Each full
                            // request, legacy note and check gets its own copy source.
                            model: reviewRound.modelData.complete ? view.reviewFields(reviewRound.verdict) : []
                            delegate: StageProseField {
                              required property var modelData
                              width: stageRow.width
                              label: modelData.label
                              foreground: modelData.kind === "issues" ? view.urgent : view.mutedForeground
                              originalText: modelData.text
                              onHasSelectionChanged: if (!hasSelection) reviewUpdate.restart()
                            }
                          }
                        }
                      }
                    }
                    Text {
                      visible: stageRow.expanded && text !== ""
                      width: stageRow.width
                      text: UsageFormat.usageSummary(stageRow.modelData.usage)
                      textFormat: Text.PlainText
                      color: view.mutedForeground
                      wrapMode: Text.Wrap
                      font.family: view.fontFamily
                      font.pixelSize: view.fs(11)
                    }
                  }
                }
              }
              Loader {
                id: stageEditor
                active: stageRow.editable
                width: stageRow.width
                sourceComponent: Column {
                  width: stageEditor.width
                  spacing: Style.space(6)
                  function focusTitle() { titleEditor.focusField() }
                  Flow {
                    width: parent.width
                    spacing: Style.space(8)
                    Text {
                      text: stageRow.modelData.id === undefined ? "New stage"
                        : "Stage " + stageRow.modelData.id
                      color: view.foreground
                      font.family: view.fontFamily
                      font.pixelSize: view.fs(12)
                      font.bold: true
                    }
                    ViewButton {
                      label: "↑ Up"
                      enabled: !view.editPending && PlanEdit.editableNeighbor(view.editStages, stageRow.index, -1) >= 0
                      onClicked: view.moveEditStage(stageRow.index, -1)
                    }
                    ViewButton {
                      label: "↓ Down"
                      enabled: !view.editPending && PlanEdit.editableNeighbor(view.editStages, stageRow.index, 1) >= 0
                      onClicked: view.moveEditStage(stageRow.index, 1)
                    }
                    ViewButton {
                      label: "Delete"
                      enabled: !view.editPending
                      labelColor: view.urgent
                      onClicked: view.deleteEditStage(stageRow.index)
                    }
                  }
                  PlanEditField {
                    id: titleEditor
                    width: parent.width
                    label: "Title"
                    value: stageRow.modelData.title
                    onEdited: value => view.changeStageField(stageRow.index, "title", value)
                  }
                  PlanEditField {
                    width: parent.width
                    label: "Instructions"
                    value: stageRow.modelData.instructions
                    multiline: true
                    onEdited: value => view.changeStageField(stageRow.index, "instructions", value)
                  }
                  PlanEditField {
                    width: parent.width
                    label: "Acceptance"
                    value: stageRow.modelData.acceptance
                    multiline: true
                    onEdited: value => view.changeStageField(stageRow.index, "acceptance", value)
                  }
                  PlanEditField {
                    width: parent.width
                    label: "Commit"
                    value: stageRow.modelData.commit
                    onEdited: value => view.changeStageField(stageRow.index, "commit", value)
                  }
                  Text {
                    width: parent.width
                    text: "Stage constraint overrides the global model. Blank fields allow selection. Capability and independent-review checks still apply."
                    color: view.mutedForeground
                    wrapMode: Text.Wrap
                    font.family: view.fontFamily
                    font.pixelSize: view.fs(11)
                  }
                  Repeater {
                    model: ["provider", "model", "native_effort"]
                    delegate: PlanEditField {
                      required property string modelData
                      width: stageEditor.width
                      label: modelData === "provider" ? "Provider constraint (codex / claude)"
                        : modelData === "model" ? "Exact model ID constraint" : "Native effort constraint"
                      value: (stageRow.modelData.model_constraint || {})[modelData] || ""
                      onEdited: value => view.changeModelConstraint(stageRow.index, modelData, value)
                    }
                  }
                  ViewButton {
                    label: "Clear stage constraint"
                    onClicked: view.changeStageField(stageRow.index, "model_constraint", null)
                  }
                  Item { width: 1; height: Style.space(6) }
                }
              }
            }
            Text {
              visible: !view.editingPlan && view.plan === null
              text: "no plan yet"
              color: view.mutedForeground
              font.family: view.fontFamily
              font.pixelSize: view.fs(12)
            }
          }
        }
  component PlanEditField: Column {
    id: editField
    required property string label
    required property string value
    property bool multiline: false
    signal edited(string value)
    spacing: Style.space(3)
    enabled: !view.editPending

    function focusField() { field.forceActiveFocus() }

    Text {
      text: editField.label
      color: view.mutedForeground
      font.family: view.fontFamily
      font.pixelSize: view.fs(11)
    }
    Rectangle {
      width: parent.width
      height: Math.min(Math.max(Style.space(editField.multiline ? 48 : 26),
        field.contentHeight + Style.space(12)), Style.space(editField.multiline ? 96 : 26))
      color: view.background
      radius: 4
      border.width: 1
      border.color: field.activeFocus ? view.accent : Qt.darker(view.foreground, 3)
      Flickable {
        id: fieldFlick
        anchors.fill: parent
        anchors.margins: Style.space(6)
        clip: true
        contentWidth: field.width
        contentHeight: field.height
        flickableDirection: Flickable.VerticalFlick
        boundsBehavior: Flickable.StopAtBounds

        function ensureCursorVisible() {
          if (!field.activeFocus) return
          const cursor = field.cursorRectangle
          if (contentY > cursor.y) contentY = cursor.y
          else if (contentY + height < cursor.y + cursor.height)
            contentY = cursor.y + cursor.height - height
          contentY = Math.max(0, Math.min(contentY, contentHeight - height))
          // Keep the active field visible even when its stage exceeds the viewport.
          const top = editField.mapToItem(stageList.contentItem, 0, 0).y
          if (top < stageList.contentY) stageList.contentY = top
          else if (top + editField.height > stageList.contentY + stageList.height)
            stageList.contentY = top + editField.height - stageList.height
        }
        onHeightChanged: Qt.callLater(ensureCursorVisible)
        TextEdit {
          id: field
          width: fieldFlick.width
          height: Math.max(contentHeight, fieldFlick.height)
          text: editField.value
          textFormat: TextEdit.PlainText
          wrapMode: TextEdit.Wrap
          selectByMouse: true
          color: view.foreground
          font.family: view.fontFamily
          font.pixelSize: view.fs(12)
          onTextChanged: if (activeFocus) editField.edited(text)
          onActiveFocusChanged: {
            if (activeFocus) {
              view.editFocusChanged(field)
              fieldFlick.ensureCursorVisible()
            } else if (view.editFocusedField === field) view.editFocusChanged(null)
          }
          onCursorRectangleChanged: fieldFlick.ensureCursorVisible()
          Keys.onPressed: event => {
            if (event.key === Qt.Key_F1) {
              view.helpRequested()
              view.leaveRequested()
              event.accepted = true
            } else if (!editField.multiline
                       && (event.key === Qt.Key_Return || event.key === Qt.Key_Enter)) {
              event.accepted = true
            }
          }
          Keys.onEscapePressed: event => {
            view.leaveRequested()
            event.accepted = true
          }
        }
      }
    }
  }

  component StageProseField: StageProse {
    id: stageProse

    foreground: view.mutedForeground
    mutedForeground: view.mutedForeground
    background: view.background
    fontFamily: view.fontFamily
    fontSize: view.fs(11)
    onCopyRequested: original => Quickshell.clipboardText = original
    onLeaveRequested: view.leaveRequested()
    onFocusRevealed: control => view.detailRevealed(control)
    onInspecting: view.detailInspected(stageProse)
  }

  component ViewButton: PanelViewButton {
    foreground: view.foreground
    background: view.background
    surface: view.surface
    accent: view.accent
    fontFamily: view.fontFamily
    fontSize: view.fs(11)
  }
}
