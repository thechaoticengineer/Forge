import QtQuick
import QtTest
import "PanelFixtures.js" as Fx

// Panel redesign M1, Queue tab. Covers S18: with two queued goals the Queue view lists them with
// Start queue, ↑, ↓ and ×, each emitting the right signal, and the tab bar labels the tab with
// the count ("Queue 2"). Features as a tab is covered by tests/panel_redesign_m1.test.mjs and the
// existing FeaturesView tests. Loads the qs-free QueueView and PanelTabBar from quickshell/ by URL.
Item {
    id: fixture
    width: 728
    height: 600

    Item {
        id: area
        anchors.fill: parent
    }

    TestCase {
        name: "PanelQueue"
        when: windowShown

        property var created: []

        function twoGoals() {
            return [
                { id: "q-1", goal: "Add a dark mode toggle to the settings page", status: "queued" },
                { id: "q-2", goal: "Fix the login redirect loop", status: "queued" }
            ]
        }

        // Same rule as the old Panel.canMoveQueueGoal: only queued goals can swap places.
        function movable(queue) {
            return function (index, step) {
                for (let i = index + step; i >= 0 && i < queue.length; i += step) {
                    if (queue[i].status !== "done") return queue[i].status === "queued"
                }
                return false
            }
        }

        function make(name, props) {
            const comp = Qt.createComponent("../../quickshell/" + name + ".qml")
            if (comp.status !== Component.Ready)
                fail(name + ".qml cannot be loaded: " + comp.errorString())
            const obj = comp.createObject(area, Object.assign({}, Fx.palette, props))
            if (!obj) fail(name + ".qml rejected the contract properties: " + comp.errorString())
            created.push(obj)
            return obj
        }

        function makeQueue(extra) {
            const queue = twoGoals()
            return make("QueueView", Object.assign({
                width: 728, height: 600, queue: queue, engineOnline: true, startEnabled: true,
                canMove: movable(queue)
            }, extra || {}))
        }

        function cleanup() {
            for (let i = 0; i < created.length; ++i) created[i].destroy()
            created = []
            wait(10)
        }

        function rowsOf(view) {
            return Fx.walk(view, "queueRow").sort((a, b) => a.mapToItem(area, 0, 0).y - b.mapToItem(area, 0, 0).y)
        }

        function within(row, name) {
            const found = Fx.walk(row, name)
            compare(found.length, 1, "each queue row must contain exactly one " + name)
            return found[0]
        }

        function test_S18_queue_lists_both_goals_with_start_queue_and_row_controls() {
            const view = makeQueue()
            wait(30)
            const start = Fx.walk(view, "startQueue")
            compare(start.length, 1)
            verify(start[0].visible)
            verify(Fx.allText(start[0]).indexOf("Start queue") >= 0)

            const rows = rowsOf(view)
            compare(rows.length, 2)
            verify(Fx.allText(rows[0]).indexOf("Add a dark mode toggle to the settings page") >= 0)
            verify(Fx.allText(rows[1]).indexOf("Fix the login redirect loop") >= 0)
            for (const row of rows) {
                verify(row.visible)
                for (const name of ["queueUp", "queueDown", "queueRemove"])
                    verify(within(row, name).visible, name + " must be visible on a queued goal")
                verify(Fx.allText(within(row, "queueUp")).indexOf("↑") >= 0)
                verify(Fx.allText(within(row, "queueDown")).indexOf("↓") >= 0)
                verify(Fx.allText(within(row, "queueRemove")).indexOf("×") >= 0)
            }
        }

        function test_S18_start_queue_emits_startRequested_only_while_enabled() {
            const view = makeQueue()
            const started = []
            view.startRequested.connect(() => started.push(true))
            wait(30)
            const start = Fx.walk(view, "startQueue")[0]
            compare(start.enabled, true)
            mouseClick(start)
            compare(started.length, 1)

            view.startEnabled = false
            wait(30)
            compare(start.enabled, false)
            mouseClick(start)
            compare(started.length, 1, "a disabled Start queue must emit nothing")
        }

        function test_S18_up_and_down_emit_moveRequested_with_the_goal_id_and_direction() {
            const view = makeQueue()
            const moves = []
            view.moveRequested.connect((id, dir) => moves.push([id, dir]))
            wait(30)
            const rows = rowsOf(view)
            mouseClick(within(rows[1], "queueUp"))
            compare(moves, [["q-2", "up"]])
            mouseClick(within(rows[0], "queueDown"))
            compare(moves, [["q-2", "up"], ["q-1", "down"]])
        }

        function test_S18_the_first_goal_cannot_move_up_and_the_last_cannot_move_down() {
            const view = makeQueue()
            const moves = []
            view.moveRequested.connect((id, dir) => moves.push([id, dir]))
            wait(30)
            const rows = rowsOf(view)
            compare(within(rows[0], "queueUp").enabled, false)
            compare(within(rows[1], "queueDown").enabled, false)
            compare(within(rows[0], "queueDown").enabled, true)
            compare(within(rows[1], "queueUp").enabled, true)
            mouseClick(within(rows[0], "queueUp"))
            mouseClick(within(rows[1], "queueDown"))
            compare(moves.length, 0, "moves that canMove refuses must emit nothing")
        }

        function test_S18_remove_emits_removeRequested_with_the_goal_id() {
            const view = makeQueue()
            const removed = []
            view.removeRequested.connect(id => removed.push(id))
            wait(30)
            const rows = rowsOf(view)
            mouseClick(within(rows[0], "queueRemove"))
            mouseClick(within(rows[1], "queueRemove"))
            compare(removed, ["q-1", "q-2"])
        }

        function test_S18_an_offline_engine_disables_every_queue_control() {
            const view = makeQueue({ engineOnline: false, startEnabled: false })
            const events = []
            view.startRequested.connect(() => events.push("start"))
            view.moveRequested.connect(() => events.push("move"))
            view.removeRequested.connect(() => events.push("remove"))
            wait(30)
            const rows = rowsOf(view)
            for (const row of rows)
                for (const name of ["queueUp", "queueDown", "queueRemove"])
                    compare(within(row, name).enabled, false, name + " needs the engine online")
            mouseClick(Fx.walk(view, "startQueue")[0])
            mouseClick(within(rows[0], "queueRemove"))
            compare(events.length, 0)
        }

        function test_S18_a_blocked_goal_can_only_be_removed() {
            const queue = [
                { id: "q-1", goal: "Add a dark mode toggle", status: "blocked" },
                { id: "q-2", goal: "Fix the login redirect loop", status: "queued" }
            ]
            const view = make("QueueView", { width: 728, height: 600, queue: queue, engineOnline: true,
                startEnabled: true, canMove: movable(queue) })
            wait(30)
            const first = rowsOf(view)[0]
            verify(within(first, "queueRemove").visible, "a blocked goal can be removed")
            const up = Fx.walk(first, "queueUp").filter(b => b.visible)
            const down = Fx.walk(first, "queueDown").filter(b => b.visible)
            compare(up.length + down.length, 0, "only queued goals show the move arrows")
        }

        function test_S18_the_queue_tab_label_shows_the_count() {
            const bar = make("PanelTabBar", { width: 760, currentTab: "overview", queueCount: 2 })
            const tab = Fx.walk(bar, "tab_queue")
            compare(tab.length, 1)
            compare(tab[0].label, "Queue 2")
            verify(Fx.allText(tab[0]).indexOf("Queue 2") >= 0)
            compare(Fx.walk(bar, "tab_overview")[0].label, "Overview")

            bar.queueCount = 0
            wait(20)
            compare(Fx.walk(bar, "tab_queue")[0].label, "Queue")
        }
    }
}
