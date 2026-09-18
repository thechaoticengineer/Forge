pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Controls as QQC
import "ModelRouting.js" as ModelRouting
import "PanelNavigation.js" as PanelNavigation
import "ReviewPresentation.js" as ReviewPresentation
import "StagePresentation.js" as StagePresentation
import "UsageFormat.js" as UsageFormat

// Interface: the page of one stage, pushed from the Plan or Overview tab. A
// breadcrumb back to Plan with the previous and next stage, a status line and the
// sub-tabs Instructions / Acceptance / Review / Routing / Output, which together
// hold everything the inline expanded stage showed: the instructions and commit
// message, the acceptance criteria, the review gate and the historical reviews,
// the model routing and its diagnostics, and the usage and live output. The
// stages, the shown index, the sub-tab and the stage's review and prose state
// enter as properties; navigation, review loading and the stage's actions leave as
// signals. handleKey(event) acts on the keys ] [ l h Escape and q and answers
// "handled" or "". Imports no shell modules, so qmltestrunner loads it on its own.
Item {
  id: page

  required property var stages
  required property int stageIndex
  required property string subTab
  // The prose Panel.qml captured when the stage was opened; used only when its
  // key matches the shown stage.
  required property var stageSnapshot
  required property var stageReviewBlocks
  required property bool stageRoutingExpanded
  required property real agentNow
  required property bool editingPlan
  required property var reviewView
  required property var stageDetailScope
  required property var stageActivity
  required property var stageReviewIncompleteRange
  required property var ensureStageReviewsLoaded
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
  property real spacing: 8
  property real horizontalPadding: 18
  property real verticalPadding: 10
  property real scrollBarSpace: 16
  // The left and right margin of the page content; Panel.qml passes the shell margin.
  property real margin: 0

  property alias contentFlick: flick

  signal backRequested
  signal stageStepRequested(int step)
  signal subTabRequested(string id)
  signal loadStageReviews(int stageId, var cursor, var end)
  signal retryStageReviews(var stage)
  signal stageRoutingExpandedRequested(bool expanded)
  signal diffRequested
  signal liveOutputRequested
  signal copyRequested(string original)
  signal leaveRequested
  signal detailRevealed(var control)
  signal detailInspected(var control)

  // The shown stage, or null when the index points at none.
  readonly property var current: stageIndex >= 0 && stageIndex < stages.length ? stages[stageIndex] : null
  readonly property bool hasStage: current !== null
  readonly property var stage: current || ({})
  readonly property int stageId: hasStage && stage.id !== undefined ? stage.id : -1
  readonly property bool hasPrevious: hasStage && stageIndex > 0
  readonly property bool hasNext: hasStage && stageIndex < stages.length - 1
  readonly property string detailScope: hasStage ? stageDetailScope(current) : ""
  // A stage being edited in Plan shows no prose, as the inline expansion never did.
  readonly property bool editable: editingPlan && stage.status !== "committed"
  readonly property var prose: hasStage && !editable && stageSnapshot && stageSnapshot.key === detailScope
    ? stageSnapshot : null
  readonly property string activity: hasStage ? stageActivity(current) : ""
  // The same elapsed/duration rule as the stage rows.
  readonly property double elapsedSecs: stage.status === "in_progress" && typeof stage.started_unix === "number"
    ? Math.max(0, Math.floor(agentNow - stage.started_unix))
    : typeof stage.duration_secs === "number" ? Math.max(0, Math.floor(stage.duration_secs)) : -1
  readonly property color statusColor: stage.status === "committed" ? success
    : stage.status === "in_progress" ? working
    : stage.status === "blocked" || stage.status === "failed" ? urgent : mutedForeground
  readonly property var noReviews: ({ rows: [], scope: { key: "", count: 0 }, pending: null,
    error: "", retry: null, older: 0 })
  readonly property var reviews: hasStage ? reviewView(current) : noReviews
  readonly property var reviewHistory: reviews.rows
  readonly property string reviewMessage: reviews.error || stageReviewBlocks[detailScope] || ""
  readonly property bool reviewRetryAvailable: reviewMessage !== "" && (!!reviews.retry
    || stageReviewIncompleteRange(reviews) !== null || reviews.older > 0)
  readonly property var lastReview: reviewHistory.length > 0 ? reviewHistory[reviewHistory.length - 1] : null
  readonly property var lastDecision: lastReview ? ReviewPresentation.reviewDecision(lastReview.verdict) : null
  readonly property real bodyWidth: Math.max(0, flick.width - scrollBarSpace)

  function stageNumber(index) {
    const candidate = stages[index]
    return candidate && candidate.id !== undefined ? candidate.id : index + 1
  }

  function contentFor(id) {
    return id === "instructions" ? instructionsTab : id === "acceptance" ? acceptanceTab
      : id === "review" ? reviewTab : id === "routing" ? routingTab : id === "output" ? outputTab : null
  }

  // Scroll a control that took keyboard focus into view.
  function reveal(item) {
    const top = item.mapToItem(flick.contentItem, 0, 0).y
    if (top < flick.contentY || top + item.height > flick.contentY + flick.height)
      flick.contentY = Math.max(0, Math.min(Math.max(0, flick.contentHeight - flick.height), top))
  }

  // ] and [ step between stages, l and h between sub-tabs, Escape and q go back.
  function handleKey(event) {
    if (event.modifiers !== Qt.NoModifier) return ""
    if (event.key === Qt.Key_Escape || event.key === Qt.Key_Q) {
      page.backRequested()
    } else if (!hasStage) {
      return ""
    } else if (event.key === Qt.Key_BracketRight) {
      page.stageStepRequested(1)
    } else if (event.key === Qt.Key_BracketLeft) {
      page.stageStepRequested(-1)
    } else if (event.key === Qt.Key_L) {
      page.subTabRequested(PanelNavigation.stepDetailTab(page.subTab, 1))
    } else if (event.key === Qt.Key_H) {
      page.subTabRequested(PanelNavigation.stepDetailTab(page.subTab, -1))
    } else {
      return ""
    }
    event.accepted = true
    return "handled"
  }

  // A different stage or sub-tab starts at its top; polling keeps the reading position.
  onStageIdChanged: flick.contentY = 0
  onSubTabChanged: {
    flick.contentY = 0
    reviewUpdate.restart()
  }
  onVisibleChanged: if (visible) reviewUpdate.restart()

  // ------------------------------------------------------------- header
  Column {
    id: header

    x: page.margin
    width: parent.width - page.margin * 2
    spacing: page.spacing

    Item {
      id: breadcrumb

      objectName: "stageBreadcrumb"
      width: parent.width
      height: Math.max(backText.implicitHeight, crumbTitle.implicitHeight) + 6

      Item {
        id: backLink

        objectName: "stageBack"
        width: backText.implicitWidth
        height: parent.height

        Text {
          id: backText

          anchors.verticalCenter: parent.verticalCenter
          text: "‹ Plan"
          textFormat: Text.PlainText
          color: page.accent
          font.family: page.fontFamily
          font.pixelSize: page.fontSize12
        }

        MouseArea {
          anchors.fill: parent
          cursorShape: Qt.PointingHandCursor
          onClicked: page.backRequested()
        }
      }

      Text {
        id: crumbSlash

        visible: page.hasStage
        anchors.left: backLink.right
        anchors.leftMargin: page.spacing
        anchors.verticalCenter: parent.verticalCenter
        text: "/"
        color: page.mutedForeground
        font.family: page.fontFamily
        font.pixelSize: page.fontSize12
      }

      Text {
        id: crumbTitle

        visible: page.hasStage
        anchors.left: crumbSlash.right
        anchors.leftMargin: page.spacing
        anchors.right: stepper.left
        anchors.rightMargin: page.spacing
        anchors.verticalCenter: parent.verticalCenter
        text: page.hasStage ? page.stageNumber(page.stageIndex) + ". " + (page.stage.title || "") : ""
        textFormat: Text.PlainText
        maximumLineCount: 1
        elide: Text.ElideRight
        color: page.foreground
        font.family: page.fontFamily
        font.pixelSize: page.fontSize12
        font.bold: true
      }

      Row {
        id: stepper

        visible: page.hasStage
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        spacing: page.spacing

        Item {
          id: previousStage

          objectName: "stagePrevious"
          enabled: page.hasPrevious
          width: previousText.implicitWidth + page.spacing
          height: breadcrumb.height
          opacity: enabled ? 1 : 0.35
          Accessible.name: "Previous stage"

          Text {
            id: previousText

            anchors.centerIn: parent
            text: "‹ " + (page.hasPrevious ? page.stageNumber(page.stageIndex - 1) : "")
            textFormat: Text.PlainText
            color: page.mutedForeground
            font.family: page.fontFamily
            font.pixelSize: page.fontSize11
          }

          MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: page.stageStepRequested(-1)
          }
        }

        Text {
          anchors.verticalCenter: parent.verticalCenter
          visible: page.hasPrevious && page.hasNext
          text: "·"
          textFormat: Text.PlainText
          color: page.mutedForeground
          font.family: page.fontFamily
          font.pixelSize: page.fontSize11
        }

        Item {
          id: nextStage

          objectName: "stageNext"
          enabled: page.hasNext
          width: nextText.implicitWidth + page.spacing
          height: breadcrumb.height
          opacity: enabled ? 1 : 0.35
          Accessible.name: "Next stage"

          Text {
            id: nextText

            anchors.centerIn: parent
            text: (page.hasNext ? page.stageNumber(page.stageIndex + 1) : "") + " ›"
            textFormat: Text.PlainText
            color: page.mutedForeground
            font.family: page.fontFamily
            font.pixelSize: page.fontSize11
          }

          MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: page.stageStepRequested(1)
          }
        }
      }
    }

    Item {
      id: statusLine

      objectName: "stageStatusLine"
      visible: page.hasStage
      width: parent.width
      height: visible ? statusText.implicitHeight + 2 : 0

      Text {
        id: statusText

        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        width: Math.min(implicitWidth, parent.width * 0.7)
        text: page.hasStage ? (page.stage.status === "committed" ? "✓ " : page.stage.status === "in_progress" ? "● "
            : page.stage.status === "blocked" || page.stage.status === "failed" ? "! " : "· ")
          + StagePresentation.statusText(page.stage, page.editingPlan).replace(/_/g, " ")
          + (page.activity !== "" ? " · " + page.activity : "")
          + (page.elapsedSecs >= 0 ? " · " + Math.floor(page.elapsedSecs / 60) + "m "
            + (page.elapsedSecs % 60) + "s" : "") : ""
        textFormat: Text.PlainText
        maximumLineCount: 1
        elide: Text.ElideRight
        color: page.statusColor
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
      }

      Text {
        anchors.left: statusText.right
        anchors.leftMargin: page.spacing
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        text: page.hasStage ? ModelRouting.stageModelStatus(page.stage).split("\n")[0] : ""
        textFormat: Text.PlainText
        maximumLineCount: 1
        elide: Text.ElideRight
        color: page.mutedForeground
        font.family: page.fontFamily
        font.pixelSize: page.fontSize10
      }
    }

    Row {
      id: subTabBar

      visible: page.hasStage
      spacing: 2

      Repeater {
        model: PanelNavigation.stageDetailTabs
        delegate: Item {
          id: subTabButton

          required property string modelData
          readonly property bool selected: page.subTab === modelData

          objectName: "stageSubTab_" + modelData
          width: subTabText.implicitWidth + page.spacing * 2
          height: subTabText.implicitHeight + page.spacing
          Accessible.name: PanelNavigation.stageDetailTabLabels[modelData]

          Rectangle {
            anchors.fill: parent
            radius: 4
            color: page.surface
            opacity: subTabButton.selected ? 1 : subTabArea.containsMouse ? 0.6 : 0
          }

          Text {
            id: subTabText

            anchors.centerIn: parent
            text: PanelNavigation.stageDetailTabLabels[subTabButton.modelData]
            textFormat: Text.PlainText
            color: subTabButton.selected ? page.foreground : page.mutedForeground
            font.family: page.fontFamily
            font.pixelSize: page.fontSize11
          }

          MouseArea {
            id: subTabArea

            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: page.subTabRequested(subTabButton.modelData)
          }
        }
      }
    }
  }

  Text {
    visible: !page.hasStage
    anchors.top: header.bottom
    anchors.topMargin: page.spacing
    x: page.margin
    width: parent.width - page.margin * 2
    text: "No stage to show."
    textFormat: Text.PlainText
    color: page.mutedForeground
    font.family: page.fontFamily
    font.pixelSize: page.fontSize12
  }

  // -------------------------------------------------------- sub-tab content
  Flickable {
    id: flick

    visible: page.hasStage
    anchors.top: header.bottom
    anchors.topMargin: page.spacing
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.bottom: parent.bottom
    anchors.leftMargin: page.margin
    anchors.rightMargin: page.margin
    clip: true
    contentWidth: width
    contentHeight: {
      const shown = page.contentFor(page.subTab)
      return shown ? shown.implicitHeight : 0
    }
    flickableDirection: Flickable.VerticalFlick
    boundsBehavior: Flickable.StopAtBounds
    // A shorter sub-tab or stage cannot leave the view scrolled past its end.
    onContentHeightChanged: contentY = Math.max(0, Math.min(contentY, contentHeight - height))
    ScrollBar.vertical: ScrollBar {
      policy: ScrollBar.AsNeeded
    }

    Column {
      id: instructionsTab

      objectName: "stageSubTabContent_instructions"
      visible: page.subTab === "instructions"
      width: page.bodyWidth
      spacing: 2

      Text {
        visible: page.prose === null
        width: parent.width
        text: page.editable ? "This stage is being edited in Plan." : "Loading stage details…"
        textFormat: Text.PlainText
        color: page.mutedForeground
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
      }
      StageProseField {
        visible: page.prose !== null
        width: parent.width
        label: "Instructions"
        originalText: page.prose ? page.prose.instructions || "" : ""
      }
      StageProseField {
        visible: page.prose !== null
        width: parent.width
        label: "Commit"
        originalText: page.prose ? page.prose.commit || "" : ""
      }
      Flow {
        visible: page.prose !== null
        width: parent.width
        spacing: page.spacing
        PageButton {
          label: "View diff"
          onClicked: page.diffRequested()
        }
        PageButton {
          label: "Live output ›"
          onClicked: page.liveOutputRequested()
        }
        PageButton {
          label: "Copy"
          onClicked: page.copyRequested(page.prose ? page.prose.instructions || "" : "")
        }
      }
    }

    Column {
      id: acceptanceTab

      objectName: "stageSubTabContent_acceptance"
      visible: page.subTab === "acceptance"
      width: page.bodyWidth
      spacing: 2

      Text {
        visible: page.prose === null
        width: parent.width
        text: page.editable ? "This stage is being edited in Plan." : "Loading stage details…"
        textFormat: Text.PlainText
        color: page.mutedForeground
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
      }
      StageProseField {
        visible: page.prose !== null
        width: parent.width
        label: "Acceptance criteria"
        originalText: page.prose ? page.prose.acceptance || "" : ""
      }
    }

    Column {
      id: reviewTab

      objectName: "stageSubTabContent_review"
      visible: page.subTab === "review"
      width: page.bodyWidth
      spacing: 2

      Text {
        visible: !!page.stage.review_gate
        width: parent.width
        text: ReviewPresentation.reviewGateText(page.stage)
        textFormat: Text.PlainText
        color: page.mutedForeground
        wrapMode: Text.Wrap
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
      }
      Text {
        visible: page.reviewHistory.length > 0
        width: parent.width
        text: page.lastReview && page.lastDecision
          ? "historical review · " + ReviewPresentation.reviewRoundLabel(page.lastReview)
            + ": " + page.lastDecision.label
            + (page.stage.last_verdict_valid === false ? " · obsolete for current work" : "")
          : ""
        textFormat: Text.PlainText
        color: page.stage.last_verdict_valid === false ? page.mutedForeground
          : page.lastDecision && page.lastDecision.optionalNotes ? page.working
          : page.lastDecision && page.lastDecision.clean
          ? (page.activity ? page.mutedForeground : page.success) : page.urgent
        wrapMode: Text.Wrap
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
      }
      StageProseField {
        visible: page.prose !== null
        width: parent.width
        label: "Review policy rationale"
        originalText: page.prose ? page.prose.policyRationale || "" : ""
      }
      Column {
        id: stageReviews

        objectName: "stageHistoricalReviews"
        readonly property var currentRows: page.reviewHistory
        readonly property string currentScope: page.reviews.scope.key
        readonly property bool planEditing: page.editingPlan
        readonly property bool heldPreview: ReviewPresentation.stageReviewHasHeldPreview(reviewPresentation, page.reviews)
        // A single queued update coalesces publications and runs against current
        // state, outside binding evaluation.
        onCurrentRowsChanged: reviewUpdate.restart()
        onCurrentScopeChanged: reviewUpdate.restart()
        onPlanEditingChanged: reviewUpdate.restart()
        width: parent.width
        spacing: 2

        ListModel { id: reviewPresentation; dynamicRoles: true }
        Text {
          visible: page.reviewHistory.length > 0
          width: parent.width
          text: "Historical reviews · showing " + page.reviewHistory.length
            + " of " + page.reviews.scope.count
            + (page.reviewHistory.some(function(r) { return !r.complete }) ? " · previews require full-text loading" : "")
          textFormat: Text.PlainText
          color: page.mutedForeground
          wrapMode: Text.Wrap
          font.family: page.fontFamily
          font.pixelSize: page.fontSize11
        }
        Text {
          objectName: "stageReviewStatus"
          visible: text !== ""
          width: parent.width
          text: page.reviewMessage || (page.reviews.pending ? "Loading complete reviews…"
            : page.stageReviewIncompleteRange(page.reviews) ? "Complete reviews have not been loaded."
            : stageReviews.heldPreview ? "Selected preview retained; complete reviews are shown separately." : "")
          textFormat: Text.PlainText
          wrapMode: Text.Wrap
          color: page.reviewMessage ? page.urgent : page.mutedForeground
          font.family: page.fontFamily
          font.pixelSize: page.fontSize11
        }
        Flow {
          width: parent.width
          spacing: page.spacing
          QQC.Button {
            objectName: "stageReviewsOlder"
            visible: page.reviews.older > 0
            width: Math.min(implicitWidth, page.bodyWidth)
            text: "Load older reviews (" + page.reviews.older + ")"
            enabled: !page.reviews.pending
            onClicked: page.loadStageReviews(page.stageId,
              Math.max(0, page.reviews.older - 8), page.reviews.older)
          }
          QQC.Button {
            objectName: "stageReviewsRetry"
            visible: page.reviewRetryAvailable
            width: Math.min(implicitWidth, page.bodyWidth)
            text: "Retry reviews"
            enabled: !page.reviews.pending
            onClicked: page.retryStageReviews(page.current)
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
            readonly property var decision: ReviewPresentation.reviewDecision(verdict)

            width: stageReviews.width
            spacing: 2
            Text {
              width: parent.width
              text: "Historical · " + (reviewRound.verdict.role || "reviewer") + " review "
                + ReviewPresentation.reviewRoundLabel(reviewRound.modelData) + " — "
                + reviewRound.decision.label
                + (reviewRound.sourceScope !== stageReviews.currentScope
                  ? " · selected text held from an earlier publication" : "")
              textFormat: Text.PlainText
              color: reviewRound.decision.optionalNotes ? page.working
                : reviewRound.decision.clean
                  ? (page.activity ? page.mutedForeground : page.success) : page.urgent
              wrapMode: Text.Wrap
              font.family: page.fontFamily
              font.pixelSize: page.fontSize11
            }
            Text {
              visible: text !== ""
              width: parent.width
              text: ReviewPresentation.reviewTimestamp(reviewRound.verdict)
              textFormat: Text.PlainText
              color: page.mutedForeground
              wrapMode: Text.Wrap
              font.family: page.fontFamily
              font.pixelSize: page.fontSize11
            }
            StageProseField {
              width: parent.width
              label: reviewRound.modelData.complete ? "Summary" : "Summary preview · feedback may be omitted"
              originalText: reviewRound.verdict.summary || ""
              onHasSelectionChanged: if (!hasSelection) reviewUpdate.restart()
            }
            Repeater {
              // Shortened feedback is never offered as complete. Each full
              // request, legacy note and check gets its own copy source.
              model: reviewRound.modelData.complete ? ReviewPresentation.reviewFields(reviewRound.verdict) : []
              delegate: StageProseField {
                required property var modelData
                width: reviewRound.width
                label: modelData.label
                foreground: modelData.kind === "issues" ? page.urgent : page.mutedForeground
                originalText: modelData.text
                onHasSelectionChanged: if (!hasSelection) reviewUpdate.restart()
              }
            }
          }
        }
      }
    }

    Column {
      id: routingTab

      objectName: "stageSubTabContent_routing"
      visible: page.subTab === "routing"
      width: page.bodyWidth
      spacing: 2

      Text {
        width: parent.width
        text: ModelRouting.stageModelStatus(page.stage)
        textFormat: Text.PlainText
        color: page.mutedForeground
        wrapMode: Text.Wrap
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
      }
      Text {
        visible: text !== ""
        width: parent.width
        text: ModelRouting.stageModelErrors(page.stage)
        textFormat: Text.PlainText
        color: page.urgent
        wrapMode: Text.Wrap
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
      }
      Repeater {
        model: page.prose ? page.prose.rationale : []
        delegate: StageProseField {
          required property var modelData
          width: routingTab.width
          label: modelData.label
          originalText: modelData.text
        }
      }
      QQC.Button {
        id: stageRoutingToggle

        objectName: "stageRoutingToggle"
        visible: page.prose !== null
        width: parent.width
        implicitHeight: Math.max(32, routingLabel.implicitHeight + 12)
        padding: 6
        focusPolicy: Qt.StrongFocus
        Accessible.name: (page.stageRoutingExpanded ? "Collapse " : "Expand ") + "model agreement and routing details"
        contentItem: Text {
          id: routingLabel

          text: (page.stageRoutingExpanded ? "▾  " : "▸  ") + "Model agreement and routing details"
          textFormat: Text.PlainText
          wrapMode: Text.Wrap
          color: page.foreground
          font.family: page.fontFamily
          font.pixelSize: page.fontSize11
        }
        background: Rectangle {
          radius: 4
          color: stageRoutingToggle.hovered || stageRoutingToggle.down ? Qt.alpha(page.foreground, 0.06) : "transparent"
          border.width: stageRoutingToggle.visualFocus ? 1 : 0
          border.color: page.accent
        }
        onClicked: page.stageRoutingExpandedRequested(!page.stageRoutingExpanded)
        onActiveFocusChanged: if (activeFocus && focusReason !== Qt.MouseFocusReason
          && focusReason !== Qt.PopupFocusReason) {
          page.reveal(stageRoutingToggle)
          page.detailRevealed(stageRoutingToggle)
        }
        Keys.priority: Keys.AfterItem
        Keys.onPressed: event => {
          if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) stageRoutingToggle.clicked()
          else if (event.key === Qt.Key_Escape) page.leaveRequested()
          if (event.key !== Qt.Key_Tab && event.key !== Qt.Key_Backtab) event.accepted = true
        }
      }
      Repeater {
        model: page.stageRoutingExpanded && page.prose ? page.prose.diagnostics : []
        delegate: StageProseField {
          required property var modelData
          width: routingTab.width
          label: modelData.label
          originalText: modelData.text
        }
      }
    }

    Column {
      id: outputTab

      objectName: "stageSubTabContent_output"
      visible: page.subTab === "output"
      width: page.bodyWidth
      spacing: 2

      Text {
        visible: text !== ""
        width: parent.width
        text: page.activity
        textFormat: Text.PlainText
        color: page.working
        wrapMode: Text.Wrap
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
        font.bold: true
      }
      Text {
        visible: text !== ""
        width: parent.width
        text: UsageFormat.usageSummary(page.stage.usage)
        textFormat: Text.PlainText
        color: page.mutedForeground
        wrapMode: Text.Wrap
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
      }
      Flow {
        width: parent.width
        spacing: page.spacing
        PageButton {
          label: "View diff"
          onClicked: page.diffRequested()
        }
        PageButton {
          label: "Live output ›"
          onClicked: page.liveOutputRequested()
        }
      }
    }
  }

  // Loads the stage's complete reviews while the page is shown, as the inline
  // expanded stage did, then reconciles the retained review preview rows.
  Timer {
    id: reviewUpdate

    interval: 0
    running: true
    onTriggered: {
      if (!page.hasStage) return
      if (page.visible) page.ensureStageReviewsLoaded(page.current)
      ReviewPresentation.reconcileStageReviewPresentation(page.reviews, page.detailScope,
        reviewPresentation, reviewRepeater)
    }
  }

  component StageProseField: StageProse {
    id: stageProse

    foreground: page.mutedForeground
    mutedForeground: page.mutedForeground
    background: page.background
    fontFamily: page.fontFamily
    fontSize: page.fontSize11
    onCopyRequested: original => page.copyRequested(original)
    onLeaveRequested: page.leaveRequested()
    onFocusRevealed: control => {
      page.reveal(control)
      page.detailRevealed(control)
    }
    onInspecting: page.detailInspected(stageProse)
  }

  component PageButton: PanelViewButton {
    foreground: page.foreground
    background: page.background
    surface: page.surface
    accent: page.accent
    fontFamily: page.fontFamily
    fontSize: page.fontSize11
    horizontalPadding: page.horizontalPadding
    verticalPadding: page.verticalPadding
  }
}
