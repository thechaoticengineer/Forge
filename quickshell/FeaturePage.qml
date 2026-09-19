pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import "Features.js" as Features

// Interface: the page of one feature, opened from the Features tab. A Back link
// and title, the sub-tabs README / Scenarios / Decisions / Milestones / Design,
// and below them a scrolling body: the status header (validation, spec status,
// progress, review verdict, milestone plans), the actions and the shown tab. The
// feature row, its spec entry, the GET /api/features/content response, the
// detail state, the running activity and a refused request enter as properties;
// navigation and the actions leave as signals, so the host keeps the request
// paths and the enablement rules of the Features tab (Features.featurePageActions
// shares them). Markdown files render through Text.MarkdownText; fenced Mermaid
// blocks and .mmd files are shown as labelled monospace source, not rendered.
// handleKey(event) acts on the keys h l 1-5 j k Escape and q and answers
// "handled" or "". Imports no shell modules, so qmltestrunner loads it on its own.
Item {
  id: page

  // The Features controller the page is hosted with (Panel.qml); its state feeds
  // the properties below. Unset, each property is set directly (tests do).
  property var controller: null
  property var feature: controller ? controller.featurePageFeature : null
  property var spec: controller ? controller.featurePageSpec : null
  // The GET /api/features/content response, or null while it loads.
  property var content: controller ? controller.featureContent : null
  property var detailState: controller ? controller.featureDetailState : null
  // The running feature activity ({slug, status}) or null.
  property var activity: controller ? controller.featureActivity : null
  // A refused request, {message}, shown inline; null clears it.
  property var refusal: controller ? controller.featurePageRefusal : null
  property bool pending: controller ? controller.featureContentPending : false
  property string error: controller ? controller.featureContentError : ""
  property string subTab: "README"
  // Shows the latest architect verdict's summary, issues and questions inline.
  property bool reviewDetailsOpen: false
  readonly property var verdict: Features.reviewSummary(hasReview ? spec.latest_review : null)
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
  property real scrollBarSpace: 16
  // The left and right margin of the page content; Panel.qml passes the shell margin.
  property real margin: 0

  property alias contentFlick: flick

  signal backRequested
  signal reviewRequested(var feature)
  signal approveSpecRequested(var feature)
  signal approveScenariosRequested(var feature)
  signal planMilestoneRequested(var feature, var milestone)
  signal chatRequested(var feature)
  signal reviewDetailsRequested(var feature)

  readonly property var files: content && content.files ? content.files : ({})
  readonly property var scenarios: content && Features.isList(content.scenarios) ? content.scenarios : []
  readonly property var designs: Features.designItems(content ? content.design : null)
  readonly property var headerState: detailState
    || (feature ? { valid: feature.valid, reasons: feature.reasons || [] } : null)
  readonly property var header: Features.featurePageHeader(spec, headerState)
  readonly property var actions: Features.featurePageActions(spec, activity)
  readonly property var planActions: actions.filter(function(action) { return action.id === "plan" })
  readonly property var blockedActions: actions.filter(function(action) { return !action.enabled && action.reason !== "" })
  readonly property bool hasReview: !!spec && !!spec.latest_review
  readonly property bool markdownTab: subTab === "README" || subTab === "Decisions" || subTab === "Milestones"
  readonly property var shownFile: markdownTab ? fileFor(subTab) : null
  readonly property real bodyWidth: Math.max(0, flick.width - scrollBarSpace)

  function fileFor(tab) {
    const name = tab === "README" ? "README.md" : tab === "Decisions" ? "decisions.md"
      : tab === "Milestones" ? "milestones.md" : "scenarios.md"
    return files[name] || null
  }

  function selectTab(tab) {
    if (Features.featurePageTabs().indexOf(tab) !== -1) page.subTab = tab
  }

  // l and h step between sub-tabs, 1-5 pick one, j and k scroll, Escape and q go back.
  function handleKey(event) {
    if (event.modifiers !== Qt.NoModifier) return ""
    const tabs = Features.featurePageTabs()
    if (event.key === Qt.Key_Escape || event.key === Qt.Key_Q) {
      page.backRequested()
    } else if (event.key === Qt.Key_L) {
      page.selectTab(Features.stepFeaturePageTab(page.subTab, 1))
    } else if (event.key === Qt.Key_H) {
      page.selectTab(Features.stepFeaturePageTab(page.subTab, -1))
    } else if (event.key >= Qt.Key_1 && event.key < Qt.Key_1 + tabs.length) {
      page.selectTab(tabs[event.key - Qt.Key_1])
    } else if (event.key === Qt.Key_J || event.key === Qt.Key_K) {
      const step = event.key === Qt.Key_J ? 60 : -60
      flick.contentY = Math.max(0, Math.min(Math.max(0, flick.contentHeight - flick.height), flick.contentY + step))
    } else {
      return ""
    }
    event.accepted = true
    return "handled"
  }

  // A different tab or feature starts at its top.
  onSubTabChanged: flick.contentY = 0
  onFeatureChanged: {
    flick.contentY = 0
    reviewDetailsOpen = false
  }

  function requestAction(action) {
    if (!action.enabled || !page.feature) return
    if (action.id === "review") page.reviewRequested(page.feature)
    else if (action.id === "approveSpec") page.approveSpecRequested(page.feature)
    else if (action.id === "approveScenarios") page.approveScenariosRequested(page.feature)
  }

  function actionById(id) {
    return actions.filter(function(action) { return action.id === id })[0] || ({ enabled: false, reason: "" })
  }

  function planMilestone(id) {
    const milestones = spec && Array.isArray(spec.milestones) ? spec.milestones : []
    const milestone = milestones.filter(function(m) { return m.id === id })[0]
    if (milestone && feature) page.planMilestoneRequested(feature, milestone)
  }

  // ------------------------------------------------------------- top bar
  Column {
    id: topBar

    x: page.margin
    width: parent.width - page.margin * 2
    spacing: page.spacing

    Item {
      id: breadcrumb

      objectName: "featurePageBreadcrumb"
      width: parent.width
      height: Math.max(backText.implicitHeight, titleText.implicitHeight) + 6

      Item {
        id: backLink

        objectName: "featurePageBack"
        width: backText.implicitWidth
        height: parent.height
        Accessible.name: "Back to features"

        Text {
          id: backText

          anchors.verticalCenter: parent.verticalCenter
          text: "‹ Back"
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
        id: titleText

        anchors.left: backLink.right
        anchors.leftMargin: page.spacing
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        text: "/ " + (page.feature ? page.feature.title || page.feature.slug : "")
        textFormat: Text.PlainText
        maximumLineCount: 1
        elide: Text.ElideRight
        color: page.foreground
        font.family: page.fontFamily
        font.pixelSize: page.fontSize12
        font.bold: true
      }
    }

    Row {
      id: subTabBar

      spacing: 2

      Repeater {
        model: Features.featurePageTabs()
        delegate: Item {
          id: subTabButton

          required property string modelData
          readonly property string label: modelData
          readonly property bool selected: page.subTab === modelData

          objectName: "featurePageTab_" + modelData
          width: subTabText.implicitWidth + page.spacing * 2
          height: subTabText.implicitHeight + page.spacing
          Accessible.name: modelData

          Rectangle {
            anchors.fill: parent
            radius: 4
            color: page.surface
            opacity: subTabButton.selected ? 1 : subTabArea.containsMouse ? 0.6 : 0
          }

          Text {
            id: subTabText

            anchors.centerIn: parent
            text: subTabButton.modelData
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
            onClicked: page.selectTab(subTabButton.modelData)
          }
        }
      }
    }
  }

  // -------------------------------------------------------------- body
  Flickable {
    id: flick

    objectName: "featurePageScroll"
    anchors.top: topBar.bottom
    anchors.topMargin: page.spacing
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.bottom: parent.bottom
    anchors.leftMargin: page.margin
    anchors.rightMargin: page.margin
    clip: true
    contentWidth: width
    contentHeight: body.implicitHeight
    flickableDirection: Flickable.VerticalFlick
    boundsBehavior: Flickable.StopAtBounds
    // A shorter tab cannot leave the view scrolled past its end.
    onContentHeightChanged: contentY = Math.max(0, Math.min(contentY, contentHeight - height))
    ScrollBar.vertical: ScrollBar {
      policy: ScrollBar.AsNeeded
    }

    Column {
      id: body

      width: page.bodyWidth
      spacing: page.spacing

      // ---- status header
      Column {
        id: statusHeader

        objectName: "featurePageHeader"
        visible: page.feature !== null
        width: parent.width
        spacing: 2

        Row {
          spacing: page.spacing

          Text {
            text: page.header.validation
            textFormat: Text.PlainText
            color: page.header.validation === "valid" ? page.success : page.urgent
            font.family: page.fontFamily
            font.pixelSize: page.fontSize11
            font.bold: true
          }

          Text {
            text: page.header.specStatus
            textFormat: Text.PlainText
            color: page.foreground
            font.family: page.fontFamily
            font.pixelSize: page.fontSize11
          }

          Text {
            text: page.header.progress
            textFormat: Text.PlainText
            color: page.mutedForeground
            font.family: page.fontFamily
            font.pixelSize: page.fontSize11
          }

          Text {
            visible: page.header.review !== null
            text: page.header.review
              ? "review: " + page.header.review.verdict + (page.header.review.current ? " · current" : " · not current") : ""
            textFormat: Text.PlainText
            color: page.header.review && page.header.review.verdict === "approved" && page.header.review.current
              ? page.success : page.working
            font.family: page.fontFamily
            font.pixelSize: page.fontSize11
          }
        }

        Repeater {
          model: page.header.reasons
          delegate: Text {
            required property string modelData

            width: statusHeader.width
            text: "· " + modelData
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: page.urgent
            font.family: page.fontFamily
            font.pixelSize: page.fontSize10
          }
        }

        Flow {
          visible: page.header.milestonePlans.length > 0
          width: parent.width
          spacing: page.spacing

          Repeater {
            model: page.header.milestonePlans
            delegate: Text {
              required property var modelData

              text: modelData.id + " " + modelData.status
              textFormat: Text.PlainText
              color: modelData.status === "failed" ? page.urgent
                : modelData.status === "completed" ? page.success : page.accent
              font.family: page.fontFamily
              font.pixelSize: page.fontSize10
            }
          }
        }
      }

      // ---- actions
      Flow {
        id: actionRow

        visible: page.feature !== null
        width: parent.width
        spacing: page.spacing

        PageButton {
          objectName: "featurePageReview"
          label: "Request review"
          enabled: page.actionById("review").enabled
          onClicked: page.requestAction(page.actionById("review"))
        }

        PageButton {
          objectName: "featurePageApproveSpec"
          label: "Approve spec"
          enabled: page.actionById("approveSpec").enabled
          onClicked: page.requestAction(page.actionById("approveSpec"))
        }

        PageButton {
          objectName: "featurePageApproveScenarios"
          label: "Approve scenarios"
          enabled: page.actionById("approveScenarios").enabled
          onClicked: page.requestAction(page.actionById("approveScenarios"))
        }

        Repeater {
          model: page.planActions
          delegate: PageButton {
            required property var modelData

            objectName: "featurePagePlan_" + modelData.milestone
            label: modelData.label
            enabled: modelData.enabled
            onClicked: page.planMilestone(modelData.milestone)
          }
        }

        PageButton {
          objectName: "featurePageChat"
          label: "Chat"
          onClicked: if (page.feature) page.chatRequested(page.feature)
        }

        PageButton {
          objectName: "featurePageReviewDetails"
          label: "Review details"
          enabled: page.hasReview
          onClicked: if (page.feature) {
            page.reviewDetailsOpen = !page.reviewDetailsOpen
            page.reviewDetailsRequested(page.feature)
          }
        }
      }

      Repeater {
        model: page.blockedActions
        delegate: Text {
          required property var modelData

          objectName: "featurePageActionReason"
          width: body.width
          text: modelData.label + ": " + modelData.reason
          textFormat: Text.PlainText
          wrapMode: Text.Wrap
          color: page.mutedForeground
          font.family: page.fontFamily
          font.pixelSize: page.fontSize10
        }
      }

      // ---- the latest verdict's details
      Column {
        objectName: "featurePageReviewDetailsBody"
        visible: page.reviewDetailsOpen && page.verdict !== null
        width: parent.width
        spacing: 2

        Text {
          width: parent.width
          text: page.verdict ? page.verdict.summary : ""
          textFormat: Text.PlainText
          wrapMode: Text.Wrap
          color: page.foreground
          font.family: page.fontFamily
          font.pixelSize: page.fontSize10
        }

        Repeater {
          model: page.verdict ? page.verdict.issues : []
          delegate: Text {
            required property string modelData

            objectName: "featurePageVerdictIssue"
            width: parent.width
            text: "- " + modelData
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: page.urgent
            font.family: page.fontFamily
            font.pixelSize: page.fontSize10
          }
        }

        Repeater {
          model: page.verdict ? page.verdict.questions : []
          delegate: Text {
            required property string modelData

            objectName: "featurePageVerdictQuestion"
            width: parent.width
            text: "? " + modelData
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: page.mutedForeground
            font.family: page.fontFamily
            font.pixelSize: page.fontSize10
          }
        }
      }

      Text {
        objectName: "featurePageRefusal"
        visible: page.refusal !== null && page.refusal !== undefined && !!page.refusal.message
        width: parent.width
        text: page.refusal && page.refusal.message ? page.refusal.message : ""
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        color: page.urgent
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
      }

      // ---- loading and failure of the content
      Text {
        objectName: "featurePageStatus"
        visible: page.content === null && page.error === ""
        width: parent.width
        text: page.pending ? "Loading…" : "No content to show."
        textFormat: Text.PlainText
        color: page.mutedForeground
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
      }

      Text {
        objectName: "featurePageError"
        visible: page.error !== ""
        width: parent.width
        text: page.error
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        color: page.urgent
        font.family: page.fontFamily
        font.pixelSize: page.fontSize11
      }

      // ---- README, Decisions and Milestones: Markdown
      Column {
        id: markdownTab

        objectName: "featurePageMarkdownTab"
        visible: page.content !== null && page.markdownTab
        width: parent.width
        spacing: page.spacing

        Text {
          objectName: "featureFileError"
          visible: page.shownFile === null || page.shownFile.error !== null && page.shownFile.error !== undefined
          width: parent.width
          text: page.shownFile === null ? "This file was not provided." : page.shownFile.error || ""
          textFormat: Text.PlainText
          wrapMode: Text.Wrap
          color: page.urgent
          font.family: page.fontFamily
          font.pixelSize: page.fontSize11
        }

        Repeater {
          model: page.shownFile && typeof page.shownFile.text === "string"
            ? Features.markdownSegments(page.shownFile.text) : []
          delegate: Column {
            id: segment

            required property var modelData

            width: markdownTab.width
            spacing: 4

            Text {
              objectName: "featureMarkdown"
              visible: segment.modelData.kind === "markdown"
              width: parent.width
              text: segment.modelData.kind === "markdown" ? segment.modelData.text : ""
              textFormat: Text.MarkdownText
              wrapMode: Text.Wrap
              color: page.foreground
              linkColor: page.accent
              font.family: page.fontFamily
              font.pixelSize: page.fontSize11
              onLinkActivated: link => Qt.openUrlExternally(link)
            }

            MermaidSource {
              visible: segment.modelData.kind === "mermaid"
              width: parent.width
              source: segment.modelData.kind === "mermaid" ? segment.modelData.text : ""
            }
          }
        }
      }

      // ---- Scenarios
      Column {
        id: scenariosTab

        objectName: "featurePageScenariosTab"
        visible: page.content !== null && page.subTab === "Scenarios"
        width: parent.width
        spacing: page.spacing

        Text {
          objectName: "featureFileError"
          visible: page.scenarios.length === 0 && page.fileFor("Scenarios") !== null
            && !!page.fileFor("Scenarios").error
          width: parent.width
          text: page.fileFor("Scenarios") && page.fileFor("Scenarios").error ? page.fileFor("Scenarios").error : ""
          textFormat: Text.PlainText
          wrapMode: Text.Wrap
          color: page.urgent
          font.family: page.fontFamily
          font.pixelSize: page.fontSize11
        }

        Text {
          objectName: "featureNoScenarios"
          visible: page.scenarios.length === 0
          width: parent.width
          text: "No scenarios found."
          textFormat: Text.PlainText
          color: page.mutedForeground
          font.family: page.fontFamily
          font.pixelSize: page.fontSize11
        }

        Repeater {
          model: page.scenarios
          delegate: Rectangle {
            id: scenarioRow

            required property var modelData
            readonly property var result: Features.scenarioResultView(modelData)

            objectName: "featureScenario"
            width: scenariosTab.width
            height: scenarioColumn.implicitHeight + 12
            radius: 4
            color: page.surface

            Column {
              id: scenarioColumn

              x: 6
              y: 6
              width: parent.width - 12
              spacing: 2

              Row {
                spacing: page.spacing

                Text {
                  text: scenarioRow.modelData.id
                  textFormat: Text.PlainText
                  color: page.accent
                  font.family: page.fontFamily
                  font.pixelSize: page.fontSize11
                  font.bold: true
                }

                Text {
                  text: scenarioRow.result.label
                  objectName: "featureScenarioResult"
                  textFormat: Text.PlainText
                  color: scenarioRow.result.label === "passed" ? page.success
                    : scenarioRow.result.label === "failed" ? page.urgent : page.mutedForeground
                  font.family: page.fontFamily
                  font.pixelSize: page.fontSize10
                  font.bold: true
                }

                Text {
                  objectName: "featureScenarioOutOfDate"
                  visible: scenarioRow.result.outOfDate
                  text: "possibly out of date"
                  textFormat: Text.PlainText
                  color: page.working
                  font.family: page.fontFamily
                  font.pixelSize: page.fontSize10
                }
              }

              Text {
                width: parent.width
                text: scenarioRow.modelData.title || ""
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: page.foreground
                font.family: page.fontFamily
                font.pixelSize: page.fontSize11
                font.bold: true
              }

              Text {
                width: parent.width
                text: "Given: " + (scenarioRow.modelData.given || "")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: page.foreground
                font.family: page.fontFamily
                font.pixelSize: page.fontSize10
              }

              Text {
                width: parent.width
                text: "When: " + (scenarioRow.modelData.when || "")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: page.foreground
                font.family: page.fontFamily
                font.pixelSize: page.fontSize10
              }

              Text {
                width: parent.width
                text: "Then: " + (scenarioRow.modelData.then || "")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: page.foreground
                font.family: page.fontFamily
                font.pixelSize: page.fontSize10
              }

              Text {
                width: parent.width
                text: scenarioRow.modelData.milestone ? "Covered by " + scenarioRow.modelData.milestone
                  : "Not covered by a milestone"
                textFormat: Text.PlainText
                color: page.mutedForeground
                font.family: page.fontFamily
                font.pixelSize: page.fontSize10
              }

              Text {
                objectName: "featureScenarioEvidence"
                visible: scenarioRow.result.evidence !== ""
                width: parent.width
                text: scenarioRow.result.evidence
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: page.urgent
                font.family: page.fontFamily
                font.pixelSize: page.fontSize10
              }

              Text {
                objectName: "featureScenarioMeta"
                visible: scenarioRow.result.when !== "" || scenarioRow.result.plan !== ""
                width: parent.width
                text: [scenarioRow.result.when, scenarioRow.result.plan].filter(function(part) { return part !== "" }).join(" · ")
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: page.mutedForeground
                font.family: page.fontFamily
                font.pixelSize: page.fontSize10
              }
            }
          }
        }
      }

      // ---- Design
      Column {
        id: designTab

        objectName: "featurePageDesignTab"
        visible: page.content !== null && page.subTab === "Design"
        width: parent.width
        spacing: page.spacing

        Text {
          objectName: "featureNoDesign"
          visible: page.designs.length === 0
          width: parent.width
          text: "No design files."
          textFormat: Text.PlainText
          color: page.mutedForeground
          font.family: page.fontFamily
          font.pixelSize: page.fontSize11
        }

        Repeater {
          model: page.designs
          delegate: Column {
            id: designItem

            required property var modelData

            width: designTab.width
            spacing: 4

            Text {
              objectName: "featureDesignLabel"
              width: parent.width
              text: designItem.modelData.label
              textFormat: Text.PlainText
              wrapMode: Text.WrapAnywhere
              color: page.foreground
              font.family: page.fontFamily
              font.pixelSize: page.fontSize11
              font.bold: true
            }

            Text {
              objectName: "featureDesignNote"
              visible: designItem.modelData.note !== ""
              width: parent.width
              text: designItem.modelData.note
              textFormat: Text.PlainText
              color: page.mutedForeground
              font.family: page.fontFamily
              font.pixelSize: page.fontSize10
            }

            Image {
              id: designImage

              objectName: "featureDesignImage"
              readonly property real ratio: implicitWidth > 0 ? implicitHeight / implicitWidth : 0

              visible: designItem.modelData.image !== null
              // Only the shown tab loads its images.
              source: designTab.visible && designItem.modelData.image !== null
                ? Features.fileUrl(designItem.modelData.image) : ""
              asynchronous: true
              fillMode: Image.PreserveAspectFit
              // Never wider than the page; the natural size keeps small exports crisp.
              width: implicitWidth > 0 ? Math.min(designItem.width, implicitWidth) : designItem.width
              height: ratio > 0 ? width * ratio : 0
            }

            Text {
              objectName: "featureDesignImageError"
              visible: designImage.visible && designImage.status === Image.Error
              width: parent.width
              text: "The exported image could not be loaded."
              textFormat: Text.PlainText
              color: page.urgent
              font.family: page.fontFamily
              font.pixelSize: page.fontSize10
            }

            Text {
              objectName: "featureDesignError"
              visible: designItem.modelData.error !== ""
              width: parent.width
              text: designItem.modelData.error
              textFormat: Text.PlainText
              wrapMode: Text.Wrap
              color: page.urgent
              font.family: page.fontFamily
              font.pixelSize: page.fontSize10
            }

            MermaidSource {
              visible: designItem.modelData.kind === "mermaid" && designItem.modelData.error === ""
              width: parent.width
              source: designItem.modelData.text
            }
          }
        }
      }
    }
  }

  // A Mermaid diagram is not rendered: its source is shown, labelled, as plain monospace text.
  component MermaidSource: Column {
    id: mermaid

    property string source: ""

    spacing: 2

    Text {
      text: "Mermaid diagram"
      textFormat: Text.PlainText
      color: page.mutedForeground
      font.family: page.fontFamily
      font.pixelSize: page.fontSize10
    }

    Rectangle {
      width: parent.width
      height: mermaidText.implicitHeight + 12
      radius: 4
      color: page.surface

      Text {
        id: mermaidText

        objectName: "featureMermaid"
        x: 6
        y: 6
        width: parent.width - 12
        text: mermaid.source
        textFormat: Text.PlainText
        wrapMode: Text.WrapAnywhere
        color: page.foreground
        font.family: "monospace"
        font.pixelSize: page.fontSize10
      }
    }
  }

  component PageButton: Rectangle {
    id: button

    property string label: ""

    signal clicked()

    implicitWidth: buttonText.implicitWidth + 18
    width: implicitWidth
    height: buttonText.implicitHeight + 10
    radius: 4
    color: page.surface
    border.width: 1
    border.color: Qt.darker(page.foreground, 3)
    opacity: button.enabled ? (buttonArea.containsMouse ? 0.85 : 1.0) : 0.45

    Text {
      id: buttonText

      anchors.centerIn: parent
      text: button.label
      textFormat: Text.PlainText
      color: page.foreground
      font.family: page.fontFamily
      font.pixelSize: page.fontSize11
    }

    MouseArea {
      id: buttonArea

      anchors.fill: parent
      hoverEnabled: true
      onClicked: button.clicked()
    }
  }
}
