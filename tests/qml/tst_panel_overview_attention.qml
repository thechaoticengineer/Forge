import QtQuick
import QtTest
import "PanelFixtures.js" as Fx

// Panel redesign M2, Overview attention. Covers S7: a blocked or failed stage is shown at the top
// of Overview, under the goal and above the now-working card and the stage rows, with its number,
// title, status and reason, in the idle and in the busy phases; clicking it, or clicking a stage
// row, emits stageRequested(index), which Panel.qml maps to the stage detail page. Loads the
// qs-free OverviewView from quickshell/ by URL, as tst_panel_overview.qml does.
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
        name: "PanelOverviewAttention"
        when: windowShown

        property var created: []
        readonly property int attentionStage: 4
        readonly property string attentionTitle: "Settings tab and Plan tab"

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

        // A five stage plan whose fourth stage is `status`; a blocked stage has exhausted its fix
        // rounds, the state the engine reports as review_gate.status "exhausted".
        function planWith(status) {
            const plan = Fx.fiveStagePlan("approved")
            plan.stages[3] = Fx.stage(attentionStage, attentionTitle, status, status === "blocked"
                ? { review_gate: { status: "exhausted",
                    roles: { architect: "not_required", reviewer: "changes_requested" } } } : {})
            return plan
        }

        function props(phase, plan, actions) {
            const state = Fx.engineState(phase, plan)
            const running = phase === "running"
            return {
                engineState: state, plan: plan, phase: phase, busy: phase === "running" || phase === "planning",
                agent: running ? Fx.runningAgent() : null, agentActive: running,
                agentElapsedText: running ? "4:12" : "",
                latestOutputText: running ? "ok  github.com/example/forge/panel  0.412s" : "",
                runSummaryText: "2/5 stages committed · run 4m 12s",
                actions: actions, goalEnhanceStatus: "", discussionStatus: "", discussionCount: 0
            }
        }

        // Idle and busy phases, each with a blocked and a failed stage.
        readonly property var cases: [
            { phase: "idle", status: "blocked", actions: ["run", "editPlan"] },
            { phase: "blocked", status: "blocked", actions: ["run", "editPlan"] },
            { phase: "failed", status: "failed", actions: ["run", "editPlan"] },
            { phase: "running", status: "blocked", actions: ["stop"] },
            { phase: "running", status: "failed", actions: ["stop"] }
        ]

        function label(entry) {
            return entry.phase + " phase, " + entry.status + " stage"
        }

        function attention(view, what) {
            const found = Fx.walk(view, "overviewAttention").filter(o => o.visible)
            compare(found.length, 1, "exactly one visible overviewAttention in the " + what)
            return found[0]
        }

        function yIn(item) {
            return item.mapToItem(area, 0, 0).y
        }

        // ---- S7 -------------------------------------------------------------------------

        function test_S7_a_blocked_or_failed_stage_is_shown_in_every_phase() {
            for (const entry of cases) {
                const view = make(props(entry.phase, planWith(entry.status), entry.actions))
                wait(30)
                verify(attention(view, label(entry)).visible)
                view.destroy()
                created = []
            }
        }

        function test_S7_the_attention_stage_is_the_first_content_under_the_goal() {
            for (const entry of cases) {
                const view = make(props(entry.phase, planWith(entry.status), entry.actions))
                wait(30)
                const item = attention(view, label(entry))
                const goal = Fx.walk(view, "overviewGoal")[0]
                verify(yIn(item) >= yIn(goal) + goal.height - 0.5,
                    "the attention stage is below the goal in the " + label(entry))
                for (const row of Fx.walk(view, "overviewStageRow").filter(o => o.visible))
                    verify(yIn(item) <= yIn(row), "the attention stage is above every stage row in the " + label(entry))
                for (const card of Fx.walk(view, "nowWorkingCard").filter(o => o.visible))
                    verify(yIn(item) <= yIn(card), "the attention stage is above the now-working card in the " + label(entry))
                verify(Fx.insideArea(item, area), "the attention stage needs no scrolling in the " + label(entry))
                view.destroy()
                created = []
            }
        }

        function test_S7_the_attention_stage_shows_number_title_status_and_reason() {
            for (const entry of cases) {
                const view = make(props(entry.phase, planWith(entry.status), entry.actions))
                wait(30)
                const text = Fx.allText(attention(view, label(entry)))
                const what = "in the " + label(entry) + "\n" + text
                verify(text.indexOf(String(attentionStage)) >= 0, "the stage number is shown " + what)
                verify(text.indexOf(attentionTitle) >= 0, "the title is shown " + what)
                verify(text.indexOf(entry.status) >= 0, "the status is shown " + what)
                if (entry.status === "blocked")
                    verify(text.indexOf("fix rounds exhausted") >= 0, "the blocked reason is shown " + what)
                // Whatever is left once number, title, status and filler words are removed is the reason.
                const reason = text.toLowerCase().split(attentionTitle.toLowerCase()).join(" ")
                    .replace(/blocked|failed|stage|plan|[0-9!·—›.\-]/g, " ").replace(/\s+/g, " ").trim()
                verify(reason.length >= 6, "a reason is shown besides the status " + what)
                view.destroy()
                created = []
            }
        }

        function test_S7_clicking_the_attention_stage_opens_its_detail() {
            for (const entry of cases) {
                const view = make(props(entry.phase, planWith(entry.status), entry.actions))
                const requested = []
                view.stageRequested.connect(index => requested.push(index))
                wait(30)
                mouseClick(attention(view, label(entry)))
                compare(requested, [3], "clicking the attention stage requests stage 4 (index 3) in the " + label(entry))
                view.destroy()
                created = []
            }
        }

        function test_S7_no_stage_needs_attention_when_none_is_blocked_or_failed() {
            for (const phase of ["idle", "running", "done"]) {
                const plan = Fx.fiveStagePlan(phase === "done" ? "done" : "approved")
                const view = make(props(phase, plan, phase === "running" ? ["stop"] : ["run", "editPlan"]))
                wait(30)
                compare(Fx.walk(view, "overviewAttention").filter(o => o.visible).length, 0,
                    "no attention stage in the " + phase + " phase")
                view.destroy()
                created = []
            }
        }

        function test_S7_clicking_a_stage_row_opens_that_stage() {
            const view = make(props("running", planWith("blocked"), ["stop"]))
            const requested = []
            view.stageRequested.connect(index => requested.push(index))
            wait(30)
            const rows = Fx.walk(view, "overviewStageRow").filter(o => o.visible)
                .sort((a, b) => yIn(a) - yIn(b))
            compare(rows.length, 5, "one line per stage")
            mouseClick(rows[2])
            compare(requested, [2], "clicking row 3 requests index 2")
            mouseClick(rows[3])
            compare(requested, [2, 3], "clicking the blocked row requests index 3")
            mouseClick(rows[0])
            compare(requested, [2, 3, 0])
        }
    }
}
