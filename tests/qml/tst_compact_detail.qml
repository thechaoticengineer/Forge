import QtQuick
import QtQuick.Controls
import QtTest
import "../../quickshell"
import "../../quickshell/DetailView.js" as DetailView

Item {
    id: fixture
    width: 600
    height: 520
    property string copied: ""
    property int parentKeys: 0
    Keys.onPressed: event => { parentKeys++; event.accepted = true }
    ListModel { id: entries }
    DetailList {
        id: list
        width: 520
        height: 280
        model: entries
        onCopyRequested: original => fixture.copied = original
    }
    TestCase {
        name: "CompactDetail"
        when: windowShown
        function record(id, text, kind) {
            return { id: id, text: text, kind: kind || "message" }
        }
        function append(records) {
            list.beginUpdate()
            DetailView.reconcile(entries, records, "/fixture", "session")
            list.endUpdate()
            wait(30)
        }
        function controls(name) {
            const found = []
            function walk(item) {
                const kids = item.children || []
                for (let i = 0; i < kids.length; ++i) {
                    if (kids[i].objectName === name) found.push(kids[i])
                    walk(kids[i])
                }
            }
            walk(list)
            return found
        }
        function focusedIndex(items) {
            for (let i = 0; i < items.length; ++i) if (items[i].activeFocus) return i
            return -1
        }
        function revealed(item) {
            const y = item.mapToItem(list, 0, 0).y
            return y >= -0.5 && y + Math.min(item.height, list.height) <= list.height + 0.5
        }
        function init() {
            entries.clear()
            list.width = 520
            list.height = 280
            list.history = false
            list.resetView()
            fixture.copied = ""
            fixture.parentKeys = 0
            wait(30)
        }
        function test_preview_expand_and_exact_copy() {
            const source = "\r\n \t\r  <b>plain 界🙂</b> " + "long ".repeat(160) + "\r\nlast  \t\n"
            append([record("one", source, "error")])
            const preview = findChild(list, "detailPreview")
            compare(preview.textFormat, Text.PlainText)
            compare(preview.wrapMode, Text.NoWrap)
            compare(preview.elide, Text.ElideRight)
            compare(preview.maximumLineCount, 1)
            verify(preview.truncated)
            verify(preview.width >= 0)
            const toggle = findChild(list, "detailToggle")
            toggle.forceActiveFocus()
            keyClick(Qt.Key_Space)
            tryCompare(entries.get(0), "expanded", true)
            const editor = findChild(list, "detailFullText")
            compare(editor.readOnly, true)
            compare(editor.textFormat, TextEdit.PlainText)
            editor.forceActiveFocus()
            keyClick(Qt.Key_A, Qt.ControlModifier)
            verify(editor.selectedText.length > 100)
            keyClick(Qt.Key_C, Qt.ControlModifier)
            const before = fixture.parentKeys
            keyClick(Qt.Key_R)
            keyClick(Qt.Key_D, Qt.ControlModifier)
            compare(fixture.parentKeys, before, "selection keys cannot invoke panel actions")
            const copy = findChild(list, "detailCopy")
            copy.forceActiveFocus()
            keyClick(Qt.Key_Space)
            compare(fixture.copied, source)
            compare(entries.get(0).expanded, true, "copy cannot collapse a row")
        }
        function test_blank_and_single_line_keyboard_mouse_controls() {
            append([record("blank", " \r\n\t ")])
            compare(findChild(list, "detailPreview").text, "(empty text)")
            let toggle = findChild(list, "detailToggle")
            mouseClick(toggle)
            tryCompare(entries.get(0), "expanded", true)
            mouseClick(findChild(list, "detailCopy"))
            compare(fixture.copied, " \r\n\t ")
            toggle.forceActiveFocus()
            keyClick(Qt.Key_Space)
            tryCompare(entries.get(0), "expanded", false)
            entries.clear()
            append([record("single", "x".repeat(600))])
            toggle = findChild(list, "detailToggle")
            toggle.forceActiveFocus()
            keyClick(Qt.Key_Space)
            tryCompare(entries.get(0), "expanded", true)
            list.width = 110
            wait(30)
            verify(findChild(list, "detailPreview").width >= 0)
            verify(toggle.width <= list.width)
        }
        function test_kinds_errors_timestamps_and_goals_stay_separate_from_previews() {
            const body = "  first line  \nsecond"
            for (const kind of ["error", "git", "review", "message"]) {
                entries.clear()
                list.resetView()
                append([{ id: kind, text: body, kind: kind, stream: "",
                    t: "12:00:00", unix: 0, goal: "ship it" }])
                const preview = findChild(list, "detailPreview")
                compare(preview.text, "  first line  ", "metadata never enters the preview")
                compare(preview.color, kind === "error" ? list.urgent
                    : kind === "git" ? list.success
                    : kind === "review" ? list.working : list.foreground, kind)
            }
            // stderr is identified as urgent even when its record kind is not.
            entries.clear()
            list.resetView()
            append([{ id: "e", text: body, kind: "message", stream: "stderr" }])
            compare(findChild(list, "detailPreview").color, list.urgent)

            list.history = true
            entries.clear()
            list.resetView()
            append([{ id: "h", text: body, kind: "error", t: "12:00:00", unix: 0, goal: "ship it" }])
            list.beginUpdate()
            DetailView.filterRows(entries, "all")
            list.endUpdate()
            wait(30)
            const separator = findChild(list, "detailSeparator")
            verify(separator.visible, "the goal separator is its own row")
            compare(separator.text, "— goal: ship it")
            compare(separator.color, list.accent)
            compare(findChild(list, "detailPreview").text, "  first line  ")
            compare(findChild(list, "detailMetadata").text, "12:00:00 [error]",
                "the timestamp and kind stay outside the message preview")
        }
        function test_long_lines_and_large_history_stay_responsive() {
            // Collapsed rows must not build an editor: laying one out per entry
            // is what made a single long line stall the interface for minutes.
            const line = "x".repeat(200000)
            let started = Date.now()
            append([record("long", "\n" + line, "error")])
            verify(Date.now() - started < 10000, "a long line must not stall a collapsed row")
            compare(findChild(list, "detailFullText"), null, "a collapsed row has no editor")
            const preview = findChild(list, "detailPreview")
            verify(preview.truncated, "the preview still elides by width")
            verify(preview.text.length < 2000,
                "an ordinary long line stops growing after the first layout")
            list.width = 200
            wait(30)
            verify(preview.truncated)
            verify(preview.text.length < 2000, "narrowing does not enlarge the slice")
            list.width = 520
            wait(30)
            verify(preview.truncated)
            verify(preview.text.length < 2000, "widening re-estimates without escalating")

            started = Date.now()
            list.beginUpdate()
            DetailView.expand(entries, entries.get(0).entryKey, true)
            list.endUpdate()
            wait(30)
            verify(Date.now() - started < 10000, "expanding a long line must not stall")
            const editor = findChild(list, "detailFullText")
            compare(editor.text.length, line.length + 1, "every original character is exposed")
            compare(editor.wrapMode, TextEdit.WrapAnywhere)
            mouseClick(findChild(list, "detailCopy"))
            compare(fixture.copied, "\n" + line)

            list.beginUpdate()
            DetailView.expand(entries, entries.get(0).entryKey, false)
            list.endUpdate()
            wait(30)
            compare(findChild(list, "detailFullText"), null, "collapsing releases the editor")

            entries.clear()
            list.resetView()
            const records = []
            for (let i = 0; i < 400; ++i)
                records.push(record("h" + i, "message " + i + "\nsecond line", "git"))
            started = Date.now()
            append(records)
            verify(Date.now() - started < 10000, "a full history must build promptly")
            compare(entries.count, 400)
            list.beginUpdate()
            DetailView.expand(entries, entries.get(7).entryKey, true)
            list.endUpdate()
            wait(30)
            compare(findChild(list, "detailFullText").wrapMode, TextEdit.Wrap,
                "ordinary messages keep word wrapping")
        }
        function test_poll_filter_resize_and_append_preserve_selection_and_reading() {
            const records = []
            for (let i = 0; i < 25; ++i) records.push(record("r" + i, "same line\nsecond line  ", i === 0 ? "error" : "git"))
            append(records)
            list.followTail = false
            list.positionViewAtBeginning()
            list.captureReading()
            const toggle = findChild(list, "detailToggle")
            mouseClick(toggle)
            const editor = findChild(list, "detailFullText")
            editor.forceActiveFocus()
            mouseDoubleClickSequence(editor, 30, 12)
            verify(editor.selectedText.length > 0)
            compare(entries.get(0).expanded, true, "selecting text cannot collapse a row")
            editor.select(0, 9)
            const selection = editor.selectedText
            append(records) // an unchanged polling response
            compare(findChild(list, "detailFullText"), editor)
            compare(editor.selectedText, selection)
            append([record("r25", "appended")])
            compare(editor.selectedText, selection)
            compare(list.contentY, 0)
            list.beginUpdate()
            DetailView.filterRows(entries, "git")
            list.endUpdate()
            wait(30)
            list.beginUpdate()
            DetailView.filterRows(entries, "all")
            list.endUpdate()
            wait(30)
            compare(findChild(list, "detailFullText"), editor)
            compare(editor.selectedText, selection)
            list.width = 360
            wait(30)
            compare(editor.selectedText, selection)
            compare(entries.get(0).expanded, true)
            // Reading a middle row stays anchored when an earlier row grows.
            list.contentY = 350
            list.captureReading()
            const anchor = list.readingAnchor
            list.beginUpdate()
            DetailView.expand(entries, entries.get(1).entryKey, true)
            list.endUpdate()
            wait(30)
            compare(list.readingAnchor.key, anchor.key)
            compare(Math.round(list.readingAnchor.offset), Math.round(anchor.offset))
            const y = list.contentY
            append([record("r26", "appended again")])
            compare(Math.round(list.contentY), Math.round(y))
            list.beginUpdate()
            DetailView.filterRows(entries, "errors")
            list.endUpdate()
            wait(30)
            append(records) // polling while the reading anchor is filtered out
            list.beginUpdate()
            DetailView.filterRows(entries, "all")
            list.endUpdate()
            wait(30)
            compare(Math.round(list.contentY), Math.round(y), "returning to the filter restores its hidden anchor")
            // Selection blocks incidental movement to the tail.
            list.followTail = true
            list.scrollToTail()
            compare(list.followTail, false)
            editor.deselect()
            // Explicit navigation back to the tail resumes appending behavior.
            list.followTail = true
            list.scrollToTail()
            append([record("r27", "tail")])
            verify(list.atYEnd)
        }
        function test_zero_width_and_combining_text_elides_by_rendered_width() {
            // Zero-width and combining characters occupy no advance, so no
            // character budget can decide what fits. The renderer must.
            const invisible = String.fromCharCode(0x200b).repeat(1000)
            const fitting = "a" + invisible + "VISIBLE SUFFIX"
            append([record("zw", fitting)])
            let preview = findChild(list, "detailPreview")
            verify(preview.width > 0)
            tryVerify(function() { return preview.text.indexOf("VISIBLE SUFFIX") !== -1 },
                2000, "text that fits after zero-width characters stays visible")
            verify(!preview.truncated, "text narrower than the width is not elided")
            verify(preview.contentWidth <= preview.width + 0.5)
            mouseClick(findChild(list, "detailToggle"))
            tryCompare(entries.get(0), "expanded", true)
            mouseClick(findChild(list, "detailCopy"))
            compare(fixture.copied, fitting, "the copy source keeps every original character")

            // The same line plus visible overflow must elide by width instead.
            entries.clear()
            list.resetView()
            const overflowing = "a" + invisible + "V".repeat(400)
            append([record("zw2", overflowing)])
            preview = findChild(list, "detailPreview")
            tryVerify(function() { return preview.truncated }, 2000,
                "overflow after zero-width characters still elides by width")
            verify(preview.contentWidth <= preview.width + 0.5)
            verify(preview.text.length > 1001, "the elided slice reaches the visible characters")

            // Combining marks behave the same way and never reach the source.
            entries.clear()
            list.resetView()
            const combining = "e" + String.fromCharCode(0x0301).repeat(800) + "TAIL"
            append([record("cm", combining)])
            preview = findChild(list, "detailPreview")
            tryVerify(function() { return preview.text.indexOf("TAIL") !== -1 }, 2000,
                "combining marks do not consume the preview budget")
            mouseClick(findChild(list, "detailToggle"))
            tryCompare(entries.get(0), "expanded", true)
            compare(findChild(list, "detailFullText").text, combining)

            // The worst case for growth is a line that is entirely zero-advance:
            // every character must be laid out before anything can be shown.
            entries.clear()
            list.resetView()
            const allInvisible = String.fromCharCode(0x200b).repeat(200000) + "END"
            const started = Date.now()
            append([record("zw3", allInvisible)])
            preview = findChild(list, "detailPreview")
            tryVerify(function() { return preview.text.indexOf("END") !== -1 }, 20000,
                "growth reaches visible text past 200,000 zero-advance characters")
            verify(Date.now() - started < 10000, "the worst case stays responsive")
        }
        function test_keyboard_traversal_reveals_focused_controls() {
            const records = []
            for (let i = 0; i < 25; ++i)
                records.push(record("t" + i, "row " + i + "\nsecond line", "git"))
            list.height = 150
            append(records)
            verify(list.atYEnd, "a new list follows the tail")
            const toggles = controls("detailToggle")
            compare(toggles.length, 25)
            toggles[24].forceActiveFocus()
            wait(30)
            verify(revealed(toggles[24]))
            for (let i = 0; i < 10; ++i) { keyClick(Qt.Key_Backtab); wait(10) }
            const back = focusedIndex(toggles)
            verify(back >= 0 && back < 24, "Backtab moves focus toward earlier rows")
            verify(revealed(toggles[back]),
                "Backtab across the viewport boundary reveals its target")
            for (let i = 0; i < 6; ++i) { keyClick(Qt.Key_Tab); wait(10) }
            const forward = focusedIndex(toggles)
            verify(forward > back, "Tab moves focus toward later rows")
            verify(revealed(toggles[forward]),
                "Tab across the viewport boundary reveals its target")

            // Controls inside an expanded detail are revealed as focus enters them.
            list.beginUpdate()
            DetailView.expand(entries, entries.get(2).entryKey, true)
            list.endUpdate()
            wait(30)
            list.positionViewAtEnd()
            list.captureReading()
            wait(30)
            toggles[2].forceActiveFocus()
            wait(30)
            verify(revealed(toggles[2]), "focusing a scrolled-away row reveals it")
            keyClick(Qt.Key_Tab)
            wait(30)
            const copy = controls("detailCopy")[0]
            verify(copy.activeFocus, "Tab reaches the copy control of an expanded row")
            verify(revealed(copy), "the focused copy control is revealed")

            // Neither an unchanged poll nor a pointer press moves the view.
            const resting = list.contentY
            append(records)
            compare(Math.round(list.contentY), Math.round(resting),
                "an unchanged poll leaves the revealed position alone")
            const editor = controls("detailFullText")[0]
            list.contentY = resting + 30
            list.captureReading()
            wait(30)
            const before = list.contentY
            mousePress(editor, 6, 6)
            mouseRelease(editor, 6, 6)
            wait(30)
            verify(editor.activeFocus, "a pointer press still focuses the editor")
            compare(Math.round(list.contentY), Math.round(before),
                "a pointer press cannot move the view under a selection")
        }
    }
}
