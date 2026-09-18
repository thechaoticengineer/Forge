import QtQuick
import QtTest
import "../../quickshell"

Item {
    id: fixture
    width: 700
    height: 560
    property var openedFeature: null

    function validFeature() {
        return { slug: "alpha", title: "Alpha", path: "/p/docs/features/alpha",
            status: "valid", valid: true, reasons: [] }
    }
    function invalidFeature() {
        return { slug: "beta", title: "Beta", path: "/p/docs/features/beta",
            status: "invalid", valid: false, reasons: ["missing required file: scenarios.md"] }
    }

    function doneFeature() {
        return { slug: "gamma", title: "Gamma", path: "/p/docs/features/gamma",
            status: "valid", valid: true, reasons: [] }
    }
    function partialFeature() {
        return { slug: "delta", title: "Delta", path: "/p/docs/features/delta",
            status: "valid", valid: true, reasons: [] }
    }

    // The progress index the panel builds with Features.featureSpecsBySlug.
    function progressSpecs() {
        return {
            gamma: { slug: "gamma", spec_status: "draft", progress: "implemented",
                milestones: [{ id: "M1", title: "One", status: "implemented" }] },
            delta: { slug: "delta", spec_status: "draft", progress: "in progress",
                milestones: [{ id: "M1", title: "One", status: "implemented" },
                    { id: "M2", title: "Two", status: "planned" }] }
        }
    }

    FeaturesView {
        id: view
        anchors.fill: parent
        open: true
        pending: false
        errorText: ""
        rows: [fixture.validFeature(), fixture.invalidFeature()]
        foreground: "#dddddd"
        mutedForeground: "#aaaaaa"
        background: "#202020"
        surface: "#282828"
        accent: "#6699ff"
        urgent: "#ff6666"
        success: "#4faf72"
        fontFamily: "monospace"
        fontSize10: 10
        fontSize11: 11
        onOpenRequested: feature => fixture.openedFeature = feature
    }

    TestCase {
        name: "FeaturesView"
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
            view.rows = [fixture.validFeature(), fixture.invalidFeature()]
            view.errorText = ""
            view.pending = false
            view.showImplemented = false
            view.specs = ({})
            fixture.openedFeature = null
            wait(30)
        }

        function test_titles_and_status_render_for_valid_and_invalid_rows() {
            const titles = walk(view, "featureTitle")
            compare(titles.map(t => t.text), ["Alpha", "Beta"])

            const slugs = walk(view, "featureSlug")
            compare(slugs.map(t => t.text), ["alpha", "beta"])

            const statuses = walk(view, "featureStatus")
            compare(statuses.map(t => t.text), ["valid", "invalid"])
            compare(statuses[0].color, view.success)
            compare(statuses[1].color, view.urgent)

            const reasons = walk(view, "featureReasons")
            verify(reasons.some(t => t.visible && t.text === "missing required file: scenarios.md"))
        }

        function test_empty_state_shown_for_no_features() {
            view.rows = []
            wait(30)
            const empty = walk(view, "featuresEmptyState")
            compare(empty.length, 1)
            verify(empty[0].visible, "the empty state renders when there are no feature specs")

            const titles = walk(view, "featureTitle")
            compare(titles.length, 0)
        }

        function test_open_in_nvim_button_emits_open_requested() {
            const buttons = walk(view, "featureOpenButton")
            compare(buttons.length, 2)
            mouseClick(buttons[1])
            verify(fixture.openedFeature !== null, "clicking the button emits openRequested")
            compare(fixture.openedFeature.slug, "beta")
            compare(fixture.openedFeature.path, "/p/docs/features/beta")
        }

        function test_implemented_features_are_hidden_until_shown() {
            view.specs = fixture.progressSpecs()
            view.rows = [fixture.validFeature(), fixture.doneFeature(), fixture.partialFeature()]
            wait(30)
            compare(walk(view, "featureTitle").map(t => t.text), ["Alpha", "Delta"])
            const toggle = walk(view, "featureImplementedToggle")[0]
            verify(toggle.visible)
            compare(toggle.label, "Show implemented (1)")
            const labels = walk(view, "featureSpecStatus").map(t => t.text)
            compare(labels[1], "1/2 implemented")

            mouseClick(toggle)
            wait(30)
            compare(walk(view, "featureTitle").map(t => t.text), ["Alpha", "Gamma", "Delta"])
            compare(toggle.label, "Hide implemented")
            const gamma = walk(view, "featureSpecStatus")[1]
            compare(gamma.text, "implemented")
            compare(gamma.color, view.success)
        }

        function test_i_key_toggles_implemented_and_empty_state_explains_hidden_rows() {
            view.specs = fixture.progressSpecs()
            view.rows = [fixture.doneFeature()]
            wait(30)
            compare(walk(view, "featureTitle").length, 0)
            const empty = walk(view, "featuresEmptyState")[0]
            verify(empty.visible)
            compare(empty.text, "All feature specs are implemented · i shows them")
            view.handleKey({ key: Qt.Key_I, modifiers: Qt.NoModifier })
            wait(30)
            verify(view.showImplemented)
            compare(walk(view, "featureTitle").map(t => t.text), ["Gamma"])
            verify(!empty.visible)
        }

        function test_toggle_hidden_without_implemented_features() {
            verify(!walk(view, "featureImplementedToggle")[0].visible)
        }
    }
}
