pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import "PanelDetails.js" as PanelDetails

// Interface: the Plan tab. A summary line with the plan actions the phase
// allows, one compact PlanStageList row per stage with the plan review strip,
// the "what should be improved" feedback field and the plan Q&A. While editing,
// PlanEditorView replaces the compact rows. Plan state and guards enter as
// properties; actions leave as actionRequested(id), which Panel.qml maps to the
// calls the old buttons made, and a stage row or the review strip leave as
// stageOpened(index) and planReviewRequested(). The editor, the stage rows, the
// two text fields and the chat list stay reachable as aliases, so root keyboard
// routing, drafts and reading positions keep working.
Flickable {
  id: view

  required property var guards
  required property bool editingPlan
  required property bool editPending
  required property bool editValid
  required property bool queueActive
  required property bool engineOnline
  required property bool busy
  required property var displayedStages
  required property var editStages
  required property var plan
  required property var planReview
  required property int expandedStageId
  required property int selectedStageIndex
  required property real agentNow
  required property var editFocusedField
  required property var fs
  required property var stageActivity
  // The current step of the stage being worked on, shown on its row.
  property string activityText: ""
  // The review cadence ("per plan" / "per stage") for the summary line.
  property string reviewCadenceText: ""
  required property var chat
  required property bool chatExpanded
  required property bool chatPending
  property string detailScope: ""
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
  property real spacing: 8
  property real fieldPadding: 6
  property real chatMaxHeight: 96
  property real horizontalPadding: 18
  property real verticalPadding: 10
  property real scrollBarSpace: 16

  property alias planEditor: planEditor
  property alias stageRows: stageRows
  property alias feedbackField: feedbackField
  property alias questionField: questionField
  property alias chatList: chatList

  signal actionRequested(string id)
  signal copyRequested(string original)
  signal detailRevealed(var control)
  signal chatExpandedRequested(bool expanded)
  signal stageOpened(int index)
  signal planReviewRequested
  signal savePlanEdit
  signal cancelPlanEdit
  signal addEditStage
  signal moveEditStage(int index, int direction)
  signal deleteEditStage(int index)
  signal changeStageField(int index, string field, var value)
  signal changeModelConstraint(int index, string key, string value)
  signal editFocusChanged(var field)
  signal helpRequested
  signal leaveRequested

  readonly property var stages: plan && plan.stages ? plan.stages : []
  readonly property string summaryText: stages.length === 0 ? "" : stages.length + " stages · "
    + stages.filter(function(stage) { return stage.status === "committed" }).length + " committed"
    + (reviewCadenceText !== "" && reviewCadenceText[0] !== "…" ? " · review: " + reviewCadenceText : "")

  function reveal(item) {
    const top = item.mapToItem(contentItem, 0, 0).y
    if (top < contentY || top + item.height > contentY + height)
      contentY = Math.max(0, Math.min(Math.max(0, contentHeight - height), top))
  }

  clip: true
  contentWidth: width
  contentHeight: planColumn.implicitHeight
  flickableDirection: Flickable.VerticalFlick
  boundsBehavior: Flickable.StopAtBounds
  ScrollBar.vertical: ScrollBar {
    policy: ScrollBar.AsNeeded
  }

  Column {
    id: planColumn

    width: view.width - view.scrollBarSpace
    spacing: view.spacing + 2

    // The plan summary and only the plan actions the current phase allows; the rest stay in ⋯.
    Item {
      width: parent.width
      height: Math.max(summaryLine.implicitHeight, actionRow.height)
      visible: summaryLine.text !== "" || actionRow.width > 0

      Text {
        id: summaryLine

        anchors.left: parent.left
        anchors.right: actionRow.left
        anchors.rightMargin: view.spacing
        anchors.verticalCenter: parent.verticalCenter
        text: view.summaryText
        textFormat: Text.PlainText
        elide: Text.ElideRight
        color: view.mutedForeground
        font.family: view.fontFamily
        font.pixelSize: view.fs(11)
      }

      Row {
        id: actionRow
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        spacing: view.spacing

        PlanButton {
          id: approveButton
          label: "Plan is OK — approve"
          visible: view.guards.approve
          onClicked: view.actionRequested("approve")
        }
        PlanButton {
          id: runButton
          label: "Start implementing"
          primary: true
          visible: view.guards.run
          onClicked: view.actionRequested("run")
        }
        PlanButton {
          id: editButton
          label: "Edit plan"
          visible: !view.editingPlan && view.guards.editPlan
          onClicked: view.actionRequested("editPlan")
        }
        PlanButton {
          objectName: "planQaButton"
          label: "Q&A"
          visible: view.plan !== null
          onClicked: view.chatExpandedRequested(!view.chatExpanded)
        }
      }
    }

    // ------------------------------------ compact stage rows, or the editor
    PlanStageList {
      id: stageRows

      visible: !view.editingPlan
      width: parent.width
      stages: view.displayedStages
      selectedIndex: view.selectedStageIndex
      now: view.agentNow
      planReview: view.planReview
      activityText: view.activityText
      theme: view.theme
      gap: view.spacing
      onStageOpened: index => view.stageOpened(index)
      onPlanReviewRequested: view.planReviewRequested()
    }

    Text {
      visible: !view.editingPlan && view.plan === null
      text: "no plan yet"
      color: view.mutedForeground
      font.family: view.fontFamily
      font.pixelSize: view.fs(12)
    }

    PlanEditorView {
      id: planEditor

      visible: view.editingPlan
      width: parent.width
      editingPlan: view.editingPlan
      editPending: view.editPending
      editValid: view.editValid
      queueActive: view.queueActive
      engineOnline: view.engineOnline
      busy: view.busy
      displayedStages: view.displayedStages
      editStages: view.editStages
      plan: view.plan
      expandedStageId: view.expandedStageId
      selectedStageIndex: view.selectedStageIndex
      agentNow: view.agentNow
      panelHeight: view.height
      editFocusedField: view.editFocusedField
      fs: view.fs
      stageActivity: view.stageActivity
      foreground: view.foreground
      mutedForeground: view.mutedForeground
      background: view.background
      surface: view.surface
      accent: view.accent
      urgent: view.urgent
      success: view.success
      working: view.working
      fontFamily: view.fontFamily
      onSavePlanEdit: view.savePlanEdit()
      onCancelPlanEdit: view.cancelPlanEdit()
      onAddEditStage: view.addEditStage()
      onMoveEditStage: (index, direction) => view.moveEditStage(index, direction)
      onDeleteEditStage: index => view.deleteEditStage(index)
      onChangeStageField: (index, field, value) => view.changeStageField(index, field, value)
      onChangeModelConstraint: (index, key, value) => view.changeModelConstraint(index, key, value)
      onStageOpened: index => view.stageOpened(index)
      onEditFocusChanged: field => view.editFocusChanged(field)
      onHelpRequested: view.helpRequested()
      onLeaveRequested: view.leaveRequested()
      onDetailRevealed: control => view.reveal(control)
    }

    // ------------------------------------------- what should be improved
    Row {
      width: parent.width
      spacing: view.spacing
      Rectangle {
        width: parent.width - improveButton.width - parent.spacing
        height: improveButton.height
        color: view.surface
        radius: 4
        border.width: 1
        border.color: feedbackField.activeFocus
          ? view.accent : Qt.darker(view.foreground, 3)
        TextInput {
          id: feedbackField
          anchors.fill: parent
          anchors.margins: view.fieldPadding
          verticalAlignment: TextInput.AlignVCenter
          color: view.foreground
          font.family: view.fontFamily
          font.pixelSize: view.fs(12)
          selectByMouse: true
          clip: true
          onAccepted: view.actionRequested("improve")
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
          Text {
            anchors.verticalCenter: parent.verticalCenter
            visible: feedbackField.text === "" && !feedbackField.activeFocus
            text: "what should be improved…"
            color: view.mutedForeground
            font.family: view.fontFamily
            font.pixelSize: view.fs(12)
          }
        }
      }
      PlanButton {
        id: improveButton
        label: "Improve with AI"
        enabled: view.guards.improve
        onClicked: view.actionRequested("improve")
      }
    }

    // ------------------------------------------------------- plan Q&A
    Column {
      id: chatSection
      objectName: "chatSection"
      visible: view.plan !== null
      width: parent.width
      spacing: view.fieldPadding

      Row {
        width: parent.width
        spacing: view.spacing
        PlanButton {
          id: chatToggle
          objectName: "chatToggle"
          label: (view.chatExpanded ? "▾" : "▸") + " Plan Q&A"
            + (view.chat.length > 0 ? " (" + view.chat.length + ")" : "")
          onClicked: view.chatExpandedRequested(!view.chatExpanded)
        }
        Rectangle {
          width: Math.max(0, parent.width - chatToggle.width - askButton.width
            - parent.spacing * 2)
          height: askButton.height
          color: view.surface
          radius: 4
          border.width: 1
          border.color: questionField.activeFocus
            ? view.accent : Qt.darker(view.foreground, 3)
          TextInput {
            id: questionField
            anchors.fill: parent
            anchors.margins: view.fieldPadding
            verticalAlignment: TextInput.AlignVCenter
            color: view.foreground
            font.family: view.fontFamily
            font.pixelSize: view.fs(12)
            selectByMouse: true
            clip: true
            onAccepted: view.actionRequested("ask")
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
            Text {
              anchors.verticalCenter: parent.verticalCenter
              visible: questionField.text === "" && !questionField.activeFocus
              text: "ask about this plan…"
              color: view.mutedForeground
              font.family: view.fontFamily
              font.pixelSize: view.fs(12)
            }
          }
        }
        PlanButton {
          id: askButton
          objectName: "askPlanButton"
          label: view.chatPending ? "Asking…" : "Ask"
          enabled: view.guards.ask
          onClicked: view.actionRequested("ask")
        }
      }

      Rectangle {
        visible: view.chatExpanded && view.chat.length > 0
        width: parent.width
        height: Math.min(chatList.contentHeight, view.chatMaxHeight) + view.spacing * 2
        color: view.surface
        radius: 4
        Flickable {
          id: chatList
          anchors.fill: parent
          anchors.margins: view.spacing
          clip: true
          boundsBehavior: Flickable.StopAtBounds
          contentHeight: chatDetails.height
          property bool followTail: true
          property real readingY: 0
          function scrollToTail() { if (followTail && !moving) contentY = Math.max(0, contentHeight - height) }
          function restoreReadingPosition() {
            if (!followTail && !moving) contentY = Math.max(0, Math.min(readingY, Math.max(0, contentHeight - height)))
          }
          onContentYChanged: if (moving) { followTail = atYEnd; readingY = contentY }
          onMovementEnded: { followTail = atYEnd; readingY = contentY }
          onContentHeightChanged: Qt.callLater(function() { restoreReadingPosition(); scrollToTail() })
          onHeightChanged: Qt.callLater(function() { restoreReadingPosition(); scrollToTail() })
          onVisibleChanged: if (visible) Qt.callLater(scrollToTail)
          DetailFields {
            id: chatDetails
            objectName: "chatDetails"
            width: chatList.width
            scope: view.detailScope
            entries: view.chat.map(function(message, i) {
              // Chat has no durable IDs. Scope by plan, position and the
              // entire original record, keeping identical adjacent messages distinct.
              return PanelDetails.field(JSON.stringify([i, message]),
                message.role === "user" ? "You" : "Forge", message.text)
            })
            foreground: view.mutedForeground
            mutedForeground: view.mutedForeground
            background: view.background
            urgent: view.urgent
            fontFamily: view.fontFamily
            fontSize: view.fs(11)
            onCopyRequested: original => view.copyRequested(original)
            onLeaveRequested: view.leaveRequested()
            onFocusRevealed: control => view.detailRevealed(control)
            onInspecting: { chatList.followTail = false; chatList.readingY = chatList.contentY }
          }
        }
      }
    }
  }

  component PlanButton: PanelViewButton {
    foreground: view.foreground
    background: view.background
    surface: view.surface
    accent: view.accent
    fontFamily: view.fontFamily
    fontSize: view.fs(11)
    horizontalPadding: view.horizontalPadding
    verticalPadding: view.verticalPadding
  }
}
