pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import "CataloguePresentation.js" as CataloguePresentation
import "PanelDetails.js" as PanelDetails
import "UsageFormat.js" as UsageFormat

// Interface: the Settings tab. Engine settings, the model catalogue and the
// Claude quota enter as properties; clicking a row (or Space/Enter on a focused
// or selected row) leaves as settingActivated(key) and every button as
// actionRequested(id). Panel.qml maps both to the exact calls the old buttons
// made. Imports no shell modules, so qmltestrunner loads it on its own.
Flickable {
  id: view

  required property var engineState
  required property var catalogue
  required property var quota
  required property bool engineOnline
  required property bool busy
  required property bool catalogueOpen
  required property string reviewerLabel
  // { architect, reviewer }: the review cadence labels, e.g. "per plan".
  required property var cadenceLabels
  // Panel.qml passes its shared guards; the defaults are the same expressions.
  property bool updateEnabled: engineOnline && !busy
  property bool changeProjectEnabled: engineOnline
  property string detailScope: ""
  // Palette and font sizes; Panel.qml passes its shared theme object.
  property var theme: null
  property color foreground: theme ? theme.foreground : "#dddddd"
  property color mutedForeground: theme ? theme.mutedForeground : "#aaaaaa"
  property color background: theme ? theme.background : "#202020"
  property color surface: theme ? theme.surface : "#282828"
  property color accent: theme ? theme.accent : "#6699ff"
  property color urgent: theme ? theme.urgent : "#ff6666"
  property color success: theme ? theme.success : foreground
  property color working: theme ? theme.working : foreground
  property string fontFamily: theme ? theme.fontFamily : "monospace"
  property real fontSize10: theme ? theme.fontSize10 : 10
  property real fontSize11: theme ? theme.fontSize11 : 11
  property real fontSize12: theme ? theme.fontSize12 : 12
  property real spacing: 8
  property real horizontalPadding: 18
  property real verticalPadding: 10
  property real scrollBarSpace: 16

  // The keyboard-selected row (j/k, gg/G), -1 when none.
  property int selectedIndex: -1
  readonly property var rows: [plannerRow, architectRow, implementerRow, reviewerRow, routingRow,
    architectReviewRow, reviewerReviewRow, autoPushRow, autoApproveRow]
  readonly property var settings: engineState && engineState.settings ? engineState.settings : null

  signal settingActivated(string key)
  signal actionRequested(string id)
  signal copyRequested(string original)
  signal leaveRequested()
  signal detailRevealed(var control)
  signal detailInspected(var control)

  function selectRow(index) {
    selectedIndex = Math.max(0, Math.min(index, rows.length - 1))
    reveal(rows[selectedIndex])
  }

  function activateSelection() {
    if (selectedIndex < 0) selectRow(0)
    else rows[selectedIndex].activate()
  }

  // j/k, gg/G and Enter act on the rows while Settings is shown. Returns
  // "handled", "g" (a pending g prefix) or "" when the key is not for Settings.
  function handleKey(event, prefix) {
    if (event.key === Qt.Key_G && event.modifiers === Qt.ShiftModifier)
      selectRow(rows.length - 1)
    else if (event.modifiers !== Qt.NoModifier) return ""
    else if (event.key === Qt.Key_J || event.key === Qt.Key_K)
      selectRow(selectedIndex < 0 ? 0 : selectedIndex + (event.key === Qt.Key_J ? 1 : -1))
    else if (event.key === Qt.Key_G) {
      if (prefix !== "g") return "g"
      selectRow(0)
    } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter
               || event.key === Qt.Key_O || event.key === Qt.Key_Space)
      activateSelection()
    else return ""
    return "handled"
  }

  function reveal(item) {
    const top = item.mapToItem(contentItem, 0, 0).y
    if (top < contentY) contentY = top
    else if (top + item.height > contentY + height)
      contentY = Math.min(top + item.height - height, Math.max(0, contentHeight - height))
  }

  clip: true
  contentWidth: width
  contentHeight: settingsColumn.implicitHeight
  flickableDirection: Flickable.VerticalFlick
  boundsBehavior: Flickable.StopAtBounds
  ScrollBar.vertical: ScrollBar {
    policy: ScrollBar.AsNeeded
  }

  Column {
    id: settingsColumn

    width: view.width - view.scrollBarSpace
    spacing: view.spacing / 2

    SectionTitle { text: "AGENTS" }
    SettingRow {
      id: plannerRow
      key: "planner"
      label: "planner"
      value: view.settings ? view.settings.planner : "…"
    }
    SettingRow {
      id: architectRow
      key: "architect"
      label: "architect"
      value: view.settings ? (view.settings.architect || "codex") : "…"
    }
    SettingRow {
      id: implementerRow
      key: "implementer"
      label: "implementer"
      value: view.settings ? view.settings.implementer : "…"
    }
    SettingRow {
      id: reviewerRow
      key: "reviewer"
      label: "reviewer"
      value: view.reviewerLabel.replace(/^reviewer: /, "")
    }
    SettingRow {
      id: routingRow
      key: "automatic_routing"
      label: "automatic routing"
      value: view.settings && view.settings.automatic_routing !== false ? "yes" : "no"
    }

    SectionTitle { text: "REVIEW & RUN" }
    SettingRow {
      id: architectReviewRow
      key: "architect_review"
      label: "architect review"
      value: view.cadenceLabels ? view.cadenceLabels.architect : "…"
      available: view.engineOnline
    }
    SettingRow {
      id: reviewerReviewRow
      key: "reviewer_review"
      label: "reviewer review"
      value: view.cadenceLabels ? view.cadenceLabels.reviewer : "…"
      available: view.engineOnline
    }
    SettingRow {
      id: autoPushRow
      key: "auto_push"
      label: "push at end"
      value: view.settings && view.settings.auto_push ? "yes" : "no"
    }
    SettingRow {
      id: autoApproveRow
      key: "queue_auto_approve"
      label: "auto-approve"
      value: view.settings && view.settings.queue_auto_approve ? "yes" : "no"
      available: view.engineOnline
    }

    SectionTitle { text: "MODELS & LIMITS" }
    Text {
      objectName: "quotaSummary"
      width: parent.width
      text: UsageFormat.quotaSummary(view.quota ? Object.assign({}, view.quota, {error: ""}) : null)
      textFormat: Text.PlainText
      color: view.foreground
      wrapMode: Text.Wrap
      font.family: view.fontFamily
      font.pixelSize: view.fontSize12
    }
    SettingsDetail {
      visible: view.quota ? !!view.quota.error : false
      entries: [PanelDetails.field("text", "Quota error", view.quota ? view.quota.error || "" : "", true)]
    }
    Text {
      objectName: "catalogueSummary"
      width: parent.width
      text: view.catalogue ? "Model tiers: configured user policy · revision " + view.catalogue.policy_revision
        + " · " + view.catalogue.configured_count + " explicit entries" : "Model catalogue pending"
      color: view.mutedForeground
      wrapMode: Text.Wrap
      font.family: view.fontFamily
      font.pixelSize: view.fontSize11
    }
    SettingsDetail {
      objectName: "policyError"
      visible: !!view.catalogue && !!view.catalogue.policy_error
      entries: [PanelDetails.field("text", "Model policy error",
        view.catalogue && view.catalogue.policy_error ? view.catalogue.policy_error : "", true)]
    }
    Text {
      width: parent.width
      visible: !!view.engineState && !!view.engineState.model_selection
      text: visible ? "Last model: " + view.engineState.model_selection.provider + "/"
        + (view.engineState.model_selection.model || "provider default") + " · "
        + view.engineState.model_selection.availability : ""
      color: view.mutedForeground
      wrapMode: Text.Wrap
      font.family: view.fontFamily
      font.pixelSize: view.fontSize11
    }
    SettingsDetail {
      objectName: "providerDetails"
      entries: PanelDetails.providers(view.catalogue ? view.catalogue.providers : [])
    }
    Text {
      objectName: "metadataSummary"
      width: parent.width
      visible: !!view.catalogue
      text: CataloguePresentation.catalogueMetadataSummaryText(view.catalogue && view.catalogue.metadata
        ? Object.assign({}, view.catalogue.metadata, {store_error: ""}) : null)
      color: view.catalogue && view.catalogue.metadata
        && (view.catalogue.metadata.source_errors || view.catalogue.metadata.store_error)
        ? view.urgent : view.mutedForeground
      wrapMode: Text.Wrap
      font.family: view.fontFamily
      font.pixelSize: view.fontSize11
    }
    SettingsDetail {
      visible: !!view.catalogue && !!view.catalogue.metadata && !!view.catalogue.metadata.store_error
      entries: [PanelDetails.field("text", "Metadata store error", view.catalogue && view.catalogue.metadata
        ? view.catalogue.metadata.store_error || "" : "", true)]
    }
    Flow {
      width: parent.width
      spacing: view.spacing

      SettingsButton {
        objectName: "settingsAction_modelSettings"
        label: view.catalogueOpen ? "Close model settings" : "Model settings & options"
      }
      SettingsButton {
        objectName: "settingsAction_refreshModels"
        label: view.catalogue && view.catalogue.refreshing ? "Models: refreshing…" : "Refresh models"
        enabled: view.engineOnline && !(view.catalogue && view.catalogue.refreshing)
      }
      SettingsButton {
        objectName: "settingsAction_cancelRefresh"
        label: "Cancel refresh"
        visible: !!view.catalogue && view.catalogue.refreshing
      }
      SettingsButton {
        objectName: "settingsAction_refreshLimits"
        label: view.quota && view.quota.refreshing ? "Limits: checking…" : "Refresh Claude limits"
        enabled: view.engineOnline && !(view.quota && view.quota.refreshing)
      }
    }

    SectionTitle { text: "MAINTENANCE" }
    Flow {
      width: parent.width
      spacing: view.spacing

      SettingsButton {
        objectName: "settingsAction_update"
        label: "Update Forge"
        enabled: view.updateEnabled
      }
      SettingsButton {
        objectName: "settingsAction_changeProject"
        label: "Change project"
        enabled: view.changeProjectEnabled
      }
    }
  }

  component SectionTitle: Text {
    width: settingsColumn.width
    topPadding: view.spacing / 2
    textFormat: Text.PlainText
    color: view.mutedForeground
    font.family: view.fontFamily
    font.pixelSize: view.fontSize10
    font.letterSpacing: 1
  }

  // One setting as a label / value row. Rows take keyboard focus like the old
  // cadence buttons: Tab reaches the enabled ones, Space/Enter toggle, Escape
  // hands focus back to the panel.
  component SettingRow: Rectangle {
    id: row

    required property string key
    required property string label
    required property string value
    // Not Item.enabled: a row that is not available keeps its focus, so
    // Escape still hands focus back, but it toggles nothing.
    property bool available: true
    readonly property bool selected: view.selectedIndex >= 0 && view.rows[view.selectedIndex] === row

    function activate() {
      if (available) view.settingActivated(key)
    }

    objectName: "settingRow_" + key
    width: settingsColumn.width
    height: rowLabel.implicitHeight + view.verticalPadding
    radius: 4
    color: view.surface
    border.width: 1
    border.color: activeFocus || selected ? view.accent : view.surface
    opacity: available ? (rowArea.containsMouse ? 0.85 : 1.0) : 0.45
    activeFocusOnTab: available
    onActiveFocusChanged: if (activeFocus) view.detailRevealed(row)
    Keys.onPressed: event => {
      if (event.key === Qt.Key_Escape) view.leaveRequested()
      else if (event.key === Qt.Key_Space || event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
        if (available && !event.isAutoRepeat) activate()
      } else if (event.key === Qt.Key_Tab || event.key === Qt.Key_Backtab) return
      event.accepted = true
    }

    Text {
      id: rowLabel

      anchors.left: parent.left
      anchors.leftMargin: view.horizontalPadding / 2
      anchors.right: rowValue.left
      anchors.rightMargin: view.spacing
      anchors.verticalCenter: parent.verticalCenter
      text: row.label
      textFormat: Text.PlainText
      elide: Text.ElideRight
      color: view.foreground
      font.family: view.fontFamily
      font.pixelSize: view.fontSize12
    }
    Text {
      id: rowValue

      anchors.right: rowGlyph.left
      anchors.rightMargin: view.spacing
      anchors.verticalCenter: parent.verticalCenter
      width: Math.min(implicitWidth, row.width * 0.6)
      text: row.value
      textFormat: Text.PlainText
      elide: Text.ElideRight
      color: view.accent
      font.family: view.fontFamily
      font.pixelSize: view.fontSize12
    }
    Text {
      id: rowGlyph

      anchors.right: parent.right
      anchors.rightMargin: view.horizontalPadding / 2
      anchors.verticalCenter: parent.verticalCenter
      text: "⇄"
      color: view.mutedForeground
      font.family: view.fontFamily
      font.pixelSize: view.fontSize12
    }
    MouseArea {
      id: rowArea

      anchors.fill: parent
      hoverEnabled: true
      enabled: row.available
      onClicked: row.activate()
    }
  }

  component SettingsDetail: DetailFields {
    id: detail

    width: settingsColumn.width
    scope: view.detailScope
    foreground: view.mutedForeground
    mutedForeground: view.mutedForeground
    background: view.background
    urgent: view.urgent
    fontFamily: view.fontFamily
    fontSize: view.fontSize11
    onCopyRequested: original => view.copyRequested(original)
    onLeaveRequested: view.leaveRequested()
    onFocusRevealed: control => view.detailRevealed(control)
    onInspecting: view.detailInspected(detail)
  }

  // Emits actionRequested with the id after "settingsAction_".
  component SettingsButton: PanelViewButton {
    foreground: view.foreground
    background: view.background
    surface: view.surface
    accent: view.accent
    fontFamily: view.fontFamily
    fontSize: view.fontSize11
    horizontalPadding: view.horizontalPadding
    verticalPadding: view.verticalPadding
    onClicked: view.actionRequested(objectName.replace("settingsAction_", ""))
  }
}
