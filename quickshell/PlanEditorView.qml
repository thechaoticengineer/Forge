pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Controls as QQC
import qs.Commons
import "PlanEdit.js" as PlanEdit
import "StagePresentation.js" as StagePresentation

// Interface: the plan editor. Plan/edit projections enter as properties; edit,
// open-stage and focus actions leave as signals. Committed stages keep a locked
// read-only header whose toggle opens the stage detail page, which shows their
// details. The list, frame, and save control aliases preserve root keyboard
// routing and focus.
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
  required property int expandedStageId
  required property int selectedStageIndex
  required property real agentNow
  required property real panelHeight
  required property var editFocusedField
  required property var fs
  required property var stageActivity
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
  // A committed stage's locked header asks to open its stage detail page.
  signal stageOpened(int index)
  signal editFocusChanged(var field)
  signal helpRequested
  signal leaveRequested
  signal detailRevealed(var control)

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
                  Accessible.name: (stageRow.expanded ? "Close stage " : "Open stage ") + stageRow.modelData.id
                  contentItem: Row {
                    spacing: Style.space(4)
                    Text {
                      text: stageRow.expanded ? "▾" : "›"
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
                  onClicked: view.stageOpened(stageRow.index)
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
                    text: StagePresentation.statusText(stageRow.modelData, view.editingPlan)
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

  component ViewButton: PanelViewButton {
    foreground: view.foreground
    background: view.background
    surface: view.surface
    accent: view.accent
    fontFamily: view.fontFamily
    fontSize: view.fs(11)
    horizontalPadding: Style.space(18)
    verticalPadding: Style.space(10)
  }
}
