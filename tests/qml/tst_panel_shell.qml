import QtQuick
import QtTest
import "PanelFixtures.js" as Fx

// Panel redesign M1, navigation shell. Covers S1 (fixed header and tab bar), S2 (clicking a
// tab), S10 (overflow menu) and S11 (project switcher). Loads the qs-free PanelHeader,
// PanelTabBar and OverflowMenu components from quickshell/ by URL, so a missing or
// contract-breaking component fails the individual test with a readable message.
Item {
    id: fixture
    width: 760
    height: 760

    Item {
        id: stage
        anchors.fill: parent
    }

    Flickable {
        id: scroller
        parent: stage
        width: 760
        contentWidth: 760
        contentHeight: 3200
        clip: true
        Rectangle {
            width: 760
            height: 3200
            color: "#303030"
        }
    }

    TestCase {
        name: "PanelShell"
        when: windowShown

        property var created: []
        readonly property var tabIds: ["overview", "plan", "activity", "architecture", "features", "queue", "settings"]
        readonly property var tabLabels: ["Overview", "Plan", "Activity", "Architecture", "Features", "Queue", "Settings"]

        function make(name, props) {
            const comp = Qt.createComponent("../../quickshell/" + name + ".qml")
            if (comp.status !== Component.Ready)
                fail(name + ".qml cannot be loaded: " + comp.errorString())
            const obj = comp.createObject(stage, Object.assign({}, Fx.palette, props))
            if (!obj) fail(name + ".qml rejected the contract properties: " + comp.errorString())
            created.push(obj)
            return obj
        }

        function makeHeader(extra) {
            return make("PanelHeader", Object.assign({
                projectName: "forge", sessions: Fx.sessions(), activeProject: "/home/user/Projects/forge",
                phase: "running", engineOnline: true, busy: true, stepText: "stage 3: implementing",
                backgroundBusy: false, activeProjectCount: 1, width: 760
            }, extra || {}))
        }

        function makeBar(extra) {
            return make("PanelTabBar", Object.assign({ currentTab: "overview", queueCount: 0, width: 760 }, extra || {}))
        }

        function makeMenu(items) {
            return make("OverflowMenu", { items: items, open: true, width: 240 })
        }

        function cleanup() {
            for (let i = 0; i < created.length; ++i) created[i].destroy()
            created = []
            scroller.contentY = 0
            wait(10)
        }

        function selectedTabs(bar) {
            return tabIds.filter(id => {
                const found = Fx.walk(bar, "tab_" + id)
                return found.length === 1 && found[0].selected === true
            })
        }

        // ---- S1 -------------------------------------------------------------------------

        function test_S1_header_and_tab_bar_stay_in_place_while_the_view_scrolls() {
            const header = makeHeader()
            const bar = makeBar({ currentTab: "plan" })
            header.y = 0
            bar.y = header.height
            scroller.y = bar.y + bar.height
            scroller.height = stage.height - scroller.y
            verify(header.height > 0, "the header needs a height")
            verify(bar.height > 0, "the tab bar needs a height")
            compare(scroller.contentHeight > scroller.height, true)

            const headerY = header.mapToItem(stage, 0, 0).y
            const barY = bar.mapToItem(stage, 0, 0).y
            scroller.contentY = scroller.contentHeight - scroller.height
            compare(scroller.contentY, scroller.contentHeight - scroller.height)
            wait(20)

            compare(header.mapToItem(stage, 0, 0).y, headerY)
            compare(bar.mapToItem(stage, 0, 0).y, barY)
            verify(header.visible && bar.visible)
            for (const name of ["forgeTitle", "projectSwitcher", "phaseBadge", "currentStep", "overflowButton"]) {
                const found = Fx.walk(header, name)
                compare(found.length, 1, "PanelHeader must contain exactly one " + name)
                verify(Fx.insideArea(found[0], stage), name + " must stay visible inside the 760 px panel")
            }
            compare(Fx.allText(Fx.walk(header, "forgeTitle")[0]), "FORGE")
            verify(Fx.allText(Fx.walk(header, "phaseBadge")[0]).indexOf("running") >= 0, "phase badge shows the phase")
            verify(Fx.allText(Fx.walk(header, "currentStep")[0]).indexOf("stage 3: implementing") >= 0,
                "the current step text is shown")
            verify(Fx.allText(Fx.walk(header, "projectSwitcher")[0]).indexOf("forge") >= 0,
                "the switcher names the current project")
        }

        function test_S1_the_tab_bar_lists_the_seven_tabs_and_marks_the_selected_one() {
            const bar = makeBar({ currentTab: "plan" })
            for (let i = 0; i < tabIds.length; ++i) {
                const found = Fx.walk(bar, "tab_" + tabIds[i])
                compare(found.length, 1, "tab_" + tabIds[i])
                compare(found[0].label, tabLabels[i])
                verify(Fx.allText(found[0]).indexOf(tabLabels[i]) >= 0, tabLabels[i] + " is drawn on its tab")
                verify(Fx.insideArea(found[0], stage), tabLabels[i] + " must fit in the 760 px panel")
            }
            compare(selectedTabs(bar), ["plan"])
        }

        function test_S1_header_keeps_the_background_activity_indicator() {
            const quiet = makeHeader({ backgroundBusy: false, activeProjectCount: 1 })
            compare(Fx.allText(quiet).indexOf("active") >= 0, false)
            quiet.destroy()
            const busy = makeHeader({ backgroundBusy: true, activeProjectCount: 2 })
            verify(Fx.allText(busy).indexOf("2 projects active") >= 0,
                "background sessions are announced as '2 projects active'")
        }

        function test_S1_header_shows_engine_offline_instead_of_the_phase() {
            const header = makeHeader({ engineOnline: false, phase: "offline" })
            verify(Fx.allText(Fx.walk(header, "phaseBadge")[0]).indexOf("engine offline") >= 0)
        }

        // ---- S2 -------------------------------------------------------------------------

        function test_S2_clicking_each_tab_requests_that_view_and_marks_it_selected() {
            const bar = makeBar({ currentTab: "settings" })
            const seen = []
            bar.tabRequested.connect(id => seen.push(id))
            compare(selectedTabs(bar), ["settings"])
            for (const id of tabIds) {
                const tab = Fx.walk(bar, "tab_" + id)[0]
                verify(tab, "tab_" + id + " must exist")
                mouseClick(tab)
                compare(seen[seen.length - 1], id, "clicking " + id + " must request it")
                bar.currentTab = id
                compare(selectedTabs(bar), [id], "exactly the clicked tab is selected")
            }
            compare(seen, tabIds)
        }

        function test_S2_only_the_current_tab_is_selected_for_every_tab() {
            const bar = makeBar()
            for (const id of tabIds) {
                bar.currentTab = id
                compare(selectedTabs(bar), [id])
            }
        }

        // ---- S10 ------------------------------------------------------------------------

        function overflowItems(overrides) {
            const base = [
                { id: "update", label: "Update Forge", enabled: true },
                { id: "discard", label: "Discard plan", enabled: false },
                { id: "refactor", label: "Refactor plan", enabled: true },
                { id: "diff", label: "View diff", enabled: true },
                { id: "changeProject", label: "Change project", enabled: true },
                { id: "help", label: "Keyboard help", enabled: true }
            ]
            return base.map(item => Object.assign({}, item, (overrides || {})[item.id] || {}))
        }

        function test_S10_the_overflow_button_asks_the_panel_to_open_the_menu() {
            const header = makeHeader()
            const spy = []
            header.overflowRequested.connect(() => spy.push(true))
            mouseClick(Fx.walk(header, "overflowButton")[0])
            compare(spy.length, 1)
        }

        function test_S10_the_menu_lists_every_rare_action_with_its_guard() {
            const items = overflowItems()
            const menu = makeMenu(items)
            wait(20)
            const rows = items.map(item => Fx.walk(menu, "overflowItem_" + item.id))
            for (let i = 0; i < items.length; ++i) {
                compare(rows[i].length, 1, "overflowItem_" + items[i].id)
                verify(rows[i][0].visible, items[i].label + " is listed while the menu is open")
                compare(rows[i][0].enabled, items[i].enabled, items[i].label + " reflects its guard")
                verify(Fx.allText(rows[i][0]).indexOf(items[i].label) >= 0, items[i].label + " is drawn")
            }
            const order = rows.map(r => r[0]).sort((a, b) => a.mapToItem(stage, 0, 0).y - b.mapToItem(stage, 0, 0).y)
            compare(order.map(r => r.objectName), items.map(item => "overflowItem_" + item.id))
        }

        function test_S10_choosing_an_enabled_row_emits_its_id_and_a_disabled_row_emits_nothing() {
            const menu = makeMenu(overflowItems())
            const chosen = []
            menu.itemChosen.connect(id => chosen.push(id))
            wait(20)
            mouseClick(Fx.walk(menu, "overflowItem_update")[0])
            compare(chosen, ["update"])
            menu.open = true
            wait(20)
            mouseClick(Fx.walk(menu, "overflowItem_discard")[0])
            compare(chosen, ["update"], "a disabled row must emit nothing")
            for (const id of ["refactor", "diff", "changeProject", "help"]) {
                menu.open = true
                wait(20)
                mouseClick(Fx.walk(menu, "overflowItem_" + id)[0])
            }
            compare(chosen, ["update", "refactor", "diff", "changeProject", "help"])
        }

        function test_S10_a_busy_engine_disables_update_and_discard_in_the_menu() {
            const menu = makeMenu(overflowItems({ update: { enabled: false }, discard: { enabled: false } }))
            const chosen = []
            menu.itemChosen.connect(id => chosen.push(id))
            wait(20)
            compare(Fx.walk(menu, "overflowItem_update")[0].enabled, false)
            mouseClick(Fx.walk(menu, "overflowItem_update")[0])
            compare(chosen.length, 0)
            compare(Fx.walk(menu, "overflowItem_help")[0].enabled, true)
        }

        function test_S10_a_closed_menu_shows_no_rows() {
            const menu = makeMenu(overflowItems())
            menu.open = false
            wait(20)
            for (const item of overflowItems()) {
                const rows = Fx.walk(menu, "overflowItem_" + item.id)
                verify(rows.length === 0 || !rows[0].visible, item.label + " must be hidden while the menu is closed")
            }
        }

        // ---- S11 ------------------------------------------------------------------------

        function test_S11_the_switcher_lists_both_projects_with_their_markers() {
            const header = makeHeader()
            header.openSwitcher()
            tryVerify(() => header.switcherOpen)
            wait(20)
            const rows = Fx.walk(header, "sessionRow")
            compare(rows.length, 2)
            compare(rows.map(r => r.label), ["forge ●", "site ! +2"])
            for (const row of rows) verify(row.visible, row.label + " must be visible in the open switcher")
        }

        function test_S11_the_switcher_uses_the_same_markers_as_the_old_project_tabs() {
            const header = makeHeader({ sessions: Fx.allMarkerSessions() })
            header.openSwitcher()
            wait(20)
            compare(Fx.walk(header, "sessionRow").map(r => r.label),
                ["busy ●", "queue ●", "failed !", "done ✓", "idle · +3"])
        }

        function test_S11_choosing_a_project_selects_it_and_keeps_the_selected_tab() {
            const bar = makeBar({ currentTab: "plan" })
            const header = makeHeader()
            const picked = []
            const tabRequests = []
            header.projectSelected.connect(path => picked.push(path))
            bar.tabRequested.connect(id => tabRequests.push(id))
            header.openSwitcher()
            wait(20)
            mouseClick(Fx.walk(header, "sessionRow")[1])
            compare(picked, ["/home/user/Projects/site"])
            compare(tabRequests.length, 0, "switching project must not request another tab")
            compare(bar.currentTab, "plan")
            compare(selectedTabs(bar), ["plan"])
        }

        function test_S11_change_project_opens_the_chooser_and_keeps_the_selected_tab() {
            const bar = makeBar({ currentTab: "activity" })
            const header = makeHeader()
            const requested = []
            const tabRequests = []
            header.changeProjectRequested.connect(() => requested.push(true))
            bar.tabRequested.connect(id => tabRequests.push(id))
            header.openSwitcher()
            wait(20)
            const rows = Fx.walk(header, "changeProjectRow")
            compare(rows.length, 1)
            verify(Fx.allText(rows[0]).indexOf("Change project…") >= 0)
            mouseClick(rows[0])
            compare(requested.length, 1)
            compare(tabRequests.length, 0)
            compare(bar.currentTab, "activity")
        }

        function test_S11_clicking_the_project_name_opens_the_switcher() {
            const header = makeHeader()
            compare(header.switcherOpen, false)
            mouseClick(Fx.walk(header, "projectSwitcher")[0])
            tryVerify(() => header.switcherOpen)
        }
    }
}
