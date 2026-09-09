// Run in an isolated Quickshell process with QT_QPA_PLATFORM=offscreen.
import QtQuick
import Quickshell
import "components"

ShellRoot {
    id: fixture
    property int next: 0
    property bool passed: true
    property var values: ["\r\n \t\r  <b>界🙂</b>\r\nlast  \t\n", " \r\n\t ", "",
        "x".repeat(2000), "\n\n\n" + "y".repeat(120000) + "\n  trailing  \t"]
    FloatingWindow {
        visible: true
        implicitWidth: 440
        implicitHeight: 320
        CompactDetail {
            id: detail
            width: 430
            onExpansionRequested: value => expanded = value
            onCopyRequested: original => {
                Quickshell.clipboardText = original
                if (Quickshell.clipboardText !== originalText) fixture.passed = false
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
                console.log(fixture.passed ? "EXACT_COPY_PASSED (5 originals)" : "EXACT_COPY_FAILED")
                Qt.quit()
                return
            }
            detail.originalText = fixture.values[fixture.next++]
            if (!detail.expanded) fixture.find(detail, "detailToggle").clicked()
            fixture.find(detail, "detailCopy").clicked()
        }
    }
}
