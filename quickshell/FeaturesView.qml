pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import "Features.js" as Features

// Interface: discovered feature rows, pending/error state and palette enter
// as properties; refresh, close and opening a feature in an editor leave as
// signals. listView is exposed for the root keyboard router. Deliberately
// avoids qs.Commons (unlike DiffView/ProjectChooser) so this component loads
// standalone under qmltestrunner, matching DiscussionChat's convention.
//
// M2: specs/activity/selectedSlug/detailState carry the spec-phase metadata
// (indexed separately from rows, see Features.featureSpecsBySlug) and are
// all optional with defaults so existing callers and tests are unaffected.
Rectangle {
  id: view

  required property bool open
  required property bool pending
  required property string errorText
  required property var rows
  // Palette and font sizes; Panel.qml passes its shared theme object.
  property var theme: null
  property color foreground: theme ? theme.foreground : "#dddddd"
  property color mutedForeground: theme ? theme.mutedForeground : "#aaaaaa"
  property color background: theme ? theme.background : "#202020"
  property color surface: theme ? theme.surface : "#282828"
  property color accent: theme ? theme.accent : "#6699ff"
  property color urgent: theme ? theme.urgent : "#ff6666"
  property color success: theme ? theme.success : "#4faf72"
  property string fontFamily: theme ? theme.fontFamily : "monospace"
  property real fontSize10: theme ? theme.fontSize10 : 10
  property real fontSize11: theme ? theme.fontSize11 : 11
  property alias listView: featuresList

  // M2: per-slug spec status/review metadata, the current chat/review
  // activity (or null) and the selected feature's detail state.
  property var specs: ({})
  property var activity: null
  property string selectedSlug: ""
  property var detailState: null
  // A tab fills its area: no dimmed backdrop, and clicking beside the list
  // does not close it. The default stays the full-window overlay.
  property bool embedded: false

  // Implemented features stay hidden until the user asks for them.
  property bool showImplemented: false
  readonly property var shownRows: Features.visibleRows(view.rows, view.specs, view.showImplemented)
  readonly property int implementedCount: Features.implementedCount(view.rows, view.specs)

  property bool newFeatureOpen: false
  property string newFeatureError: ""
  property alias newSlugField: newSlugFieldItem
  property alias newTitleField: newTitleFieldItem
  property alias chatMessageField: chatMessageFieldItem

  signal closeRequested()
  signal refreshRequested()
  signal openRequested(var feature)
  signal leaveRequested()
  signal createRequested(string slug, string title)
  signal chatRequested(var feature, string message)
  signal reviewRequested(var feature)
  signal approveSpecRequested(var feature)
  signal approveScenariosRequested(var feature)
  signal featureSelected(var feature)

  function submitNewFeature() {
    const message = Features.validateNewFeature(newSlugFieldItem.text, newTitleFieldItem.text)
    view.newFeatureError = message
    if (message !== "") return
    view.createRequested(newSlugFieldItem.text, newTitleFieldItem.text)
    view.newFeatureOpen = false
    newSlugFieldItem.text = ""
    newTitleFieldItem.text = ""
  }

  function cancelNewFeature() {
    view.newFeatureOpen = false
    view.newFeatureError = ""
    newSlugFieldItem.text = ""
    newTitleFieldItem.text = ""
  }

  // Keyboard entry points used by the root router, so the M2 actions are
  // reachable without the mouse.
  function openNewFeatureForm() {
    view.newFeatureOpen = true
    newSlugFieldItem.forceActiveFocus()
  }

  function focusChatInput() {
    if (view.selectedSlug === "") return
    chatMessageFieldItem.forceActiveFocus()
  }

  function currentRow() {
    const rows = featuresList.model || []
    return rows[featuresList.currentIndex] || null
  }

  // Normal-mode keys while the Features tab is shown. Returns the pending key
  // prefix ("g" after a first g).
  function handleKey(event) {
    const row = currentRow()
    if (event.key === Qt.Key_Escape) {
      view.closeRequested()
    } else if (event.modifiers === Qt.ShiftModifier) {
      if (event.key === Qt.Key_R && !view.pending) view.refreshRequested()
      else if (event.key === Qt.Key_A && row) view.approveScenariosRequested(row)
    } else if (event.modifiers === Qt.NoModifier) {
      if (event.key === Qt.Key_Q) view.closeRequested()
      else if (event.key === Qt.Key_J || event.key === Qt.Key_K)
        featuresList.moveSelection(event.key === Qt.Key_J ? 1 : -1)
      else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter || event.key === Qt.Key_O)
        featuresList.activateSelection()
      else if (event.key === Qt.Key_N) openNewFeatureForm()
      else if (event.key === Qt.Key_I) view.showImplemented = !view.showImplemented
      else if (event.key === Qt.Key_C && row) {
        view.featureSelected(row)
        focusChatInput()
      } else if (event.key === Qt.Key_V && row) view.reviewRequested(row)
      else if (event.key === Qt.Key_A && row) view.approveSpecRequested(row)
      else if (event.key === Qt.Key_G) return "g"
    }
    return ""
  }

  function submitChatMessage(feature) {
    const message = chatMessageFieldItem.text.trim()
    const target = feature || (view.selectedSlug !== "" ? { slug: view.selectedSlug } : null)
    if (message === "" || !target) return
    view.chatRequested(target, message)
    chatMessageFieldItem.text = ""
  }

  visible: view.open
  color: view.embedded ? "transparent" : Qt.rgba(0, 0, 0, 0.55)

  MouseArea {
    anchors.fill: parent
    enabled: !view.embedded
    onClicked: view.closeRequested()
  }

  Rectangle {
    anchors.centerIn: parent
    width: view.embedded ? parent.width : parent.width * 0.82
    height: view.embedded ? parent.height : parent.height * 0.82
    radius: 6
    color: view.surface
    border.width: view.embedded ? 0 : 1
    border.color: Qt.darker(view.foreground, 3)

    MouseArea {
      anchors.fill: parent
    }

    Column {
      anchors.fill: parent
      anchors.margins: 12
      spacing: 8

      Row {
        width: parent.width
        spacing: 8

        Text {
          width: parent.width - newFeatureButton.width - refreshFeaturesButton.width
            - closeFeaturesButton.width - parent.spacing * 3
            - (implementedToggle.visible ? implementedToggle.width + parent.spacing : 0)
          anchors.verticalCenter: parent.verticalCenter
          text: "Features"
          color: view.foreground
          font.family: view.fontFamily
          font.pixelSize: view.fontSize11
          font.bold: true
          elide: Text.ElideRight
        }

        ViewButton {
          id: implementedToggle

          objectName: "featureImplementedToggle"
          visible: view.implementedCount > 0
          label: view.showImplemented ? "Hide implemented"
            : "Show implemented (" + view.implementedCount + ")"
          onClicked: view.showImplemented = !view.showImplemented
        }

        ViewButton {
          id: newFeatureButton

          objectName: "featureNewButton"
          label: "New feature"
          onClicked: view.newFeatureOpen = !view.newFeatureOpen
        }

        ViewButton {
          id: refreshFeaturesButton

          label: "Refresh"
          enabled: !view.pending
          onClicked: view.refreshRequested()
        }

        ViewButton {
          id: closeFeaturesButton

          label: view.embedded ? "Back" : "Close"
          onClicked: view.closeRequested()
        }
      }

      Rectangle {
        id: newFeatureForm

        objectName: "featureNewForm"
        visible: view.newFeatureOpen
        width: parent.width
        height: newFeatureColumn.implicitHeight + 12
        radius: 4
        color: Qt.darker(view.surface, 1.15)
        border.width: 1
        border.color: Qt.darker(view.foreground, 3)

        Column {
          id: newFeatureColumn

          x: 6
          y: 6
          width: parent.width - 12
          spacing: 6

          Row {
            width: parent.width
            spacing: 8

            TextField {
              id: newSlugFieldItem

              objectName: "featureNewSlugField"
              width: parent.width * 0.45
              placeholderText: "slug (lowercase-with-hyphens)"
              font.family: view.fontFamily
              font.pixelSize: view.fontSize10
              Keys.onEscapePressed: view.cancelNewFeature()
            }

            TextField {
              id: newTitleFieldItem

              objectName: "featureNewTitleField"
              width: parent.width * 0.45
              placeholderText: "Title"
              font.family: view.fontFamily
              font.pixelSize: view.fontSize10
              Keys.onEscapePressed: view.cancelNewFeature()
              Keys.onReturnPressed: view.submitNewFeature()
            }
          }

          Text {
            objectName: "featureNewFormError"
            visible: view.newFeatureError !== ""
            width: parent.width
            text: view.newFeatureError
            wrapMode: Text.Wrap
            color: view.urgent
            font.family: view.fontFamily
            font.pixelSize: view.fontSize10
          }

          Row {
            spacing: 8

            ViewButton {
              objectName: "featureCreateButton"
              label: "Create"
              onClicked: view.submitNewFeature()
            }

            ViewButton {
              objectName: "featureCreateCancelButton"
              label: "Cancel"
              onClicked: view.cancelNewFeature()
            }
          }
        }
      }

      Text {
        objectName: "featuresError"
        visible: view.errorText !== ""
        width: parent.width
        text: view.errorText
        wrapMode: Text.Wrap
        color: view.urgent
        font.family: view.fontFamily
        font.pixelSize: view.fontSize10
      }

      Text {
        objectName: "featuresEmptyState"
        visible: view.shownRows.length === 0 && view.errorText === ""
        width: parent.width
        text: view.rows.length === 0 ? "No feature specs in docs/features/"
          : "All feature specs are implemented · i shows them"
        wrapMode: Text.Wrap
        color: view.mutedForeground
        font.family: view.fontFamily
        font.pixelSize: view.fontSize10
      }

      Row {
        id: bodyRow

        width: parent.width
        height: parent.height - y
        spacing: 8

        ListView {
          id: featuresList

          objectName: "featuresList"
          width: view.selectedSlug !== "" ? parent.width * 0.56 : parent.width
          height: parent.height
          clip: true
          boundsBehavior: Flickable.StopAtBounds
          model: view.shownRows
          currentIndex: -1
          onModelChanged: resetSelection()

          function moveSelection(direction) {
            const count = (model || []).length
            if (count === 0) return
            let index = currentIndex + direction
            if (index < 0) index = 0
            else if (index >= count) index = count - 1
            currentIndex = index
            positionViewAtIndex(index, ListView.Contain)
          }

          function resetSelection() {
            currentIndex = (model || []).length > 0 ? 0 : -1
          }

          function activateSelection() {
            const row = model && model[currentIndex]
            if (row) view.openRequested(row)
          }

          delegate: Rectangle {
            id: featureRow

            required property var modelData
            required property int index
            readonly property var spec: view.specs[featureRow.modelData.slug] || ({})
            readonly property bool activityRunning: !!view.activity
              && view.activity.slug === featureRow.modelData.slug && view.activity.status === "running"
            width: featuresList.width
            height: rowColumn.implicitHeight + 12
            radius: 4
            color: featuresList.currentIndex === index ? Qt.darker(view.accent, 2.8) : "transparent"

            MouseArea {
              anchors.fill: parent
              onClicked: {
                featuresList.currentIndex = featureRow.index
                view.featureSelected(featureRow.modelData)
              }
            }

            Column {
              id: rowColumn

              x: 6
              y: 6
              width: parent.width - 12
              spacing: 2

              Row {
                spacing: 8

                Text {
                  objectName: "featureTitle"
                  text: featureRow.modelData.title
                  color: view.foreground
                  font.family: view.fontFamily
                  font.bold: true
                  font.pixelSize: view.fontSize11
                }

                Text {
                  objectName: "featureSlug"
                  text: featureRow.modelData.slug
                  color: view.mutedForeground
                  font.family: view.fontFamily
                  font.pixelSize: view.fontSize10
                }

                Text {
                  objectName: "featureStatus"
                  text: featureRow.modelData.status
                  color: featureRow.modelData.valid ? view.success : view.urgent
                  font.family: view.fontFamily
                  font.bold: true
                  font.pixelSize: view.fontSize10
                }

                Text {
                  objectName: "featureSpecStatus"
                  readonly property string progress: Features.progressLabel(featureRow.spec)
                  text: progress !== "" ? progress : Features.specStatusLabel(featureRow.spec)
                  color: Features.isImplemented(featureRow.spec) ? view.success : view.mutedForeground
                  font.family: view.fontFamily
                  font.pixelSize: view.fontSize10
                }
              }

              Text {
                objectName: "featureReasons"
                visible: !featureRow.modelData.valid && featureRow.modelData.reasons.length > 0
                width: rowColumn.width
                text: featureRow.modelData.reasons.join("; ")
                wrapMode: Text.Wrap
                color: view.mutedForeground
                font.family: view.fontFamily
                font.pixelSize: view.fontSize10
              }

              Row {
                spacing: 6

                ViewButton {
                  objectName: "featureChatButton"
                  label: "Chat"
                  onClicked: {
                    featuresList.currentIndex = featureRow.index
                    view.featureSelected(featureRow.modelData)
                  }
                }

                ViewButton {
                  objectName: "featureReviewButton"
                  label: "Review"
                  enabled: featureRow.modelData.valid && !featureRow.activityRunning
                  onClicked: view.reviewRequested(featureRow.modelData)
                }

                ViewButton {
                  objectName: "featureApproveSpecButton"
                  label: "Approve spec"
                  enabled: Features.canApproveSpec(featureRow.spec)
                  onClicked: view.approveSpecRequested(featureRow.modelData)
                }

                ViewButton {
                  objectName: "featureApproveScenariosButton"
                  label: "Approve scenarios"
                  enabled: Features.canApproveScenarios(featureRow.spec)
                  onClicked: view.approveScenariosRequested(featureRow.modelData)
                }

                ViewButton {
                  id: openFeatureButton

                  objectName: "featureOpenButton"
                  label: "Open in nvim"
                  onClicked: view.openRequested(featureRow.modelData)
                }
              }
            }
          }
        }

        Column {
          id: detailPanel

          objectName: "featureDetailPanel"
          visible: view.selectedSlug !== ""
          width: parent.width - featuresList.width - bodyRow.spacing
          height: parent.height
          spacing: 6
          clip: true

          readonly property var selectedRow: {
            const matches = (view.rows || []).filter(function(r) { return r.slug === view.selectedSlug })
            return matches.length > 0 ? matches[0] : null
          }
          readonly property var selectedSpec: view.specs[view.selectedSlug] || ({})
          readonly property var verdict: Features.reviewSummary(detailPanel.selectedSpec.latest_review)
          readonly property var chatEntries: (view.detailState && view.detailState.state
            && Array.isArray(view.detailState.state.chat)) ? view.detailState.state.chat : []
          readonly property bool sending: !!view.activity
            && view.activity.slug === view.selectedSlug && view.activity.status === "running"

          Text {
            text: detailPanel.selectedRow ? detailPanel.selectedRow.title : view.selectedSlug
            color: view.foreground
            font.bold: true
            font.family: view.fontFamily
            font.pixelSize: view.fontSize11
          }

          Column {
            id: verdictBlock

            objectName: "featureReviewVerdict"
            visible: detailPanel.verdict !== null
            width: parent.width
            spacing: 2

            Text {
              objectName: "featureReviewVerdictLabel"
              text: detailPanel.verdict ? detailPanel.verdict.label : ""
              color: detailPanel.verdict && detailPanel.verdict.label === "approved" ? view.success : view.urgent
              font.bold: true
              font.family: view.fontFamily
              font.pixelSize: view.fontSize10
            }

            Text {
              objectName: "featureReviewVerdictSummary"
              text: detailPanel.verdict ? detailPanel.verdict.summary : ""
              wrapMode: Text.Wrap
              width: verdictBlock.width
              color: view.foreground
              font.family: view.fontFamily
              font.pixelSize: view.fontSize10
            }

            Repeater {
              model: detailPanel.verdict ? detailPanel.verdict.issues : []
              delegate: Text {
                required property string modelData
                objectName: "featureReviewVerdictIssue"
                text: "- " + modelData
                width: verdictBlock.width
                wrapMode: Text.Wrap
                color: view.urgent
                font.family: view.fontFamily
                font.pixelSize: view.fontSize10
              }
            }

            Repeater {
              model: detailPanel.verdict ? detailPanel.verdict.questions : []
              delegate: Text {
                required property string modelData
                objectName: "featureReviewVerdictQuestion"
                text: "? " + modelData
                width: verdictBlock.width
                wrapMode: Text.Wrap
                color: view.mutedForeground
                font.family: view.fontFamily
                font.pixelSize: view.fontSize10
              }
            }
          }

          Flickable {
            id: chatFlick

            objectName: "featureChatTranscript"
            width: parent.width
            height: Math.max(60, parent.height - y - chatInputRow.height - 8)
            clip: true
            contentWidth: width
            contentHeight: chatColumn.height
            boundsBehavior: Flickable.StopAtBounds

            Column {
              id: chatColumn

              width: chatFlick.width
              spacing: 4

              Repeater {
                model: detailPanel.chatEntries
                delegate: Text {
                  required property var modelData
                  objectName: "featureChatEntry"
                  width: chatColumn.width
                  wrapMode: Text.Wrap
                  text: (modelData.role === "user" ? "You: " : "Agent: ") + modelData.text
                  color: view.foreground
                  font.family: view.fontFamily
                  font.pixelSize: view.fontSize10
                }
              }
            }
          }

          Row {
            id: chatInputRow

            width: parent.width
            spacing: 6

            TextField {
              id: chatMessageFieldItem

              objectName: "featureChatInput"
              width: parent.width - chatSendButton.width - parent.spacing
              placeholderText: "Message the co-authoring agent"
              font.family: view.fontFamily
              font.pixelSize: view.fontSize10
              Keys.onReturnPressed: view.submitChatMessage(detailPanel.selectedRow)
            }

            ViewButton {
              id: chatSendButton

              objectName: "featureSendButton"
              label: "Send"
              enabled: chatMessageFieldItem.text.trim().length > 0 && !detailPanel.sending
              onClicked: view.submitChatMessage(detailPanel.selectedRow)
            }
          }
        }
      }
    }
  }

  component ViewButton: Rectangle {
    id: button

    property string label: ""

    signal clicked()

    implicitWidth: buttonText.implicitWidth + 18
    width: implicitWidth
    height: buttonText.implicitHeight + 10
    radius: 4
    color: view.surface
    border.width: 1
    border.color: Qt.darker(view.foreground, 3)
    opacity: button.enabled ? (buttonArea.containsMouse ? 0.85 : 1.0) : 0.45

    Text {
      id: buttonText

      anchors.centerIn: parent
      text: button.label
      color: view.foreground
      font.family: view.fontFamily
      font.pixelSize: view.fontSize11
    }

    MouseArea {
      id: buttonArea

      anchors.fill: parent
      hoverEnabled: true
      onClicked: button.clicked()
    }
  }
}
