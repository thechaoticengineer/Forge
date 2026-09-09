pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import "DetailView.js" as DetailView

Flickable {
    id: view
    property alias model: rows.model
    property bool history: false
    property double now: Date.now() / 1000
    property color foreground: "#dddddd"
    property color mutedForeground: "#aaaaaa"
    property color background: "#202020"
    property color urgent: "#ff6666"
    property color accent: "#6699ff"
    property color success: "#66bb88"
    property color working: "#ddbb66"
    property string fontFamily: "monospace"
    property real fontSize: 12
    property bool followTail: true
    property real readingY: 0
    property var readingAnchor: null
    property bool restoring: false
    property bool updating: false
    signal copyRequested(string original)
    signal leaveRequested()
    clip: true
    contentWidth: width
    contentHeight: entries.height
    flickableDirection: Flickable.VerticalFlick
    boundsBehavior: Flickable.StopAtBounds

    function captureReading() {
        if (restoring || updating) return
        readingY = contentY
        for (let i = 0; i < rows.count; ++i) {
            const row = rows.itemAt(i)
            if (row && row.visible && row.y + row.height > contentY) {
                readingAnchor = { key: row.model.entryKey, offset: contentY - row.y }
                return
            }
        }
    }
    function beginUpdate() {
        // User navigation/inspection records the anchor. Keep it when a filter
        // temporarily hides that entry, including across polls while hidden.
        if (!followTail && !readingAnchor) captureReading()
        updating = true
    }
    function endUpdate() {
        Qt.callLater(settle)
    }
    function settle() {
        entries.forceLayout()
        updating = false
        if (moving) return
        if (followTail) { scrollToTail(); return }
        restoring = true
        let target = readingY
        if (readingAnchor) {
            for (let i = 0; i < rows.count; ++i) {
                const row = rows.itemAt(i)
                if (row && row.visible && row.model.entryKey === readingAnchor.key) {
                    target = row.y + readingAnchor.offset
                    break
                }
            }
        }
        contentY = Math.max(0, Math.min(target, Math.max(0, contentHeight - height)))
        restoring = false
        if (!readingAnchor) captureReading()
    }
    function inspect() {
        followTail = false
        captureReading()
    }
    // Keyboard traversal can focus a control outside the viewport, leaving the
    // user expanding or copying a row they cannot see. Scroll the smallest
    // amount that shows it, then re-anchor so polling keeps it in place. A
    // control taller than the viewport only has to reach its top edge.
    function reveal(control) {
        if (!control || !control.visible || height <= 0) return
        const top = control.mapToItem(entries, 0, 0).y
        const bottom = top + control.height
        const needed = Math.min(control.height, height)
        if (Math.min(bottom, contentY + height) - Math.max(top, contentY) >= needed) return
        const target = top < contentY ? top : Math.min(top, bottom - height)
        contentY = Math.max(0, Math.min(target, Math.max(0, contentHeight - height)))
        captureReading()
    }
    function resetView() {
        readingAnchor = null
        readingY = 0
        followTail = true
        contentY = 0
        endUpdate()
    }
    function hasSelection() {
        for (let i = 0; i < rows.count; ++i) {
            const row = rows.itemAt(i)
            if (row && row.hasSelection) return true
        }
        return false
    }
    function scrollToTail() {
        if (hasSelection()) { followTail = false; return }
        if (followTail && !moving && !updating) positionViewAtEnd()
    }
    function positionViewAtBeginning() { contentY = 0 }
    function positionViewAtEnd() { contentY = Math.max(0, contentHeight - height) }
    onContentYChanged: {
        if (moving) {
            followTail = atYEnd && !hasSelection()
            captureReading()
        }
    }
    onMovementEnded: {
        followTail = atYEnd && !hasSelection()
        captureReading()
    }
    onContentHeightChanged: Qt.callLater(settle)
    onHeightChanged: Qt.callLater(settle)
    onWidthChanged: Qt.callLater(settle)
    onVisibleChanged: if (visible) Qt.callLater(settle)
    ScrollBar.vertical: ScrollBar {
        onPressedChanged: {
            if (pressed) view.inspect()
            else view.captureReading()
        }
    }

    // A retained Repeater deliberately keeps editors alive outside the viewport
    // and across filters, so appends cannot destroy an active selection. Models
    // are reset on project/live-session changes, never on ordinary state polls.
    Column {
        id: entries
        width: view.width
        Repeater {
            id: rows
            delegate: Column {
                id: row
                required property var model
                readonly property bool hasSelection: detail.hasSelection
                width: entries.width
                visible: model.shown
                Text {
                    objectName: "detailSeparator"
                    visible: row.model.separator !== ""
                    width: parent.width
                    topPadding: 6
                    bottomPadding: 4
                    text: "— goal: " + row.model.separator
                    textFormat: Text.PlainText
                    wrapMode: Text.NoWrap
                    elide: Text.ElideRight
                    color: view.accent
                    font.family: view.fontFamily
                    font.pixelSize: view.fontSize
                }
                CompactDetail {
                    id: detail
                    width: row.width
                    originalText: row.model.originalText
                    expanded: row.model.expanded
                    metadata: {
                        const kind = "[" + row.model.kind + "]"
                        if (!view.history)
                            return kind + (row.model.stream === "stderr" ? " [stderr]" : "")
                                + (row.model.label ? " " + row.model.label : "")
                        let prefix = ""
                        if (row.model.unix > 0) {
                            const date = new Date(row.model.unix * 1000), today = new Date(view.now * 1000)
                            if (date.toDateString() !== today.toDateString())
                                prefix = (date.getMonth() + 1) + "-" + date.getDate() + " "
                        }
                        return prefix + row.model.time + " " + kind
                    }
                    foreground: row.model.kind === "error" || row.model.stream === "stderr" ? view.urgent
                        : row.model.kind === "git" ? view.success
                        : row.model.kind === "review" || row.model.kind === "check" ? view.working
                        : view.foreground
                    mutedForeground: view.mutedForeground
                    background: view.background
                    fontFamily: view.fontFamily
                    fontSize: view.fontSize
                    onInspecting: view.inspect()
                    onFocusRevealed: control => {
                        view.inspect()
                        view.reveal(control)
                    }
                    onExpansionRequested: value => {
                        view.beginUpdate()
                        DetailView.expand(view.model, row.model.entryKey, value)
                        view.endUpdate()
                    }
                    onCopyRequested: original => view.copyRequested(original)
                    onLeaveRequested: view.leaveRequested()
                }
            }
        }
    }
}
