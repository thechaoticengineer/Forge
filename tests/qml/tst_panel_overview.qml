import QtQuick
import QtTest
import "PanelFixtures.js" as Fx

// Panel redesign M1, Overview view. Covers S4 (the running Overview fits one screen without
// scrolling and hides routing and review policy text), S5 (the idle Overview offers the goal
// actions and i focuses the goal) and S6 (only the actions of the current phase are shown as
// buttons and each emits its id). Loads the qs-free OverviewView from quickshell/ by URL.
//
// The panel window is 760x760; the header, tab bar and hint line leave a 728x600 content area.
Item {
    id: fixture
    width: 760
    height: 760

    Item {
        id: area
        width: 728
        height: 600
        clip: true
    }

    TestCase {
        name: "PanelOverview"
        when: windowShown

        property var created: []
        readonly property string latestLine: "ok  github.com/example/forge/panel  0.412s"

        function make(props) {
            const comp = Qt.createComponent("../../quickshell/OverviewView.qml")
            if (comp.status !== Component.Ready)
                fail("OverviewView.qml cannot be loaded: " + comp.errorString())
            const obj = comp.createObject(area, Object.assign({ width: 728, height: 600 }, Fx.palette, props))
            if (!obj) fail("OverviewView.qml rejected the contract properties: " + comp.errorString())
            created.push(obj)
            return obj
        }

        function cleanup() {
            for (let i = 0; i < created.length; ++i) created[i].destroy()
            created = []
            wait(10)
        }

        // Contract properties for an engine state in `phase` with an optional plan.
        function props(phase, plan, actions, extra) {
            const state = Fx.engineState(phase, plan)
            const running = phase === "running"
            return Object.assign({
                engineState: state, plan: plan, phase: phase, busy: phase === "running" || phase === "planning",
                agent: running ? Fx.runningAgent() : null, agentActive: running,
                agentElapsedText: running ? "4:12" : "", latestOutputText: running ? latestLine : "",
                runSummaryText: plan ? "2/5 stages committed · run 4m 12s" : "",
                actions: actions, goalEnhanceStatus: "", discussionStatus: "", discussionCount: 0
            }, extra || {})
        }

        function actionButtons(view) {
            return Fx.walkPrefix(view, "overviewAction_").filter(b => b.visible)
        }

        function actionIds(view) {
            return actionButtons(view).map(b => b.objectName.substring("overviewAction_".length)).sort()
        }

        function one(view, name) {
            const found = Fx.walk(view, name)
            compare(found.length, 1, "OverviewView must contain exactly one " + name)
            return found[0]
        }

        // ---- S4 -------------------------------------------------------------------------

        function test_S4_the_running_overview_fits_the_content_area_without_scrolling() {
            const view = make(props("running", Fx.fiveStagePlan("approved"), ["stop"]))
            wait(30)
            for (const name of ["overviewGoal", "nowWorkingCard", "progressSummary", "overviewAction_stop"])
                verify(Fx.insideArea(one(view, name), area), name + " must be visible without scrolling")
            const rows = Fx.walk(view, "overviewStageRow")
            compare(rows.length, 5, "one line per stage")
            for (const row of rows) verify(Fx.insideArea(row, area), "every stage row must be visible without scrolling")
        }

        function test_S4_the_now_working_card_names_stage_role_tool_model_elapsed_time_and_latest_output() {
            const view = make(props("running", Fx.fiveStagePlan("approved"), ["stop"]))
            wait(30)
            const text = Fx.allText(one(view, "nowWorkingCard"))
            for (const part of ["Activity, Architecture, Features and Queue tabs", "implementer", "claude",
                "claude-sonnet-5", "4:12", latestLine])
                verify(text.indexOf(part) >= 0, "the now working card must show " + part + "\n" + text)
            verify(/\b3\b/.test(text), "the now working card must show the stage number 3\n" + text)
        }

        function test_S4_the_progress_summary_is_one_line() {
            const view = make(props("running", Fx.fiveStagePlan("approved"), ["stop"]))
            wait(30)
            const summary = one(view, "progressSummary")
            verify(Fx.allText(summary).indexOf("2/5 stages committed · run 4m 12s") >= 0)
            for (const item of Fx.collect(summary, o => typeof o.lineCount === "number"))
                verify(item.lineCount <= 1, "the progress summary must stay on one line")
        }

        function test_S4_every_stage_is_a_single_line_without_routing_or_review_policy() {
            const view = make(props("running", Fx.fiveStagePlan("approved"), ["stop"]))
            wait(30)
            const plan = Fx.fiveStagePlan("approved")
            const rows = Fx.walk(view, "overviewStageRow")
                .sort((a, b) => a.mapToItem(area, 0, 0).y - b.mapToItem(area, 0, 0).y)
            compare(rows.length, 5)
            for (let i = 0; i < rows.length; ++i) {
                const text = Fx.allText(rows[i])
                verify(text.indexOf(plan.stages[i].title) >= 0, "row " + (i + 1) + " shows its title\n" + text)
                verify(text.indexOf(String(i + 1)) >= 0, "row " + (i + 1) + " shows its number\n" + text)
                for (const banned of ["Review policy", "Routing", "Execution:", "Proposed:", "Acceptance"])
                    verify(text.indexOf(banned) < 0, "a stage row must not show '" + banned + "'\n" + text)
                for (const line of Fx.collect(rows[i], o => typeof o.lineCount === "number"))
                    verify(line.lineCount <= 1, "a stage row must be a single text line")
                verify(rows[i].height < 48, "a stage row is one line high, got " + rows[i].height)
            }
        }

        function test_S4_the_stop_action_stops_the_run() {
            const view = make(props("running", Fx.fiveStagePlan("approved"), ["stop"]))
            const requested = []
            view.actionRequested.connect(id => requested.push(id))
            wait(30)
            mouseClick(one(view, "overviewAction_stop"))
            compare(requested, ["stop"])
        }

        // ---- S5 -------------------------------------------------------------------------

        function idleProps(extra) {
            return props("idle", null, ["createPlan", "discuss", "enhance", "addToQueue"], extra)
        }

        function test_S5_the_idle_overview_offers_the_goal_field_and_the_four_goal_actions() {
            const view = make(idleProps())
            view.goalField.text = "Add a dark mode toggle to the settings page"
            wait(30)
            verify(Fx.insideArea(one(view, "overviewGoal"), area), "the goal field is shown")
            for (const id of ["createPlan", "discuss", "enhance", "addToQueue"])
                verify(Fx.insideArea(one(view, "overviewAction_" + id), area), id + " must be visible")
            compare(actionIds(view), ["addToQueue", "createPlan", "discuss", "enhance"])
            verify(Fx.allText(one(view, "overviewAction_createPlan")).indexOf("Create plan") >= 0)
            verify(Fx.allText(one(view, "overviewAction_discuss")).indexOf("Discuss") >= 0)
            verify(Fx.allText(one(view, "overviewAction_enhance")).indexOf("Enhance with AI") >= 0)
            verify(Fx.allText(one(view, "overviewAction_addToQueue")).indexOf("Add to queue") >= 0)
        }

        function test_S5_focusGoal_puts_the_cursor_in_the_goal_field() {
            const view = make(idleProps())
            wait(30)
            compare(view.goalField.activeFocus, false)
            view.focusGoal()
            tryVerify(() => view.goalField.activeFocus, 1000, "focusGoal() must focus the goal TextEdit")
        }

        function test_S5_the_idle_overview_shows_the_last_run_and_no_now_working_card() {
            const view = make(idleProps({ runSummaryText: "Last run · 5/5 stages committed · 12m 4s" }))
            wait(30)
            const card = one(view, "lastRunCard")
            verify(card.visible, "the idle Overview shows a short card for the last run")
            verify(Fx.allText(card).indexOf("5/5 stages committed") >= 0)
            verify(!one(view, "nowWorkingCard").visible, "nothing is running")
        }

        function test_S5_goal_enhancement_and_discussion_status_stay_visible_on_overview() {
            const view = make(idleProps({ goalEnhanceStatus: "enhancing the description…",
                discussionStatus: "Forge is replying…", discussionCount: 2 }))
            wait(30)
            const text = Fx.allText(view)
            verify(text.indexOf("enhancing the description…") >= 0, "the goal enhancement status is shown")
            verify(text.indexOf("Forge is replying…") >= 0, "the discussion status is shown")
            verify(Fx.allText(one(view, "overviewAction_discuss")).indexOf("2") >= 0,
                "the discussion message count is shown on the Discuss action")
        }

        // ---- S6 -------------------------------------------------------------------------

        // Actions per phase, as PanelActions.overviewActions returns them for realistic flags.
        readonly property var phaseActions: [
            { phase: "idle", plan: null, actions: ["createPlan", "discuss", "enhance", "addToQueue"] },
            { phase: "planning", plan: null, actions: [] },
            { phase: "plan_ready", plan: "draft", actions: ["approve", "editPlan"] },
            { phase: "awaiting_approval", plan: "draft", actions: ["approve", "editPlan"] },
            { phase: "running", plan: "approved", actions: ["stop"] },
            { phase: "blocked", plan: "approved", actions: ["run", "editPlan"] },
            { phase: "failed", plan: "approved", actions: ["run", "editPlan"] },
            { phase: "done", plan: "done", actions: ["run", "editPlan"] }
        ]
        readonly property var everyAction: ["createPlan", "discuss", "enhance", "addToQueue", "approve",
            "editPlan", "run", "stop"]

        function test_S6_only_the_actions_of_the_phase_are_shown_as_buttons() {
            for (const entry of phaseActions) {
                const plan = entry.plan ? Fx.fiveStagePlan(entry.plan) : null
                const view = make(props(entry.phase, plan, entry.actions))
                wait(30)
                compare(actionIds(view), entry.actions.slice().sort(), "buttons shown in " + entry.phase)
                for (const id of everyAction.filter(id => entry.actions.indexOf(id) < 0))
                    compare(Fx.walk(view, "overviewAction_" + id).filter(b => b.visible).length, 0,
                        id + " must not be shown in " + entry.phase)
                view.destroy()
                created = []
            }
        }

        function test_S6_the_shown_buttons_follow_the_actions_property_when_the_phase_changes() {
            const view = make(props("idle", null, ["createPlan", "discuss", "enhance", "addToQueue"]))
            wait(30)
            compare(actionIds(view), ["addToQueue", "createPlan", "discuss", "enhance"])
            view.phase = "running"
            view.busy = true
            view.actions = ["stop"]
            wait(30)
            compare(actionIds(view), ["stop"])
            view.phase = "awaiting_approval"
            view.busy = false
            view.actions = ["approve", "editPlan"]
            wait(30)
            compare(actionIds(view), ["approve", "editPlan"])
        }

        function test_S6_clicking_a_shown_action_requests_that_action() {
            for (const entry of phaseActions.filter(e => e.actions.length > 0)) {
                const plan = entry.plan ? Fx.fiveStagePlan(entry.plan) : null
                const view = make(props(entry.phase, plan, entry.actions))
                const requested = []
                view.actionRequested.connect(id => requested.push(id))
                wait(30)
                for (const id of entry.actions) {
                    mouseClick(one(view, "overviewAction_" + id))
                    compare(requested[requested.length - 1], id, "clicking " + id + " in " + entry.phase)
                }
                compare(requested, entry.actions)
                view.destroy()
                created = []
            }
        }

        function test_S6_the_action_labels_are_the_old_button_labels() {
            const expected = { approve: "approve", editPlan: "Edit plan", run: "Start", stop: "Stop" }
            const view = make(props("awaiting_approval", Fx.fiveStagePlan("draft"), ["approve", "editPlan"]))
            wait(30)
            verify(Fx.allText(one(view, "overviewAction_approve")).toLowerCase().indexOf(expected.approve) >= 0)
            verify(Fx.allText(one(view, "overviewAction_editPlan")).indexOf(expected.editPlan) >= 0)
            view.actions = ["run", "editPlan"]
            wait(30)
            verify(Fx.allText(one(view, "overviewAction_run")).indexOf(expected.run) >= 0)
        }
    }
}
