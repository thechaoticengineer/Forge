pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import qs.Commons
import "PanelDetails.js" as PanelDetails
import "CataloguePresentation.js" as CataloguePresentation

// Interface: catalogue/editor state and theme enter as properties; all user
// actions leave as signals. editor is exposed for root-owned AI/API coordination.
Rectangle {
  id: view

  required property bool open
  required property bool engineOnline
  required property bool engineBusy
  required property bool aiPending
  required property string aiReady
  required property string aiUndo
  required property string aiMessage
  required property var catalogue
  required property var details
  required property string draft
  required property string detailScope
  required property color foreground
  required property color mutedForeground
  required property color background
  required property color surface
  required property color accent
  required property color urgent
  required property string fontFamily
  required property real fontSize10
  required property real fontSize11
  property alias editor: catalogueEditor

  signal closeRequested
  signal suggestRequested
  signal applyRequested
  signal undoRequested
  signal reloadRequested
  signal saveRequested
  signal metadataRefreshRequested
  signal leaveRequested
  signal detailRevealed(var control)
  signal detailInspected(var control)

  visible: open
  color: Qt.rgba(0, 0, 0, 0.55)

  MouseArea {
    anchors.fill: parent
    onClicked: view.closeRequested()
  }

  Rectangle {
    anchors.centerIn: parent
    width: parent.width * 0.9
    height: parent.height * 0.85
    color: view.surface
    radius: 6

    MouseArea {
      anchors.fill: parent
    }

    Column {
      anchors.fill: parent
      anchors.margins: Style.space(12)
      spacing: Style.space(8)

      ViewButton {
        label: "Close model settings"
        onClicked: view.closeRequested()
      }

      Flickable {
        width: parent.width
        height: parent.height - y
        contentHeight: catalogueSettings.height
        clip: true

        Column {
          id: catalogueSettings

          width: parent.width
          spacing: Style.space(6)

          Flow {
            width: parent.width
            spacing: Style.space(8)

            ViewButton {
              label: view.aiPending ? "Updating shortlist…" : "Update shortlist with AI"
              enabled: view.engineOnline && !view.engineBusy && !view.aiPending && !(view.catalogue && view.catalogue.refreshing)
              onClicked: view.suggestRequested()
            }

            ViewButton {
              label: "Apply AI tiers"
              visible: view.aiReady !== ""
              enabled: !view.aiPending
              onClicked: view.applyRequested()
            }

            ViewButton {
              label: "Undo AI tiers"
              visible: view.aiUndo !== ""
              enabled: !view.aiPending
              onClicked: view.undoRequested()
            }

            ViewButton {
              label: "Reload saved policy"
              enabled: view.engineOnline && !view.aiPending
              onClicked: view.reloadRequested()
            }
          }

          ViewDetail {
            width: parent.width
            metadata: "Automatic model shortlist"
            originalText: "AI checks current official model pages and selects up to 4 distinct models per provider, covering simple, everyday and complex work. New model families can replace older ones when discovered locally. Save the draft to use the shortlist."
          }

          ViewDetail {
            width: parent.width
            visible: view.aiMessage !== ""
            metadata: "AI model tiers"
            originalText: view.aiMessage
          }

          ViewDetail {
            width: parent.width
            originalText: "Explicit model policy (JSON). Tiers: basic, standard, strong. Lower relative_cost_preference is preferred; it is not a price. Increment policy_revision before saving. Explicit configured native efforts may be used when availability is unverified and the adapter supports them; use provider_default otherwise. Plans select a capability tier. At implementation start, Forge selects a model within the currently selected implementer provider. Changing the provider after planning needs no replan. A nonempty implementer_model pins a model. Stage constraints take precedence. Periodic refresh intervals live here too: discovery_refresh_minutes, metadata_refresh_minutes, metadata_ttl_hours, metadata_research."
            metadata: "Model policy help"
          }

          Rectangle {
            width: parent.width
            height: Style.space(180)
            color: view.surface
            border.width: 1
            border.color: catalogueEditor.activeFocus ? view.accent : view.mutedForeground

            Flickable {
              anchors.fill: parent
              anchors.margins: Style.space(6)
              contentHeight: catalogueEditor.height
              clip: true

              TextEdit {
                id: catalogueEditor

                width: parent.width
                height: Math.max(contentHeight, parent.height)
                text: view.draft
                textFormat: TextEdit.PlainText
                wrapMode: TextEdit.Wrap
                selectByMouse: true
                color: view.foreground
                font.family: view.fontFamily
                font.pixelSize: view.fontSize11
                Keys.onEscapePressed: view.leaveRequested()
              }
            }
          }

          ViewButton {
            label: "Save model policy"
            enabled: view.engineOnline && !view.aiPending && !(view.catalogue && view.catalogue.refreshing)
            onClicked: view.saveRequested()
          }

          ViewFields {
            objectName: "catalogueOptions"
            width: parent.width
            entries: PanelDetails.options(view.details ? view.details.options : [], CataloguePresentation.catalogueOptionText)
          }

          Row {
            spacing: Style.space(8)

            ViewButton {
              label: "Refresh official metadata"
              enabled: view.engineOnline
              onClicked: view.metadataRefreshRequested()
            }
          }

          Text {
            width: parent.width
            visible: !!view.details && !!view.details.metadata
            text: "Official metadata (allowlisted sources only; never grants availability):"
            wrapMode: Text.Wrap
            color: view.mutedForeground
            font.family: view.fontFamily
            font.pixelSize: view.fontSize11
          }

          ViewFields {
            objectName: "catalogueMetadata"
            width: parent.width
            entries: PanelDetails.metadata(view.details && view.details.metadata ? view.details.metadata.records : [], CataloguePresentation.catalogueStamp)
          }

          ViewFields {
            objectName: "catalogueSources"
            width: parent.width
            entries: PanelDetails.sources(view.details && view.details.metadata ? view.details.metadata.sources : [], CataloguePresentation.catalogueStamp)
          }

          Repeater {
            model: view.details && view.details.metadata ? view.details.metadata.negative : []

            delegate: Text {
              required property var modelData
              width: parent.width
              text: modelData.provider + "/" + modelData.model + " · not in official source · attempts " + modelData.attempts + " · retry after " + CataloguePresentation.catalogueStamp(modelData.next_attempt_unix)
              wrapMode: Text.Wrap
              color: view.mutedForeground
              font.family: view.fontFamily
              font.pixelSize: view.fontSize10
            }
          }

          Text {
            width: parent.width
            text: "Full discovered IDs, aliases, efforts, capabilities and changes: GET /api/models"
            color: view.mutedForeground
            wrapMode: Text.Wrap
            font.family: view.fontFamily
            font.pixelSize: view.fontSize10
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
    fontSize: view.fontSize11
  }

  component ViewFields: DetailFields {
    id: fields

    scope: view.detailScope
    foreground: view.mutedForeground
    mutedForeground: view.mutedForeground
    background: view.background
    urgent: view.urgent
    fontFamily: view.fontFamily
    fontSize: view.fontSize11
    onCopyRequested: original => Quickshell.clipboardText = original
    onLeaveRequested: view.leaveRequested()
    onFocusRevealed: control => view.detailRevealed(control)
    onInspecting: view.detailInspected(fields)
  }

  component ViewDetail: ViewFields {
    property string originalText: ""
    property string metadata: ""
    property bool error: false
    entries: [PanelDetails.field("text", metadata, originalText, error)]
  }
}
