import QtQuick
import QtTest
import "PanelFixtures.js" as Fx

// Panel redesign M2 polish, compact Settings quota. At 760x760 the Claude and Fable quota lines
// wrapped to two lines each and pushed MAINTENANCE below the fold. Each quota window is now one
// compact line ("Claude · 5h 86% remaining · resets Sat 01:10"); the full text stays available
// behind quotaDetailsToggle, and the view scrolls so that MAINTENANCE is reachable. Loads the
// qs-free SettingsView from quickshell/ by URL, as tst_panel_settings.qml does.
Item {
    id: fixture
    width: 728
    height: 600

    Item {
        id: area
        anchors.fill: parent
    }

    Text {
        id: metric
        visible: false
        text: "Ag"
        font.family: "monospace"
        font.pixelSize: 12
    }

    TestCase {
        name: "PanelSettingsCompact"
        when: windowShown

        property var created: []

        function make(extra) {
            const comp = Qt.createComponent("../../quickshell/SettingsView.qml")
            if (comp.status !== Component.Ready)
                fail("SettingsView.qml cannot be loaded: " + comp.errorString())
            const state = Fx.engineState("idle", null)
            const obj = comp.createObject(area, Object.assign({
                width: 728, height: 600, engineState: state, catalogue: state.model_catalogue,
                quota: state.claude_quota, engineOnline: true, busy: false, catalogueOpen: false,
                reviewerLabel: "reviewer: codex",
                cadenceLabels: { architect: "per plan", reviewer: "per stage" }
            }, Fx.palette, extra || {}))
            if (!obj) fail("SettingsView.qml rejected the contract properties: " + comp.errorString())
            created.push(obj)
            return obj
        }

        function cleanup() {
            for (let i = 0; i < created.length; ++i) created[i].destroy()
            created = []
            wait(10)
        }

        function one(view, name) {
            const found = Fx.walk(view, name)
            compare(found.length, 1, "SettingsView must contain exactly one " + name)
            return found[0]
        }

        function lines(view) {
            return Fx.walk(view, "quotaLine").filter(o => o.visible)
        }

        // Windows as the engine reports them, including the per-model Fable window.
        function longQuota() {
            const quota = Fx.quota()
            const soon = Math.floor(Date.now() / 1000) + 3600
            quota.windows.push({ name: "Fable · weekly", model: "fable", used_percent: 41.0,
                resets_unix: soon + 3 * 86400, resets_at: new Date((soon + 3 * 86400) * 1000).toISOString() })
            return quota
        }

        function scrollToEnd(view) {
            view.contentY = Math.max(0, view.contentHeight - view.height)
            wait(30)
        }

        // ---- polish ---------------------------------------------------------------------

        function test_polish_every_quota_window_is_one_line() {
            const view = make({ quota: longQuota() })
            wait(30)
            const found = lines(view)
            compare(found.length, 3, "one quotaLine per usage window")
            for (const line of found) {
                verify(line.height < 2 * metric.height,
                    "a quota line is one line high, got " + line.height + " (a text line is " + metric.height + ")")
                const texts = Fx.collect(line, o => typeof o.lineCount === "number")
                verify(texts.length > 0, "a quota line draws text")
                for (const text of texts) compare(text.lineCount, 1, "a quota line is a single text line")
            }
        }

        function test_polish_the_quota_lines_show_each_window_name_and_remaining_percentage() {
            const view = make({ quota: longQuota() })
            wait(30)
            const text = lines(view).map(line => Fx.allText(line)).join("\n")
            verify(text.indexOf("Claude · 5h") >= 0, "the 5h window is listed\n" + text)
            verify(text.indexOf("86% remaining") >= 0, "the 5h remaining percentage is listed\n" + text)
            verify(text.indexOf("Claude · weekly") >= 0, "the weekly window is listed\n" + text)
            verify(text.indexOf("67% remaining") >= 0, "the weekly remaining percentage is listed\n" + text)
            verify(text.indexOf("Fable · weekly") >= 0, "the Fable window is listed\n" + text)
            verify(text.indexOf("59% remaining") >= 0, "the Fable remaining percentage is listed\n" + text)
            verify(/resets/.test(text), "each line says when the window resets\n" + text)
        }

        function test_polish_the_quota_lines_fit_the_panel_width() {
            const view = make({ quota: longQuota(), width: 600 })
            wait(30)
            compare(lines(view).length, 3, "one quotaLine per usage window")
            for (const line of lines(view)) {
                const p = line.mapToItem(view, 0, 0)
                verify(p.x >= 0 && p.x + line.width <= view.width + 0.5, "a quota line must not overflow the width")
                for (const text of Fx.collect(line, o => typeof o.lineCount === "number"))
                    compare(text.lineCount, 1, "a quota line stays on one line at 600 px")
            }
        }

        function test_polish_the_full_quota_text_is_available_behind_a_toggle() {
            const view = make({ quota: longQuota() })
            wait(30)
            const toggle = one(view, "quotaDetailsToggle")
            verify(toggle.visible, "the toggle is shown")
            compare(Fx.walk(view, "quotaDetails").filter(o => o.visible).length, 0,
                "the details stay collapsed until asked for")
            mouseClick(toggle)
            wait(30)
            const details = Fx.walk(view, "quotaDetails").filter(o => o.visible)
            compare(details.length, 1, "the toggle reveals quotaDetails")
            const text = Fx.allText(details[0])
            verify(text.indexOf("Claude · 5h: 86% remaining") >= 0, "the full 5h text is shown\n" + text)
            verify(text.indexOf("Claude · weekly: 67% remaining") >= 0, "the full weekly text is shown\n" + text)
            verify(text.indexOf("Fable · weekly: 59% remaining") >= 0, "the full Fable text is shown\n" + text)
            mouseClick(toggle)
            wait(30)
            compare(Fx.walk(view, "quotaDetails").filter(o => o.visible).length, 0,
                "the toggle collapses the details again")
        }

        function test_polish_a_pending_quota_is_one_line_too() {
            const view = make({ quota: { status: "pending", windows: [] } })
            wait(30)
            const found = lines(view)
            compare(found.length, 1, "one line while the limits are being checked")
            verify(Fx.allText(found[0]).indexOf("Claude limits: checking…") >= 0, Fx.allText(found[0]))
        }

        function test_polish_maintenance_is_reachable_by_scrolling_a_short_view() {
            const view = make({ quota: longQuota(), height: 320 })
            wait(30)
            verify(view.contentHeight > view.height, "Settings is taller than a 320 px view, so it must scroll")
            scrollToEnd(view)
            for (const name of ["settingsAction_update", "settingsAction_changeProject"])
                verify(Fx.insideArea(one(view, name), view), name + " must be reachable by scrolling to the end")
            view.contentY = 0
            wait(30)
            verify(!Fx.insideArea(one(view, "settingsAction_update"), view),
                "at the top of a short view MAINTENANCE is below the fold")
        }

        function test_polish_maintenance_is_reachable_at_the_panel_size() {
            const view = make({ quota: longQuota() })
            wait(30)
            scrollToEnd(view)
            for (const name of ["settingsAction_update", "settingsAction_changeProject"])
                verify(Fx.insideArea(one(view, name), view), name + " must be inside the 728x600 view")
        }
    }
}
