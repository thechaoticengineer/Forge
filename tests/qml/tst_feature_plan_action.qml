import QtQuick
import QtTest
import "../../quickshell"

// Non-business view tests for the M3 "Plan milestone" action: milestone rows
// with their plan status, the button's enabled and disabled rendering, the
// inline reason and refusal, and the request signal.
Item {
    id: fixture
    width: 900
    height: 640

    property var lastPlan: null

    function feature() {
        return { slug: "demo", title: "Demo", path: "/p/docs/features/demo",
            status: "valid", valid: true, reasons: [] }
    }

    function specs(specStatus) {
        return { "demo": { slug: "demo", status: "valid", valid: true, spec_status: specStatus,
            content_hash: "h1", latest_review: null, review_current: false, progress: "planned",
            milestones: [
                { id: "M1", title: "Done", status: "implemented", covers: ["S1"], business_tests: [], plan: null },
                { id: "M2", title: "Planned", status: "planned", covers: ["S2", "S3"], business_tests: [],
                    plan: { status: "planning" } },
                { id: "M3", title: "Uncovered", status: "planned", covers: [], business_tests: [], plan: null }
            ] } }
    }

    FeaturesView {
        id: view
        anchors.fill: parent
        open: true
        pending: false
        errorText: ""
        rows: [fixture.feature()]
        specs: fixture.specs("scenarios approved")
        selectedSlug: "demo"
        onPlanMilestoneRequested: (feature, milestone) => fixture.lastPlan = { feature: feature, milestone: milestone }
    }

    TestCase {
        name: "FeaturePlanAction"
        when: windowShown

        function walk(item, name) {
            const found = []
            function rec(it) {
                const kids = it.children || []
                for (let i = 0; i < kids.length; ++i) {
                    if (kids[i].objectName === name) found.push(kids[i])
                    rec(kids[i])
                }
            }
            rec(item)
            return found
        }

        function init() {
            view.specs = fixture.specs("scenarios approved")
            view.planRefusal = null
            fixture.lastPlan = null
            wait(30)
        }

        function test_milestones_list_status_and_plan_status() {
            compare(walk(view, "featureMilestoneTitle").map(t => t.text), ["M1 Done", "M2 Planned", "M3 Uncovered"])
            compare(walk(view, "featureMilestoneStatus").map(t => t.text), ["implemented", "planned", "planned"])
            const planStatus = walk(view, "featureMilestonePlanStatus").filter(t => t.visible)
            compare(planStatus.map(t => t.text), ["planning"])
        }

        function test_button_only_on_planned_milestones_with_covered_scenarios() {
            const buttons = walk(view, "featurePlanMilestoneButton").filter(b => b.visible)
            compare(buttons.length, 1)
            verify(buttons[0].enabled, "enabled while the feature is scenarios approved")
        }

        function test_button_disabled_with_reason_until_scenarios_approved() {
            view.specs = fixture.specs("spec approved")
            wait(30)
            const buttons = walk(view, "featurePlanMilestoneButton").filter(b => b.visible)
            compare(buttons.length, 1)
            verify(!buttons[0].enabled)
            const reasons = walk(view, "featureMilestoneReason").filter(t => t.visible)
            compare(reasons.length, 1)
            verify(reasons[0].text.indexOf("scenarios approved") !== -1)
            mouseClick(buttons[0])
            compare(fixture.lastPlan, null)
        }

        function test_click_emits_plan_request_for_the_milestone() {
            const buttons = walk(view, "featurePlanMilestoneButton").filter(b => b.visible)
            mouseClick(buttons[0])
            verify(fixture.lastPlan !== null)
            compare(fixture.lastPlan.feature.slug, "demo")
            compare(fixture.lastPlan.milestone.id, "M2")
        }

        function test_refusal_shows_inline_under_its_milestone_only() {
            view.planRefusal = { slug: "demo", milestone: "M2", message: "engine is busy" }
            wait(30)
            const reasons = walk(view, "featureMilestoneReason").filter(t => t.visible)
            compare(reasons.map(t => t.text), ["engine is busy"])
            view.planRefusal = { slug: "demo", milestone: "M1", message: "elsewhere" }
            wait(30)
            compare(walk(view, "featureMilestoneReason").filter(t => t.visible).map(t => t.text), ["elsewhere"])
        }
    }
}
