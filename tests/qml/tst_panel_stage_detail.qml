import QtQuick
import QtTest
import "PanelFixtures.js" as Fx

// Panel redesign M2, stage detail page. Covers S13 (a breadcrumb back to Plan, previous / next
// stage, a status line and the sub-tabs Instructions / Acceptance / Review / Routing / Output,
// which together hold what the inline expanded stage showed) and S14 (the keys [ ] h l Escape
// and q act on the page and leave through signals). Loads the qs-free StageDetailPage from
// quickshell/ by URL.
//
// Contract: required stages (array), stageIndex (int), subTab (string), stageSnapshot (var),
// stageReviewBlocks (var), stageRoutingExpanded (bool), agentNow (real), editingPlan (bool);
// function-valued properties reviewView, stageDetailScope, stageActivity,
// stageReviewIncompleteRange and ensureStageReviewsLoaded; the palette properties OverviewView
// takes; signals backRequested(), stageStepRequested(int step), subTabRequested(string id),
// loadStageReviews(int stageId, var cursor, var end), retryStageReviews(var stage),
// stageRoutingExpandedRequested(bool expanded), diffRequested(), liveOutputRequested(),
// copyRequested(string original), leaveRequested(), detailRevealed(var control),
// detailInspected(var control); and the function handleKey(event) returning "handled" or "".
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

    // A key event as the panel's key handler hands it to a view.
    Component {
        id: eventComponent
        QtObject {
            property int key: 0
            property int modifiers: Qt.NoModifier
            property string text: ""
            property bool accepted: false
            property bool isAutoRepeat: false
        }
    }

    TestCase {
        name: "PanelStageDetail"
        when: windowShown

        readonly property var tabs: ["instructions", "acceptance", "review", "routing", "output"]
        readonly property real now: 1789752000
        readonly property string instructions: "Split ActivityView out of Panel.qml and keep every existing key."
        readonly property string acceptance: "qmltestrunner loads ActivityView on its own."
        readonly property string commitMessage: "refactor(panel): move Activity into its own view"
        readonly property string policyRationale: "Standard risk: one fresh independent review is enough."

        property var created: []
        property var ensured: []

        function cleanup() {
            for (let i = 0; i < created.length; ++i) created[i].destroy()
            created = []
            ensured = []
            wait(10)
        }

        function planStages() {
            return [
                Fx.stage(1, "Executable M2 scenario tests", "committed", { sha: "a1b2c3d", duration_secs: 312 }),
                Fx.stage(2, "Pure helpers and compact quota", "committed", { sha: "b2c3d4e", duration_secs: 604 }),
                Fx.stage(3, "Activity, Architecture and Queue tabs", "in_progress", {
                    started_unix: now - 125,
                    review_policy: { scope: "stage", rationale: policyRationale },
                    usage: { claude: { total_tokens: 12300, input_tokens: 9000, output_tokens: 3300, calls: 4 } }
                }),
                Fx.stage(4, "Settings tab and Plan tab", "pending"),
                Fx.stage(5, "Overview view and final Panel.qml reduction", "pending")
            ]
        }

        function scopeOf(stage) {
            return JSON.stringify(["/home/user/Projects/forge", 1, "plan-1", 1, stage.id])
        }

        // The snapshot Panel.qml captures when a stage is opened (captureStageSnapshot).
        function snapshotOf(stage) {
            return {
                key: scopeOf(stage), commit: commitMessage, instructions: instructions,
                acceptance: acceptance, policyRationale: policyRationale,
                rationale: [{ label: "Planner", text: "Standard risk" },
                    { label: "Architect", text: "Preserve interfaces" }],
                diagnostics: [{ label: "Risk", text: "pending · complexity: pending" }]
            }
        }

        function verdict(extra) {
            return Object.assign({ role: "reviewer", approved: false, round: 1, unix: 1789750000,
                summary: "The plan misses a regression test.",
                issues: ["Add a regression test for the stage keys."], notes: [], checks: [] }, extra || {})
        }

        function reviewState(stage, extra) {
            const rows = [{ key: "position:0", position: 0, verdict: verdict(), round: 1, complete: true }]
            return Object.assign({
                scope: { key: scopeOf(stage), count: 1, project: "/home/user/Projects/forge", stage: stage.id },
                pending: null, error: "", retry: null, rows: rows, older: 0
            }, extra || {})
        }

        function props(extra) {
            const stages = planStages()
            const state = (extra && extra.reviewState) || null
            const built = Object.assign({
                stages: stages, stageIndex: 2, subTab: "instructions",
                stageSnapshot: snapshotOf(stages[2]), stageReviewBlocks: ({}),
                stageRoutingExpanded: false, agentNow: now, editingPlan: false,
                reviewView: function (stage) { return state || reviewState(stage) },
                stageDetailScope: scopeOf,
                stageActivity: function (stage) {
                    return stage.status === "in_progress" ? "now: implementing · round 1" : ""
                },
                stageReviewIncompleteRange: function (view) { return null },
                ensureStageReviewsLoaded: function (stage) { ensured.push(stage.id) }
            }, extra || {})
            delete built.reviewState
            return built
        }

        function make(extra) {
            const comp = Qt.createComponent("../../quickshell/StageDetailPage.qml")
            if (comp.status !== Component.Ready)
                fail("StageDetailPage.qml cannot be loaded: " + comp.errorString())
            const obj = comp.createObject(area, Object.assign({ width: 728, height: 600 },
                Fx.palette, props(extra)))
            if (!obj) fail("StageDetailPage.qml rejected the contract properties: " + comp.errorString())
            created.push(obj)
            return obj
        }

        function one(view, name) {
            const found = Fx.walk(view, name).filter(o => o.visible)
            compare(found.length, 1, "StageDetailPage must show exactly one " + name)
            return found[0]
        }

        function content(view, id) {
            return one(view, "stageSubTabContent_" + id)
        }

        function keyEvent(key, text, modifiers) {
            return eventComponent.createObject(fixture,
                { key: key, text: text || "", modifiers: modifiers || Qt.NoModifier })
        }

        // Sends `event` to handleKey and returns {result, events} where events records every signal.
        function press(view, key, text) {
            const events = []
            view.backRequested.connect(() => events.push(["back"]))
            view.stageStepRequested.connect(step => events.push(["step", step]))
            view.subTabRequested.connect(id => events.push(["tab", id]))
            const event = keyEvent(key, text)
            const result = view.handleKey(event)
            event.destroy()
            return { result: result, events: events }
        }

        // ---- S13: header ----------------------------------------------------------------

        function test_S13_the_breadcrumb_shows_Plan_and_the_stage_number_and_title() {
            const view = make()
            wait(30)
            const text = Fx.allText(one(view, "stageBreadcrumb"))
            verify(text.indexOf("Plan") >= 0, "the breadcrumb names Plan\n" + text)
            verify(text.indexOf("3. Activity, Architecture and Queue tabs") >= 0,
                "the breadcrumb shows the stage number and title\n" + text)
        }

        function test_S13_clicking_Plan_in_the_breadcrumb_goes_back() {
            const view = make()
            const back = []
            view.backRequested.connect(() => back.push(true))
            wait(30)
            const crumb = one(view, "stageBreadcrumb")
            // The Plan part: a text that names Plan but not the stage title, else the left edge.
            const parts = Fx.collect(crumb, o => o.visible && typeof o.text === "string"
                && o.text.indexOf("Plan") >= 0 && o.text.indexOf("Activity") < 0)
            const at = parts.length > 0 ? parts[0].mapToItem(crumb, parts[0].width / 2, parts[0].height / 2)
                : { x: 8, y: crumb.height / 2 }
            mouseClick(crumb, at.x, at.y)
            compare(back.length, 1, "clicking the Plan part emits backRequested")
        }

        function test_S13_previous_and_next_step_between_stages() {
            const view = make()
            const steps = []
            view.stageStepRequested.connect(step => steps.push(step))
            wait(30)
            const previous = one(view, "stagePrevious")
            const next = one(view, "stageNext")
            verify(previous.enabled && next.enabled, "a middle stage can step both ways")
            mouseClick(previous)
            mouseClick(next)
            compare(steps, [-1, 1])
        }

        function test_S13_previous_is_disabled_on_the_first_stage_and_next_on_the_last() {
            const view = make({ stageIndex: 0 })
            const steps = []
            view.stageStepRequested.connect(step => steps.push(step))
            wait(30)
            compare(one(view, "stagePrevious").enabled, false, "no stage before the first")
            compare(one(view, "stageNext").enabled, true)
            mouseClick(one(view, "stagePrevious"))
            compare(steps.length, 0, "a disabled previous emits nothing")
            view.stageIndex = 4
            wait(30)
            compare(one(view, "stageNext").enabled, false, "no stage after the last")
            compare(one(view, "stagePrevious").enabled, true)
            mouseClick(one(view, "stageNext"))
            compare(steps.length, 0, "a disabled next emits nothing")
        }

        function test_S13_the_status_line_shows_the_status_and_the_duration() {
            const view = make()
            wait(30)
            const running = Fx.allText(one(view, "stageStatusLine"))
            verify(/implementing|in.progress/.test(running), "the running stage shows its status\n" + running)
            verify(running.indexOf("2m 5s") >= 0, "the running stage shows the elapsed time\n" + running)
            view.stageIndex = 0
            wait(30)
            const committed = Fx.allText(one(view, "stageStatusLine"))
            verify(committed.indexOf("committed") >= 0, "a committed stage shows its status\n" + committed)
            verify(committed.indexOf("a1b2c3d") >= 0, "a committed stage shows its hash\n" + committed)
            verify(committed.indexOf("5m 12s") >= 0, "a committed stage shows its duration\n" + committed)
        }

        function test_S13_the_status_line_explains_a_blocked_stage() {
            const stages = planStages()
            stages[2] = Fx.stage(3, "Activity, Architecture and Queue tabs", "blocked",
                { review_gate: { status: "exhausted", roles: { architect: "not_required", reviewer: "changes_requested" } } })
            const view = make({ stages: stages })
            wait(30)
            const text = Fx.allText(one(view, "stageStatusLine"))
            verify(text.indexOf("blocked · fix rounds exhausted") >= 0, "the blocked reason is shown\n" + text)
        }

        // ---- S13: sub-tabs --------------------------------------------------------------

        function test_S13_five_sub_tab_buttons_follow_the_stageDetailTabs_order() {
            const view = make()
            wait(30)
            const buttons = Fx.walkPrefix(view, "stageSubTab_").filter(o => o.visible
                && o.objectName.indexOf("stageSubTabContent_") !== 0)
            compare(buttons.length, 5, "five sub-tab buttons")
            const ordered = buttons.sort((a, b) => a.mapToItem(area, 0, 0).x - b.mapToItem(area, 0, 0).x)
                .map(b => b.objectName.substring("stageSubTab_".length))
            compare(ordered, tabs)
            const text = Fx.allText(view).toLowerCase()
            for (const label of ["instructions", "acceptance", "review", "routing", "output"])
                verify(text.indexOf(label) >= 0, "the sub-tab " + label + " is labelled")
        }

        function test_S13_clicking_a_sub_tab_requests_it() {
            const view = make()
            const requested = []
            view.subTabRequested.connect(id => requested.push(id))
            wait(30)
            for (const id of tabs) {
                mouseClick(one(view, "stageSubTab_" + id))
                compare(requested[requested.length - 1], id, "clicking " + id)
            }
            compare(requested, tabs)
        }

        function test_S13_only_the_content_of_the_current_sub_tab_is_visible() {
            const view = make()
            wait(30)
            for (const id of tabs) {
                view.subTab = id
                wait(30)
                const shown = Fx.walkPrefix(view, "stageSubTabContent_").filter(o => o.visible)
                    .map(o => o.objectName)
                compare(shown, ["stageSubTabContent_" + id], "only " + id + " is shown")
            }
        }

        // ---- S13: what the inline expansion showed ----------------------------------------

        function test_S13_instructions_shows_the_instructions_and_the_commit_message() {
            const view = make({ subTab: "instructions" })
            wait(30)
            const text = Fx.allText(content(view, "instructions"))
            verify(text.indexOf(instructions) >= 0, "the instructions are shown\n" + text)
            verify(text.indexOf(commitMessage) >= 0, "the commit message is shown\n" + text)
            verify(text.indexOf(acceptance) < 0, "the acceptance criteria belong to their own tab")
        }

        function test_S13_acceptance_shows_the_acceptance_criteria() {
            const view = make({ subTab: "acceptance" })
            wait(30)
            const text = Fx.allText(content(view, "acceptance"))
            verify(text.indexOf(acceptance) >= 0, "the acceptance criteria are shown\n" + text)
            verify(text.indexOf(instructions) < 0, "the instructions belong to their own tab")
        }

        function test_S13_review_shows_the_review_policy_rationale_and_the_historical_reviews() {
            const view = make({ subTab: "review" })
            wait(30)
            const pane = content(view, "review")
            const text = Fx.allText(pane)
            verify(text.indexOf(policyRationale) >= 0, "the review policy rationale is shown\n" + text)
            const history = one(pane, "stageHistoricalReviews")
            const rounds = Fx.allText(history)
            verify(rounds.indexOf("Historical") >= 0, "the historical reviews are listed\n" + rounds)
            verify(rounds.indexOf("Add a regression test for the stage keys.") >= 0,
                "a review's change request is shown in full\n" + rounds)
        }

        function test_S13_review_loads_the_complete_reviews_of_the_shown_stage() {
            const view = make({ subTab: "review" })
            wait(60)
            verify(ensured.indexOf(3) >= 0, "the shown stage's reviews are loaded: " + JSON.stringify(ensured))
        }

        function test_S13_review_offers_older_reviews_and_requests_the_previous_page() {
            const stages = planStages()
            const state = reviewState(stages[2], { older: 3, scope: { key: scopeOf(stages[2]), count: 4,
                project: "/home/user/Projects/forge", stage: 3 } })
            const view = make({ subTab: "review", reviewState: state })
            const loads = []
            view.loadStageReviews.connect((stageId, cursor, end) => loads.push([stageId, cursor, end]))
            wait(30)
            const older = one(view, "stageReviewsOlder")
            verify(Fx.allText(older).indexOf("Load older reviews (3)") >= 0, Fx.allText(older))
            mouseClick(older)
            compare(loads, [[3, 0, 3]], "the older page is requested for stage 3")
        }

        function test_S13_review_shows_a_load_error_and_a_retry_that_asks_again() {
            const stages = planStages()
            const state = reviewState(stages[2], { error: "Unable to load complete reviews",
                retry: { cursor: 0, end: 1 } })
            const view = make({ subTab: "review", reviewState: state })
            const retries = []
            view.retryStageReviews.connect(stage => retries.push(stage.id))
            wait(30)
            const status = one(view, "stageReviewStatus")
            verify(Fx.allText(status).indexOf("Unable to load complete reviews") >= 0, Fx.allText(status))
            mouseClick(one(view, "stageReviewsRetry"))
            compare(retries, [3], "retrying asks for the shown stage")
        }

        function test_S13_review_shows_the_review_gate_of_the_stage() {
            const view = make({ subTab: "review" })
            wait(30)
            const text = Fx.allText(content(view, "review"))
            verify(text.indexOf("Review policy:") >= 0, "the review policy and gate are shown\n" + text)
            verify(text.indexOf("Independent:") >= 0, "the independent reviewer outcome is shown\n" + text)
        }

        function test_S13_routing_shows_the_model_status_the_rationale_and_the_details_toggle() {
            const view = make({ subTab: "routing" })
            wait(30)
            const pane = content(view, "routing")
            const text = Fx.allText(pane)
            verify(text.indexOf("claude/claude-sonnet-5") >= 0, "the model status line is shown\n" + text)
            verify(text.indexOf("Planner") >= 0 && text.indexOf("Standard risk") >= 0,
                "the planner rationale is shown\n" + text)
            verify(text.indexOf("Architect") >= 0 && text.indexOf("Preserve interfaces") >= 0,
                "the architect rationale is shown\n" + text)
            verify(text.indexOf("complexity: pending") < 0, "diagnostics stay collapsed\n" + text)
            const toggle = one(pane, "stageRoutingToggle")
            verify(Fx.allText(toggle).indexOf("Model agreement and routing details") >= 0)
        }

        function test_S13_the_routing_toggle_asks_to_expand_and_expanded_shows_the_diagnostics() {
            const view = make({ subTab: "routing" })
            const requested = []
            view.stageRoutingExpandedRequested.connect(expanded => requested.push(expanded))
            wait(30)
            mouseClick(one(view, "stageRoutingToggle"))
            compare(requested, [true], "collapsed, the toggle asks to expand")
            view.stageRoutingExpanded = true
            wait(30)
            const text = Fx.allText(content(view, "routing"))
            verify(text.indexOf("complexity: pending") >= 0, "the diagnostics are shown when expanded\n" + text)
            mouseClick(one(view, "stageRoutingToggle"))
            compare(requested, [true, false], "expanded, the toggle asks to collapse")
        }

        function test_S13_output_shows_the_usage_summary_and_a_live_output_control() {
            const view = make({ subTab: "output" })
            const live = []
            view.liveOutputRequested.connect(() => live.push(true))
            wait(30)
            const pane = content(view, "output")
            const text = Fx.allText(pane)
            verify(text.indexOf("claude 12.3k tok") >= 0, "the usage summary is shown\n" + text)
            const controls = Fx.collect(pane, o => o.visible && typeof o.text === "string"
                && o.text.indexOf("Live output") >= 0)
            verify(controls.length > 0, "a Live output control is shown\n" + text)
            mouseClick(controls[0])
            compare(live.length, 1, "Live output asks to open the live output")
        }

        function test_S13_output_offers_the_diff() {
            const view = make({ subTab: "output" })
            const diffs = []
            view.diffRequested.connect(() => diffs.push(true))
            wait(30)
            const controls = Fx.collect(content(view, "output"), o => o.visible && typeof o.text === "string"
                && o.text.indexOf("View diff") >= 0)
            verify(controls.length > 0, "a View diff control is shown")
            mouseClick(controls[0])
            compare(diffs.length, 1)
        }

        function test_S13_routing_shows_the_model_block_of_the_stage() {
            const stages = planStages()
            stages[2] = Fx.stage(3, "Activity, Architecture and Queue tabs", "in_progress",
                { started_unix: now - 125, model_block: "model is no longer eligible" })
            const view = make({ stages: stages, subTab: "routing" })
            wait(30)
            verify(Fx.allText(content(view, "routing")).indexOf("model is no longer eligible") >= 0,
                "a model block is shown with the routing")
        }

        function test_S13_a_stage_without_a_matching_snapshot_shows_no_stale_prose() {
            const view = make({ stageSnapshot: null })
            wait(30)
            const text = Fx.allText(view)
            verify(text.indexOf(instructions) < 0, "no snapshot, no prose\n" + text)
            verify(text.indexOf("3. Activity, Architecture and Queue tabs") >= 0, "the header still names the stage")
        }

        // ---- S14: keys ------------------------------------------------------------------

        function test_S14_the_right_bracket_asks_for_the_next_stage_and_the_left_bracket_for_the_previous() {
            const view = make()
            wait(30)
            let out = press(view, Qt.Key_BracketRight, "]")
            compare(out.result, "handled")
            compare(out.events, [["step", 1]])
            out = press(view, Qt.Key_BracketLeft, "[")
            compare(out.result, "handled")
            compare(out.events, [["step", -1]])
        }

        function test_S14_l_and_h_move_between_sub_tabs_and_wrap() {
            const view = make({ subTab: "instructions" })
            wait(30)
            let out = press(view, Qt.Key_L, "l")
            compare(out.result, "handled")
            compare(out.events, [["tab", "acceptance"]])
            out = press(view, Qt.Key_H, "h")
            compare(out.result, "handled")
            compare(out.events, [["tab", "output"]], "h from Instructions wraps to Output")
            view.subTab = "output"
            wait(10)
            out = press(view, Qt.Key_L, "l")
            compare(out.events, [["tab", "instructions"]], "l from Output wraps to Instructions")
            view.subTab = "review"
            wait(10)
            out = press(view, Qt.Key_L, "l")
            compare(out.events, [["tab", "routing"]])
            out = press(view, Qt.Key_H, "h")
            compare(out.events, [["tab", "acceptance"]])
        }

        function test_S14_Escape_and_q_go_back_to_Plan() {
            const view = make()
            wait(30)
            let out = press(view, Qt.Key_Escape, "")
            compare(out.result, "handled")
            compare(out.events, [["back"]])
            out = press(view, Qt.Key_Q, "q")
            compare(out.result, "handled")
            compare(out.events, [["back"]])
        }

        function test_S14_other_keys_are_left_to_the_panel() {
            const view = make()
            wait(30)
            for (const key of [[Qt.Key_Z, "z"], [Qt.Key_Y, "y"]]) {
                const out = press(view, key[0], key[1])
                compare(out.result, "", "the key " + key[1] + " is not the stage detail's")
                compare(out.events.length, 0)
            }
        }

        function test_S14_keys_with_modifiers_do_not_navigate() {
            const view = make()
            wait(30)
            const events = []
            view.stageStepRequested.connect(step => events.push(step))
            view.subTabRequested.connect(id => events.push(id))
            view.backRequested.connect(() => events.push("back"))
            for (const key of [[Qt.Key_L, "l"], [Qt.Key_Q, "q"], [Qt.Key_BracketRight, "]"]]) {
                const event = keyEvent(key[0], key[1], Qt.ControlModifier)
                compare(view.handleKey(event), "", "Ctrl+" + key[1] + " is not a stage detail key")
                event.destroy()
            }
            compare(events.length, 0)
        }
    }
}
