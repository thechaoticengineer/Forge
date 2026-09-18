import QtQuick
import QtTest
import "PanelFixtures.js" as Fx

// Panel redesign M2, compact Plan list. Covers S12: a plan with committed, running and pending
// stages shows one line per stage (status icon, "N. title", commit hash or status, duration) and
// a one-line plan review strip that summarises the verdict and round. Clicking a row opens that
// stage (stageOpened), clicking the strip asks for the plan review details (planReviewRequested).
// Nothing about routing or review policy is shown on the list. Loads the qs-free PlanStageList
// from quickshell/ by URL.
//
// Contract: required stages (array), selectedIndex (int), now (real), planReview (var); optional
// activityText (the current step, shown for the in-progress stage) and summaryText; the palette
// properties OverviewView takes; signals stageOpened(int index) and planReviewRequested().
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

    // The tallest text of a row is 12 px: a one-line row is shorter than two such lines.
    Text {
        id: metric
        visible: false
        text: "Ag"
        font.family: "monospace"
        font.pixelSize: 12
    }

    TestCase {
        name: "PanelPlanList"
        when: windowShown

        property var created: []
        readonly property real now: 1789752000

        function make(props) {
            const comp = Qt.createComponent("../../quickshell/PlanStageList.qml")
            if (comp.status !== Component.Ready)
                fail("PlanStageList.qml cannot be loaded: " + comp.errorString())
            const obj = comp.createObject(area, Object.assign({ width: 728, height: 600 }, Fx.palette, props))
            if (!obj) fail("PlanStageList.qml rejected the contract properties: " + comp.errorString())
            created.push(obj)
            return obj
        }

        function cleanup() {
            for (let i = 0; i < created.length; ++i) created[i].destroy()
            created = []
            wait(10)
        }

        // Committed stages carry a short sha and a duration, the running one a start time, exactly
        // as the engine reports them (see OverviewStageRow: sha, duration_secs, started_unix).
        function planStages() {
            return [
                Fx.stage(1, "Executable M2 scenario tests", "committed", { sha: "a1b2c3d", duration_secs: 312 }),
                Fx.stage(2, "Pure helpers and compact quota", "committed", { sha: "b2c3d4e", duration_secs: 604 }),
                Fx.stage(3, "StageDetailPage component", "in_progress", { started_unix: now - 125 }),
                Fx.stage(4, "Wire the panel", "pending"),
                Fx.stage(5, "Visual verification and docs", "pending")
            ]
        }

        function review(extra) {
            return Object.assign({
                status: "approved", rounds: 2, budget: 3, attempt_id: "attempt-1",
                gate: { status: "approved", roles: { architect: "approved", reviewer: "approved" } }
            }, extra || {})
        }

        function props(extra) {
            return Object.assign({
                stages: planStages(), selectedIndex: 2, now: now, planReview: review(),
                activityText: "implementing", summaryText: "5 stages · 2 committed · review: per plan"
            }, extra || {})
        }

        function rowsOf(view) {
            return Fx.walk(view, "planStageRow").filter(o => o.visible)
                .sort((a, b) => a.mapToItem(area, 0, 0).y - b.mapToItem(area, 0, 0).y)
        }

        function strips(view) {
            return Fx.walk(view, "planReviewStrip").filter(o => o.visible)
        }

        function assertOneLine(item, what) {
            verify(item.height < 2 * metric.height,
                what + " must be one line high, got " + item.height + " (a text line is " + metric.height + ")")
            const lines = Fx.collect(item, o => typeof o.lineCount === "number")
            verify(lines.length > 0, what + " must draw text")
            for (const line of lines) verify(line.lineCount <= 1, what + " must be a single text line")
        }

        // ---- S12 ------------------------------------------------------------------------

        function test_S12_there_is_one_visible_stage_row_per_stage_in_plan_order() {
            const view = make(props())
            wait(30)
            const rows = rowsOf(view)
            compare(rows.length, 5, "one planStageRow per stage")
            const titles = planStages().map(s => s.title)
            for (let i = 0; i < rows.length; ++i) {
                const text = Fx.allText(rows[i])
                verify(text.indexOf(String(i + 1) + ". " + titles[i]) >= 0,
                    "row " + (i + 1) + " shows its number and title\n" + text)
            }
        }

        function test_S12_every_stage_row_is_a_single_line() {
            const view = make(props())
            wait(30)
            for (const row of rowsOf(view)) assertOneLine(row, row.objectName)
        }

        function test_S12_committed_rows_show_the_commit_hash_and_the_duration() {
            const view = make(props())
            wait(30)
            const rows = rowsOf(view)
            const first = Fx.allText(rows[0])
            verify(first.indexOf("a1b2c3d") >= 0, "a committed stage shows its commit hash\n" + first)
            verify(first.indexOf("5m 12s") >= 0, "a committed stage shows its duration\n" + first)
            const second = Fx.allText(rows[1])
            verify(second.indexOf("b2c3d4e") >= 0, second)
            verify(second.indexOf("10m 4s") >= 0, second)
            verify(first.indexOf("committed") < 0, "the hash replaces the status word\n" + first)
        }

        function test_S12_the_running_row_shows_the_current_step_and_the_elapsed_time() {
            const view = make(props())
            wait(30)
            const running = Fx.allText(rowsOf(view)[2])
            verify(running.indexOf("implementing") >= 0, "the running stage shows the step\n" + running)
            verify(running.indexOf("2m 5s") >= 0, "the running stage shows the elapsed time\n" + running)
        }

        function test_S12_pending_rows_show_their_status_and_no_duration() {
            const view = make(props())
            wait(30)
            const rows = rowsOf(view)
            for (const index of [3, 4]) {
                const text = Fx.allText(rows[index])
                verify(text.indexOf("pending") >= 0, "a pending stage shows its status\n" + text)
                verify(!/\d+m \d+s/.test(text), "a pending stage has no duration\n" + text)
            }
        }

        function test_S12_the_list_shows_no_routing_or_review_policy() {
            const view = make(props())
            wait(30)
            const text = Fx.allText(view)
            verify(!/routing/i.test(text), "the list must not mention routing\n" + text)
            verify(!/review policy/i.test(text), "the list must not show the review policy\n" + text)
            verify(!/model agreement/i.test(text), "the list must not show the model agreement\n" + text)
            verify(!/\btier\b/i.test(text), "the list must not show the model tier\n" + text)
            verify(!/acceptance/i.test(text), "the list must not show acceptance criteria\n" + text)
        }

        function test_S12_the_summary_line_is_shown_when_given() {
            const view = make(props())
            wait(30)
            verify(Fx.allText(view).indexOf("5 stages · 2 committed · review: per plan") >= 0)
        }

        function test_S12_a_single_one_line_review_strip_shows_the_verdict_and_round() {
            const view = make(props())
            wait(30)
            const found = strips(view)
            compare(found.length, 1, "exactly one visible planReviewStrip")
            const text = Fx.allText(found[0])
            verify(text.indexOf("approved") >= 0, "the strip shows the verdict\n" + text)
            verify(text.indexOf("round 2 of 4") >= 0, "the strip shows the round\n" + text)
            verify(/architect/i.test(text), "the strip names the architect\n" + text)
            verify(/independent/i.test(text), "the strip names the independent reviewer\n" + text)
            assertOneLine(found[0], "planReviewStrip")
        }

        function test_S12_the_review_strip_follows_the_plan_review() {
            const view = make(props({ planReview: review({ status: "changes_requested", rounds: 1,
                gate: { status: "blocked", roles: { architect: "changes_requested", reviewer: "approved" } } }) }))
            wait(30)
            const text = Fx.allText(strips(view)[0])
            verify(text.indexOf("round 1 of 4") >= 0, "the strip shows the round\n" + text)
            verify(/changes[ _]requested/.test(text), "the strip shows the verdict\n" + text)
        }

        function test_S12_there_is_no_review_strip_without_a_plan_review() {
            const view = make(props({ planReview: null }))
            wait(30)
            compare(strips(view).length, 0, "no plan review, no strip")
            compare(rowsOf(view).length, 5, "the stage rows stay")
        }

        function test_S12_clicking_the_review_strip_asks_for_the_plan_review_details() {
            const view = make(props())
            const requested = []
            const opened = []
            view.planReviewRequested.connect(() => requested.push(true))
            view.stageOpened.connect(index => opened.push(index))
            wait(30)
            mouseClick(strips(view)[0])
            compare(requested.length, 1, "the strip emits planReviewRequested")
            compare(opened.length, 0, "the strip does not open a stage")
        }

        function test_S12_clicking_a_stage_row_opens_that_stage() {
            const view = make(props())
            const opened = []
            view.stageOpened.connect(index => opened.push(index))
            wait(30)
            const rows = rowsOf(view)
            mouseClick(rows[2])
            compare(opened, [2], "clicking row 3 emits stageOpened(2)")
            mouseClick(rows[0])
            compare(opened, [2, 0])
            mouseClick(rows[4])
            compare(opened, [2, 0, 4])
        }
    }
}
