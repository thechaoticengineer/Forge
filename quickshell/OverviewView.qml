pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import "StagePresentation.js" as StagePresentation

// Interface: the Overview tab. The goal, the actions possible in the current
// phase and a status summary: while busy a now-working card, a one-line progress
// summary and one compact row per stage; while idle the goal field and a short
// card for the last run. Engine state and the phase's action ids enter as
// properties; buttons leave as actionRequested(id) and links as openTab(id) or
// stageRequested(index), which Panel.qml maps to the calls the old buttons made.
// The goal TextEdit stays reachable as goalField, so drafts and enhancement undo
// survive polling. Imports no shell modules, so qmltestrunner loads it on its own.
Flickable {
  id: view

  required property var engineState
  required property var plan
  required property string phase
  required property bool busy
  required property var agent
  required property bool agentActive
  required property string agentElapsedText
  // The state heartbeat's latest line, bounded to 200 characters. The retained
  // feed (Panel.qml's liveEntries) replaces it when it has entries.
  required property string latestOutputText
  property ListModel liveEntries: ListModel {}
  required property string runSummaryText
  // Ids from PanelActions.overviewActions: only possible actions are shown.
  required property var actions
  required property string goalEnhanceStatus
  property bool goalEnhanceError: false
  property bool canApplyEnhancement: false
  property bool canUndoEnhancement: false
  required property string discussionStatus
  property bool discussionError: false
  required property int discussionCount
  property bool diffEnabled: true
  property bool hasReports: false
  // One-line usage limits under the last run card; the details live in Settings.
  property string limitsSummaryText: ""
  property real now: Date.now() / 1000
  property int selectedStageIndex: -1
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

  property alias goalField: goalField
  property alias goalFlick: goalFlick

  signal actionRequested(string id)
  signal openTab(string id)
  signal stageRequested(int index)
  signal overflowRequested()
  signal copyRequested(string original)
  signal helpRequested()
  signal leaveRequested()

  readonly property var stages: plan && plan.stages ? plan.stages : []
  readonly property var currentStageNumber: engineState ? engineState.current_stage : null
  readonly property var currentStage: stages.find(function(stage) {
    return stage.id === view.currentStageNumber
  }) || null
  // The first stage that needs attention, shown at the top with a link to its detail.
  readonly property int attentionIndex: stages.findIndex(function(stage) {
    return stage.status === "blocked" || stage.status === "failed"
  })

  readonly property var actionLabels: ({
    createPlan: "Create plan  p", discuss: "Discuss first", enhance: "Enhance with AI  E",
    addToQueue: "Add to queue", approve: "Plan is OK — approve  a", run: "Start implementing  r",
    editPlan: "Edit plan  e", stop: "Stop  x"
  })

  // The goal field hides while busy; give the keyboard back instead of losing focus.
  onBusyChanged: if (busy && goalField.activeFocus) leaveRequested()

  // The goal is read-only while busy, so there is nothing to focus then.
  function focusGoal() {
    if (view.busy) return
    goalField.forceActiveFocus()
    Qt.callLater(function() { view.reveal(goalBox) })
  }

  function reveal(item) {
    const top = item.mapToItem(contentItem, 0, 0).y
    if (top < contentY || top + item.height > contentY + height)
      contentY = Math.max(0, Math.min(Math.max(0, contentHeight - height), top))
  }

  clip: true
  contentWidth: width
  contentHeight: overviewColumn.implicitHeight
  flickableDirection: Flickable.VerticalFlick
  boundsBehavior: Flickable.StopAtBounds
  ScrollBar.vertical: ScrollBar {
    policy: ScrollBar.AsNeeded
  }

  Column {
    id: overviewColumn

    width: view.width - view.scrollBarSpace
    spacing: view.spacing + 2

    // ---------------------------------------------------- goal
    Column {
      objectName: "overviewGoal"
      width: parent.width
      spacing: view.spacing / 2

      Text {
        text: "GOAL"
        color: view.mutedForeground
        font.family: view.fontFamily
        font.pixelSize: view.fontSize10
        font.letterSpacing: 1
      }

      // While busy the goal is read-only; the draft in goalField is kept.
      Text {
        visible: view.busy
        width: parent.width
        text: view.engineState ? view.engineState.goal || "" : ""
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        maximumLineCount: 2
        elide: Text.ElideRight
        color: view.foreground
        font.family: view.fontFamily
        font.pixelSize: view.fontSize12
      }

      Rectangle {
        id: goalBox

        visible: !view.busy
        width: parent.width
        height: Math.min(Math.max(52, goalField.contentHeight + 16), 140)
        color: view.surface
        radius: 4
        border.width: 1
        border.color: goalField.activeFocus
          ? view.accent : Qt.darker(view.foreground, 3)
        Flickable {
          id: goalFlick
          anchors.fill: parent
          anchors.margins: 6
          clip: true
          contentWidth: goalField.width
          contentHeight: goalField.height
          flickableDirection: Flickable.VerticalFlick
          boundsBehavior: Flickable.StopAtBounds

          function ensureCursorVisible() {
            const cursor = goalField.cursorRectangle
            if (contentY > cursor.y)
              contentY = cursor.y
            else if (contentY + height < cursor.y + cursor.height)
              contentY = cursor.y + cursor.height - height
            contentY = Math.max(0, Math.min(contentY, contentHeight - height))
          }

          onHeightChanged: Qt.callLater(ensureCursorVisible)
          onContentHeightChanged: Qt.callLater(ensureCursorVisible)

          TextEdit {
            id: goalField
            Keys.onPressed: event => {
              if (event.key === Qt.Key_F1) {
                view.helpRequested()
                view.leaveRequested()
                event.accepted = true
              }
            }
            Keys.onEscapePressed: event => {
              view.leaveRequested()
              event.accepted = true
            }
            width: goalFlick.width
            height: Math.max(contentHeight, goalFlick.height)
            wrapMode: TextEdit.Wrap
            color: view.foreground
            font.family: view.fontFamily
            font.pixelSize: view.fontSize12
            onCursorRectangleChanged: goalFlick.ensureCursorVisible()
            Text {
              visible: goalField.text === "" && !goalField.activeFocus
              text: "Describe a goal, or leave empty and press Refactor plan for suggestions"
              color: view.mutedForeground
              font.family: view.fontFamily
              font.pixelSize: view.fontSize12
            }
          }
        }
      }
    }

    // ------------------------------------------------ needs attention
    // A blocked or failed stage comes first, in every phase; it opens its stage detail.
    Text {
      objectName: "overviewAttention"
      readonly property var stage: view.attentionIndex >= 0 ? view.stages[view.attentionIndex] : null
      visible: stage !== null
      width: parent.width
      text: stage ? "! stage " + stage.id + " · " + stage.title + " · " + stage.status
        + " — " + StagePresentation.attentionReason(stage) + " ›" : ""
      textFormat: Text.PlainText
      elide: Text.ElideRight
      color: view.urgent
      font.family: view.fontFamily
      font.pixelSize: view.fontSize11
      MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: view.stageRequested(view.attentionIndex)
      }
    }

    Text {
      width: parent.width
      visible: text !== ""
      text: view.goalEnhanceStatus
      textFormat: Text.PlainText
      color: view.goalEnhanceError ? view.urgent : view.mutedForeground
      wrapMode: Text.Wrap
      font.family: view.fontFamily
      font.pixelSize: view.fontSize11
    }

    // ------------------------------------------------ now working
    Rectangle {
      objectName: "nowWorkingCard"
      visible: view.busy
      width: parent.width
      height: visible ? workingColumn.implicitHeight + 16 : 0
      color: view.surface
      radius: 4

      Column {
        id: workingColumn

        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.margins: 8
        spacing: 4

        Item {
          width: parent.width
          height: workingTitle.implicitHeight
          Text {
            id: workingTitle
            anchors.left: parent.left
            anchors.right: workingTime.left
            anchors.rightMargin: view.spacing
            text: "● " + (view.phase === "planning" ? "planning…"
              : view.currentStageNumber !== null && view.currentStageNumber !== undefined
                ? "stage " + view.currentStageNumber
                  + (view.currentStage ? " · " + view.currentStage.title : "")
                : "now working")
            textFormat: Text.PlainText
            elide: Text.ElideRight
            color: view.working
            font.family: view.fontFamily
            font.pixelSize: view.fontSize11
          }
          Text {
            id: workingTime
            anchors.right: parent.right
            visible: text !== ""
            text: view.agentActive ? view.agentElapsedText : ""
            textFormat: Text.PlainText
            color: view.mutedForeground
            font.family: view.fontFamily
            font.pixelSize: view.fontSize11
          }
        }
        Text {
          visible: view.agentActive
          width: parent.width
          text: view.agentActive ? view.agent.role + " · " + view.agent.tool
            + (view.agent.model ? " · " + view.agent.model : "")
            + " · " + view.agent.lines + " lines" : ""
          textFormat: Text.PlainText
          elide: Text.ElideRight
          color: view.mutedForeground
          font.family: view.fontFamily
          font.pixelSize: view.fontSize11
        }
        // Shows the first line. Full-text actions must use the complete retained
        // feed, never the bounded heartbeat line: a click copies originalText.
        Text {
          id: latestLine
          readonly property ListModel liveEntries: view.liveEntries
          readonly property string originalText: liveEntries.count > 0 ? liveEntries.get(liveEntries.count - 1).originalText
            : view.latestOutputText
          readonly property bool error: liveEntries.count > 0 && (liveEntries.get(liveEntries.count - 1).kind === "error"
            || liveEntries.get(liveEntries.count - 1).stream === "stderr")
          objectName: "latestOutput"
          visible: view.agentActive && originalText !== ""
          width: parent.width
          text: "› " + originalText.split("\n")[0]
          textFormat: Text.PlainText
          maximumLineCount: 1
          elide: Text.ElideRight
          color: error ? view.urgent : view.foreground
          font.family: view.fontFamily
          font.pixelSize: view.fontSize11
          MouseArea {
            anchors.fill: parent
            cursorShape: Qt.PointingHandCursor
            onClicked: view.copyRequested(latestLine.originalText)
          }
        }
      }
    }

    // ------------------------------------------------ stages
    Item {
      objectName: "progressSummary"
      visible: view.busy && view.runSummaryText !== ""
      width: parent.width
      height: visible ? summaryText.implicitHeight : 0
      Text {
        id: summaryLabel
        anchors.verticalCenter: summaryText.verticalCenter
        text: "STAGES"
        color: view.mutedForeground
        font.family: view.fontFamily
        font.pixelSize: view.fontSize10
        font.letterSpacing: 1
      }
      Text {
        id: summaryText
        anchors.left: summaryLabel.right
        anchors.leftMargin: view.spacing
        anchors.right: planLink.left
        anchors.rightMargin: view.spacing
        text: view.runSummaryText
        textFormat: Text.PlainText
        maximumLineCount: 1
        elide: Text.ElideRight
        color: view.mutedForeground
        font.family: view.fontFamily
        font.pixelSize: view.fontSize11
      }
      Text {
        id: planLink
        anchors.right: parent.right
        anchors.verticalCenter: summaryText.verticalCenter
        text: "Plan ›"
        color: view.accent
        font.family: view.fontFamily
        font.pixelSize: view.fontSize11
        MouseArea {
          anchors.fill: parent
          cursorShape: Qt.PointingHandCursor
          onClicked: view.openTab("plan")
        }
      }
    }

    Column {
      visible: view.busy
      width: parent.width
      Repeater {
        model: view.busy ? view.stages.length : 0
        delegate: OverviewStageRow {
          required property int index
          objectName: "overviewStageRow"
          width: parent.width
          stage: view.stages[index] || ({})
          number: stage.id !== undefined ? stage.id : index + 1
          activityText: stage.status === "in_progress" && view.engineState
            && stage.id === view.currentStageNumber ? view.engineState.current_step || "" : ""
          now: view.now
          selected: index === view.selectedStageIndex
          foreground: view.foreground
          mutedForeground: view.mutedForeground
          accent: view.accent
          urgent: view.urgent
          success: view.success
          working: view.working
          fontFamily: view.fontFamily
          fontSize11: view.fontSize11
          fontSize12: view.fontSize12
          spacing: view.spacing
          onClicked: view.stageRequested(index)
        }
      }
    }

    // ------------------------------------------------ actions
    Flow {
      width: parent.width
      spacing: view.spacing

      Repeater {
        model: view.actions
        delegate: OverviewButton {
          required property string modelData
          objectName: "overviewAction_" + modelData
          label: (view.actionLabels[modelData] || modelData)
            + (modelData === "discuss" && view.discussionCount > 0 ? " (" + view.discussionCount + ")  t"
              : modelData === "discuss" ? "  t" : "")
          primary: modelData === "createPlan" || modelData === "run"
          onClicked: view.actionRequested(modelData)
        }
      }
      OverviewButton {
        objectName: "overviewAction_applyEnhancement"
        visible: view.canApplyEnhancement
        label: "Apply AI description"
        onClicked: view.actionRequested("applyEnhancement")
      }
      OverviewButton {
        objectName: "overviewAction_undoEnhancement"
        visible: view.canUndoEnhancement
        label: "Undo enhance"
        onClicked: view.actionRequested("undoEnhancement")
      }
      OverviewButton {
        objectName: "overviewLink_diff"
        visible: view.busy
        enabled: view.diffEnabled
        label: "View diff  d"
        onClicked: view.actionRequested("diff")
      }
      OverviewButton {
        objectName: "overviewLink_activity"
        visible: view.busy
        label: "Live output  g a"
        onClicked: view.openTab("activity")
      }
      OverviewButton {
        objectName: "overviewMore"
        visible: !view.busy
        label: "⋯ more"
        onClicked: view.overflowRequested()
      }
    }

    Text {
      width: parent.width
      visible: text !== ""
      text: view.discussionStatus
      textFormat: Text.PlainText
      color: view.discussionError ? view.urgent : view.mutedForeground
      wrapMode: Text.Wrap
      font.family: view.fontFamily
      font.pixelSize: view.fontSize11
    }

    // ------------------------------------------------ last run
    Rectangle {
      objectName: "lastRunCard"
      visible: !view.busy && view.runSummaryText !== ""
      width: parent.width
      height: visible ? lastRunColumn.implicitHeight + 16 : 0
      color: view.surface
      radius: 4

      Column {
        id: lastRunColumn

        anchors.top: parent.top
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.margins: 8
        spacing: view.spacing / 2 + 2

        Text {
          width: parent.width
          text: view.runSummaryText
          textFormat: Text.PlainText
          elide: Text.ElideRight
          color: view.mutedForeground
          font.family: view.fontFamily
          font.pixelSize: view.fontSize10
        }
        Text {
          visible: text !== ""
          width: parent.width
          text: view.plan && view.plan.goal ? view.plan.goal : ""
          textFormat: Text.PlainText
          wrapMode: Text.Wrap
          maximumLineCount: 2
          elide: Text.ElideRight
          color: view.foreground
          font.family: view.fontFamily
          font.pixelSize: view.fontSize11
        }
        Row {
          spacing: view.spacing
          OverviewButton {
            objectName: "lastRunPlan"
            visible: view.plan !== null
            label: "Plan ›"
            onClicked: view.openTab("plan")
          }
          OverviewButton {
            objectName: "lastRunReports"
            visible: view.hasReports
            label: "Reports ›"
            onClicked: view.actionRequested("reports")
          }
          OverviewButton {
            objectName: "lastRunDiff"
            enabled: view.diffEnabled
            label: "View diff"
            onClicked: view.actionRequested("diff")
          }
        }
      }
    }

    Text {
      objectName: "limitsSummary"
      visible: !view.busy && text !== ""
      width: parent.width
      text: view.limitsSummaryText
      textFormat: Text.PlainText
      elide: Text.ElideRight
      color: view.mutedForeground
      font.family: view.fontFamily
      font.pixelSize: view.fontSize11
    }
  }

  component OverviewButton: PanelViewButton {
    foreground: view.foreground
    background: view.background
    surface: view.surface
    accent: view.accent
    fontFamily: view.fontFamily
    fontSize: view.fontSize11
    horizontalPadding: view.horizontalPadding
    verticalPadding: view.verticalPadding
  }
}
