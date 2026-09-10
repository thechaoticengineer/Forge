pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls
import "DetailText.js" as DetailText

FocusScope {
    id: prose
    property string label: ""
    property string originalText: ""
    property color foreground: "#dddddd"
    property color mutedForeground: "#aaaaaa"
    property color background: "#202020"
    property string fontFamily: "monospace"
    property real fontSize: 12
    readonly property bool stageProseField: true
    readonly property string selectedText: {
        const offsets = selectionMap.offsets
        // Source replacement can notify bindings before the editor is updated.
        // Never interpret an old document's selection against a new source.
        if (offsets.source !== originalText || editor.selectionStart === editor.selectionEnd)
            return ""
        const start = offsets.toOriginal[editor.selectionStart]
        const end = offsets.toOriginal[editor.selectionEnd]
        return start === undefined || end === undefined ? "" : originalText.slice(start, end)
    }
    readonly property bool hasSelection: selectedText !== ""
    signal copyRequested(string original)
    signal leaveRequested()
    signal focusRevealed(var control)
    signal inspecting()
    implicitHeight: heading.height + 4 + (originalText !== "" ? editor.height : emptyText.height)

    QtObject {
        id: selectionMap
        objectName: "_stageProseSelectionMap"
        property var offsets: ({ source: "", toOriginal: [0], toNative: [0] })

        // Native offsets and JS string offsets both count UTF-16 code units.
        // Only CRLF changes their count; its interior boundary has no native
        // counterpart. Other separators (including U+2028/U+2029) keep offsets
        // even when Qt renders them as LF. Keep both endpoint boundaries.
        function build(source) {
            const toOriginal = [0]
            const toNative = [0]
            let nativeOffset = 0
            for (let i = 0; i < source.length; ++i) {
                if (source.charCodeAt(i) === 13 && source.charCodeAt(i + 1) === 10) {
                    toNative[++i] = -1
                }
                toOriginal[++nativeOffset] = i + 1
                toNative[i + 1] = nativeOffset
            }
            return { source: source, toOriginal: toOriginal, toNative: toNative }
        }

        function replace() {
            // Clear first, including replacements with identical LF rendering.
            // Publishing the new map must not revive the preceding selection.
            editor.deselect()
            offsets = build(prose.originalText)
        }
    }
    onOriginalTextChanged: selectionMap.replace()

    function copySelection() {
        if (hasSelection) copyRequested(selectedText)
    }

    // Let the editor handle navigation first and retain ordinary Tab traversal.
    Keys.priority: Keys.AfterItem
    Keys.onPressed: event => {
        if (event.key === Qt.Key_Escape) {
            prose.leaveRequested()
            event.accepted = true
        } else if (event.key !== Qt.Key_Tab && event.key !== Qt.Key_Backtab) {
            event.accepted = true
        }
    }

    Text {
        id: heading
        objectName: "stageProseLabel"
        width: parent.width
        text: prose.label
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        color: prose.mutedForeground
        font.family: prose.fontFamily
        font.pixelSize: prose.fontSize
    }
    Text {
        id: emptyText
        objectName: "stageProseEmpty"
        y: heading.height + 4
        width: parent.width
        visible: prose.originalText === ""
        text: "(empty text)"
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        color: prose.mutedForeground
        font.family: prose.fontFamily
        font.pixelSize: prose.fontSize
    }
    TextArea {
        id: editor
        objectName: "stageProseText"
        y: heading.height + 4
        width: parent.width
        height: implicitHeight
        visible: prose.originalText !== ""
        text: prose.originalText
        textFormat: TextEdit.PlainText
        readOnly: true
        selectByMouse: true
        persistentSelection: true
        wrapMode: DetailText.wrapsWords(prose.originalText) ? TextEdit.Wrap : TextEdit.WrapAnywhere
        color: prose.foreground
        font.family: prose.fontFamily
        font.pixelSize: prose.fontSize
        background: Rectangle { color: prose.background }
        onActiveFocusChanged: {
            if (!activeFocus) return
            prose.inspecting()
            // Moving the viewport on a pointer press would break drag selection.
            if (focusReason !== Qt.MouseFocusReason && focusReason !== Qt.PopupFocusReason)
                prose.focusRevealed(editor)
        }
        onSelectedTextChanged: if (selectedText !== "") prose.inspecting()
        Keys.onPressed: event => {
            if (!(event.modifiers & Qt.ControlModifier)) return
            if (event.key === Qt.Key_C) {
                prose.copySelection()
                event.accepted = true
            } else if (event.key === Qt.Key_A) {
                editor.selectAll()
                event.accepted = true
            }
        }
    }
}
