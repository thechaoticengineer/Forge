import QtQuick
import QtQuick.Controls
import QtTest
import "../../quickshell"

Item {
    id: fixture
    width: 640
    height: 560
    property int parentKeys: 0
    property bool watchReplacement: false
    property var replacementSelections: []
    Keys.onPressed: event => { parentKeys++; event.accepted = true }

    TextField { id: before; width: 100; height: 30 }
    StageProse {
        id: prose
        y: 40
        width: 520
        label: "Instructions <b>plain</b>"
        onSelectedTextChanged: {
            if (fixture.watchReplacement)
                fixture.replacementSelections.push(selectedText)
        }
    }
    TextField { id: after; y: 510; width: 100; height: 30 }
    SignalSpy { id: copies; target: prose; signalName: "copyRequested" }
    SignalSpy { id: leaves; target: prose; signalName: "leaveRequested" }
    SignalSpy { id: reveals; target: prose; signalName: "focusRevealed" }
    SignalSpy { id: inspections; target: prose; signalName: "inspecting" }
    Component { id: initialProse; StageProse { width: 300; originalText: "initial\r\n🙂\rtext" } }

    TestCase {
        name: "StageProse"
        when: windowShown
        readonly property string original: "\r\n \t\r  <b>plain 界🙂</b> "
            + "long ".repeat(160) + "\r\nlast  \t\n"
        function editor() { return findChild(prose, "stageProseText") }
        function init() {
            fixture.watchReplacement = false
            fixture.replacementSelections = []
            prose.originalText = ""
            prose.width = 520
            before.forceActiveFocus()
            copies.clear()
            leaves.clear()
            reveals.clear()
            inspections.clear()
            fixture.parentKeys = 0
        }
        function selectAndCopy(start, end, expected) {
            const body = editor()
            body.forceActiveFocus()
            body.select(start, end)
            compare(body.selectionStart, Math.min(start, end))
            compare(body.selectionEnd, Math.max(start, end))
            compare(prose.selectedText, expected)
            compare(prose.hasSelection, expected !== "")
            copies.clear()
            keyClick(Qt.Key_C, Qt.ControlModifier)
            compare(copies.count, expected === "" ? 0 : 1)
            if (expected !== "") compare(copies.signalArguments[0][0], expected)
        }
        function test_native_presentation_and_complete_layout() {
            prose.originalText = original
            const body = editor()
            compare(body.text, original.replace(/\r\n/g, "\n").replace(/\r/g, "\n"))
            compare(body.textFormat, TextEdit.PlainText)
            compare(body.readOnly, true)
            compare(body.selectByMouse, true)
            compare(body.persistentSelection, true)
            compare(body.wrapMode, TextEdit.Wrap)
            compare(body.color, prose.foreground)
            compare(body.background.color, prose.background)
            compare(body.font.family, prose.fontFamily)
            compare(body.font.pixelSize, prose.fontSize)
            const label = findChild(prose, "stageProseLabel")
            compare(label.text, prose.label)
            compare(label.textFormat, Text.PlainText)
            compare(label.wrapMode, Text.Wrap)
            compare(label.color, prose.mutedForeground)
            compare(findChild(prose, "stageProseEmpty").visible, false)
            compare(body.height, body.implicitHeight)
            verify(body.height >= body.contentHeight)
            compare(prose.implicitHeight, body.y + body.height)
            const wideHeight = body.height
            prose.width = 180
            tryVerify(function() { return body.height > wideHeight })
            verify(body.height > 260, "the panel owns scrolling; prose has no height cap")
            compare(prose.implicitHeight, body.y + body.height)
        }
        function test_select_all_preserves_original() {
            prose.originalText = original
            editor().forceActiveFocus()
            keyClick(Qt.Key_A, Qt.ControlModifier)
            compare(prose.selectedText, original, "preservation is separate from native LF presentation")
            verify(prose.hasSelection)
            keyClick(Qt.Key_C, Qt.ControlModifier)
            compare(copies.count, 1)
            compare(copies.signalArguments[0][0], original)
            before.forceActiveFocus()
            compare(prose.selectedText, original, "selection persists after focus leaves")
            prose.originalText = original
            prose.width = 300
            compare(prose.selectedText, original, "unchanged input and resizing retain selection")
        }
        function test_no_controls_or_previews() {
            prose.originalText = original
            verify(prose.stageProseField)
            function walk(item) {
                for (const child of item.children || []) {
                    verify(!(child instanceof Button), "prose must have no buttons")
                    verify(!(child instanceof ScrollView), "prose must use natural height")
                    walk(child)
                }
            }
            walk(prose)
            for (const name of ["detailToggle", "detailCopy", "detailPreview"])
                compare(findChild(prose, name), null)
        }
        function test_long_line_wrap_guard() {
            prose.originalText = "x".repeat(5000)
            compare(editor().text, prose.originalText)
            compare(editor().wrapMode, TextEdit.WrapAnywhere)
            compare(editor().height, editor().implicitHeight)
            prose.originalText = "ordinary prose ".repeat(30)
            compare(editor().wrapMode, TextEdit.Wrap)
        }
        function test_partial_and_reverse_selections_data() {
            return [
                { tag: "CRLF prefix", source: "A\r\nB", start: 0, end: 2, expected: "A\r\n" },
                { tag: "CRLF newline only", source: "A\r\nB", start: 1, end: 2, expected: "\r\n" },
                { tag: "CR prefix", source: "A\rB", start: 0, end: 2, expected: "A\r" },
                { tag: "CR newline only", source: "A\rB", start: 1, end: 2, expected: "\r" },
                { tag: "mixed adjacent", source: "A\r\n\r\n\r\n\n\rB", start: 1, end: 6,
                    expected: "\r\n\r\n\r\n\n\r" },
                { tag: "repeated second", source: "same\r\nsame\rsame\n", start: 5, end: 10,
                    expected: "same\r" },
                { tag: "repeated third", source: "same\r\nsame\rsame\n", start: 10, end: 15,
                    expected: "same\n" },
                { tag: "tabs and trailing space", source: "\tA\r\nlast  \t\n", start: 7, end: 11,
                    expected: "  \t\n" },
                { tag: "astral and newline", source: "A🙂\r\n界🙂Z", start: 1, end: 7,
                    expected: "🙂\r\n界🙂" },
                { tag: "empty selection", source: "A\r\nB", start: 2, end: 2, expected: "" }
            ]
        }
        function test_partial_and_reverse_selections(data) {
            prose.originalText = data.source
            selectAndCopy(data.start, data.end, data.expected)
            selectAndCopy(data.end, data.start, data.expected)
        }
        function test_boundary_round_trips() {
            // Explicit source boundaries: CRLF interiors are unrepresentable,
            // while each surrogate code unit retains its own native position.
            prose.originalText = "A\r\n🙂\r\n\r\tZ"
            const sourceBoundaries = [0, 1, 3, 4, 5, 7, 8, 9, 10]
            const nativeBoundaries = [0, 1, -1, 2, 3, 4, -1, 5, 6, 7, 8]
            const offsets = findChild(prose, "_stageProseSelectionMap").offsets
            compare(offsets.toOriginal, sourceBoundaries)
            compare(offsets.toNative, nativeBoundaries)
            compare(editor().length, sourceBoundaries.length - 1)
            for (let native = 0; native < sourceBoundaries.length; ++native) {
                compare(offsets.toNative[offsets.toOriginal[native]], native)
                editor().select(0, native)
                compare(prose.selectedText, prose.originalText.slice(0, sourceBoundaries[native]))
                compare(nativeBoundaries[prose.selectedText.length], native)
            }
            for (let source = 0; source < nativeBoundaries.length; ++source) {
                const native = nativeBoundaries[source]
                if (native === -1) continue
                compare(offsets.toOriginal[offsets.toNative[source]], source)
                editor().select(native, 0)
                compare(prose.selectedText, prose.originalText.slice(0, source))
                compare(prose.selectedText.length, source)
            }
        }
        function test_additional_native_separators() {
            prose.originalText = "A\u2028B\u2029C\u0085D\vE\fF\u0000G"
            compare(editor().text, "A\nB\nC\u0085D\vE\fF\u0000G")
            compare(editor().length, 13, "additional separators retain native UTF-16 offsets")
            selectAndCopy(1, 4, "\u2028B\u2029")
            selectAndCopy(5, 12, "\u0085D\vE\fF\u0000")
        }
        function test_replacement_never_reuses_old_selection() {
            prose.originalText = "old\r\ntext"
            selectAndCopy(0, 4, "old\r\n")
            fixture.watchReplacement = true
            for (const source of ["A\r\nB", "A\nB", "A\rB", "🙂\r\nnew  \t", "", original]) {
                fixture.replacementSelections = []
                prose.originalText = source
                compare(prose.selectedText, "")
                verify(!prose.hasSelection)
                verify(fixture.replacementSelections.every(value => value === ""),
                    "replacement observers must never see stale selection with new source")
                copies.clear()
                prose.copySelection()
                compare(copies.count, 0)
                if (source === "") continue
                editor().forceActiveFocus()
                keyClick(Qt.Key_A, Qt.ControlModifier)
                compare(prose.selectedText, source)
                keyClick(Qt.Key_C, Qt.ControlModifier)
                compare(copies.signalArguments[0][0], source)
            }
        }
        function test_initial_document_mapping() {
            const initial = createTemporaryObject(initialProse, fixture)
            verify(initial !== null)
            const body = findChild(initial, "stageProseText")
            compare(body.text, "initial\n🙂\ntext")
            body.selectAll()
            compare(initial.selectedText, "initial\r\n🙂\rtext")
        }
        function test_pointer_selection_and_safe_focus_reveal() {
            prose.originalText = "alpha beta gamma delta\nsecond line"
            const body = editor()
            wait(30) // Pointer coordinates require the new document's layout.
            const start = body.positionToRectangle(0)
            const end = body.positionToRectangle(10)
            mouseDrag(body, start.x + 1, start.y + start.height / 2, end.x - start.x, 0)
            verify(body.activeFocus)
            verify(prose.hasSelection, "a real mouse drag selects text")
            compare(reveals.count, 0, "pointer focus cannot move the viewport during a drag")
            verify(inspections.count > 0)
            const selection = prose.originalText.slice(body.selectionStart, body.selectionEnd)
            compare(prose.selectedText, selection)
            keyClick(Qt.Key_C, Qt.ControlModifier)
            compare(copies.count, 1)
            compare(copies.signalArguments[0][0], selection)
            before.forceActiveFocus()
            body.forceActiveFocus(Qt.PopupFocusReason)
            compare(reveals.count, 0, "popup focus also avoids moving the viewport")
        }
        function test_keyboard_containment_and_focus_traversal() {
            prose.originalText = "selectable prose"
            keyClick(Qt.Key_Tab)
            verify(editor().activeFocus)
            compare(reveals.count, 1)
            compare(reveals.signalArguments[0][0], editor())
            verify(inspections.count > 0)
            keyClick(Qt.Key_R)
            keyClick(Qt.Key_D, Qt.ControlModifier)
            compare(fixture.parentKeys, 0)
            keyClick(Qt.Key_Escape)
            compare(leaves.count, 1)
            compare(fixture.parentKeys, 0)
            keyClick(Qt.Key_Tab)
            verify(after.activeFocus)
            keyClick(Qt.Key_Backtab)
            verify(editor().activeFocus)
            compare(reveals.count, 2)
        }
        function test_empty_text_does_not_copy() {
            const empty = findChild(prose, "stageProseEmpty")
            verify(empty.visible)
            compare(empty.text, "(empty text)")
            compare(empty.color, prose.mutedForeground)
            verify(!editor().visible)
            compare(prose.implicitHeight, empty.y + empty.height)
            editor().selectAll()
            prose.forceActiveFocus()
            keyClick(Qt.Key_A, Qt.ControlModifier)
            keyClick(Qt.Key_C, Qt.ControlModifier)
            prose.copySelection()
            compare(prose.selectedText, "")
            verify(!prose.hasSelection)
            compare(copies.count, 0)
        }
    }
}
