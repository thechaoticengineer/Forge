pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls

Rectangle {
    id: architecture
    property var entries: []
    property string scope: ""
    property color foreground: "#dddddd"
    property color mutedForeground: "#aaaaaa"
    property color background: "#202020"
    property color accent: foreground
    property color urgent: "#ff6666"
    property string fontFamily: "monospace"
    property real fontSize: 12
    signal copyRequested(string original)
    signal leaveRequested()
    signal focusRevealed(var control)
    signal inspecting()
    implicitHeight: content.height + 24
    color: background
    radius: 6
    border.color: Qt.alpha(foreground, 0.15)

    function category(key) {
        if (key.indexOf("guidance") === 0) return "guidance"
        if (key.indexOf("risk/") === 0) return "risks"
        if (key.indexOf("decision/") === 0) return "decisions"
        return "overview"
    }

    component Fields: DetailFields {
        width: parent.width
        scope: architecture.scope
        foreground: architecture.foreground
        mutedForeground: architecture.mutedForeground
        background: architecture.background
        urgent: architecture.urgent
        fontFamily: architecture.fontFamily
        fontSize: architecture.fontSize
        onCopyRequested: original => architecture.copyRequested(original)
        onLeaveRequested: architecture.leaveRequested()
        onFocusRevealed: control => architecture.focusRevealed(control)
        onInspecting: architecture.inspecting()
    }

    component Section: Column {
        id: section
        required property string category
        required property string title
        property bool expanded: false
        property bool loaded: false
        readonly property var records: architecture.entries.filter(e => architecture.category(e.key) === category)
        readonly property int count: records.filter(e => category === "decisions" ? !e.detail
            : category === "guidance" ? e.detail : true).length
        width: parent.width
        visible: count > 0 || expanded
        spacing: 8
        Connections {
            target: architecture
            function onScopeChanged() { section.expanded = false; section.loaded = false }
        }
        Rectangle { width: parent.width; height: 1; color: Qt.alpha(architecture.foreground, 0.12) }
        Button {
            id: toggle
            objectName: section.category + "Toggle"
            width: parent.width
            implicitHeight: Math.max(32, label.implicitHeight + 12)
            padding: 6
            focusPolicy: Qt.StrongFocus
            Accessible.name: (section.expanded ? "Collapse " : "Expand ") + section.title + ", " + section.count
            contentItem: Text {
                id: label
                text: (section.expanded ? "▾  " : "▸  ") + section.title + "  ·  " + section.count
                textFormat: Text.PlainText
                color: section.expanded ? architecture.accent : architecture.foreground
                font.family: architecture.fontFamily
                font.pixelSize: architecture.fontSize
                font.bold: true
                wrapMode: Text.Wrap
            }
            background: Rectangle {
                radius: 4
                color: toggle.hovered || toggle.down ? Qt.alpha(architecture.foreground, 0.06) : "transparent"
                border.width: toggle.visualFocus ? 1 : 0
                border.color: architecture.accent
            }
            onClicked: {
                section.loaded = true
                section.expanded = !section.expanded
                architecture.inspecting()
            }
            onActiveFocusChanged: if (activeFocus && focusReason !== Qt.MouseFocusReason)
                architecture.focusRevealed(toggle)
            Keys.onEscapePressed: architecture.leaveRequested()
        }
        // Retain inspected editors through outer collapse and polling.
        Loader {
            width: parent.width
            active: section.loaded
            visible: section.expanded
            sourceComponent: Fields { entries: section.records }
        }
    }

    Column {
        id: content
        x: 12
        y: 12
        width: parent.width - 24
        spacing: 8
        Text {
            text: "Architecture"
            color: architecture.foreground
            font.family: architecture.fontFamily
            font.pixelSize: architecture.fontSize + 2
            font.bold: true
        }
        Fields {
            entries: architecture.entries.filter(e => architecture.category(e.key) === "overview")
        }
        Section { category: "guidance"; title: "Stage guidance" }
        Section { category: "risks"; title: "Open risks" }
        Section { category: "decisions"; title: "Decisions" }
    }
}
