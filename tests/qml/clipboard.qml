// Run in an isolated Quickshell process with QT_QPA_PLATFORM=offscreen.
import QtQuick
import QtTest
import Quickshell
import "components"

ShellRoot {
    id: fixture
    property int next: 0
    property bool passed: true
    property int stageCopies: 0
    property var values: ["\r\n \t\r  <b>界🙂</b>\r\nlast  \t\n", " \r\n\t ", "",
        "x".repeat(2000), "\n\n\n" + "y".repeat(120000) + "\n  trailing  \t"]
    FloatingWindow {
        visible: true
        implicitWidth: 880
        implicitHeight: 320
        TestCase { id: keyboard; name: "ClipboardKeys"; when: false }
        CompactDetail {
            id: detail
            width: 430
            onExpansionRequested: value => expanded = value
            onCopyRequested: original => {
                Quickshell.clipboardText = original
                if (Quickshell.clipboardText !== originalText) fixture.passed = false
            }
        }
        StageProse {
            id: prose
            x: 440
            width: 430
            label: "Stage prose"
            onCopyRequested: original => {
                fixture.stageCopies++
                Quickshell.clipboardText = original
                if (original !== originalText || Quickshell.clipboardText !== originalText)
                    fixture.passed = false
            }
        }
    }
    function find(item, name) {
        if (item.objectName === name) return item
        for (const child of item.children || []) {
            const found = find(child, name)
            if (found) return found
        }
        return null
    }
    Timer {
        interval: 100
        running: true
        repeat: true
        onTriggered: {
            if (fixture.next === fixture.values.length) {
                console.log(fixture.passed && fixture.stageCopies === 4
                    ? "EXACT_COPY_PASSED (5 originals, CompactDetail and StageProse)" : "EXACT_COPY_FAILED")
                Qt.quit()
                return
            }
            detail.originalText = fixture.values[fixture.next++]
            if (!detail.expanded) fixture.find(detail, "detailToggle").clicked()
            fixture.find(detail, "detailCopy").clicked()
            prose.originalText = detail.originalText
            const editor = fixture.find(prose, "stageProseText")
            editor.forceActiveFocus()
            keyboard.keyClick(Qt.Key_A, Qt.ControlModifier)
            if (prose.selectedText !== prose.originalText) fixture.passed = false
            // Empty prose has no selection and must not emit or alter the clipboard.
            Quickshell.clipboardText = "stage copy sentinel"
            const before = fixture.stageCopies
            keyboard.keyClick(Qt.Key_C, Qt.ControlModifier)
            if (prose.originalText === "") {
                if (fixture.stageCopies !== before || Quickshell.clipboardText !== "stage copy sentinel")
                    fixture.passed = false
            } else if (fixture.stageCopies !== before + 1) {
                fixture.passed = false
            }
        }
    }
}
