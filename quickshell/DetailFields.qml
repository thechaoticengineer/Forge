pragma ComponentBehavior: Bound
import QtQuick
import "PanelDetails.js" as PanelDetails

Column {
    id: fields
    property var entries: []
    property string scope: ""
    property color foreground: "#dddddd"
    property color mutedForeground: "#aaaaaa"
    property color background: "#202020"
    property color urgent: "#ff6666"
    property string fontFamily: "monospace"
    property real fontSize: 12
    signal copyRequested(string original)
    signal leaveRequested()
    signal focusRevealed(var control)
    signal inspecting()
    spacing: 4
    onEntriesChanged: PanelDetails.reconcile(records, entries)
    onScopeChanged: { records.clear(); PanelDetails.reconcile(records, entries) }
    ListModel { id: records }
    Repeater {
        model: records
        delegate: Column {
            id: row
            required property var model
            width: fields.width
            Text {
                objectName: "fieldStatus"
                visible: !row.model.detail
                width: parent.width
                text: row.model.status
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: row.model.error ? fields.urgent : fields.foreground
                font.family: fields.fontFamily
                font.pixelSize: fields.fontSize
            }
            CompactDetail {
                visible: row.model.detail
                width: parent.width
                originalText: row.model.text
                metadata: (row.model.error ? "! " : "") + row.model.label
                    + (row.model.retired ? " · previously shown" : "")
                expanded: row.model.expanded
                foreground: row.model.error ? fields.urgent : fields.foreground
                mutedForeground: fields.mutedForeground
                background: fields.background
                fontFamily: fields.fontFamily
                fontSize: fields.fontSize
                onExpansionRequested: value => PanelDetails.expand(records, row.model.key, value)
                onCopyRequested: original => fields.copyRequested(original)
                onLeaveRequested: fields.leaveRequested()
                onFocusRevealed: control => fields.focusRevealed(control)
                onInspecting: fields.inspecting()
            }
        }
    }
}
