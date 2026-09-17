import QtQuick
import QtTest
import "../../quickshell"

// Non-business view tests for the M2 spec-phase panel additions: spec status
// text, review verdict rendering, action button enabled states and signal
// emission for create/chat/review/approve. Complements tst_features_view.qml,
// which stays unchanged and covers only the M1 shape.
Item {
    id: fixture
    width: 900
    height: 640

    property var lastCreate: null
    property var lastChat: null
    property var lastReview: null
    property var lastApproveSpec: null
    property var lastApproveScenarios: null
    property var lastSelected: null

    function draftFeature() {
        return { slug: "draft-one", title: "Draft One", path: "/p/docs/features/draft-one",
            status: "valid", valid: true, reasons: [] }
    }
    function invalidFeature() {
        return { slug: "broken-one", title: "Broken One", path: "/p/docs/features/broken-one",
            status: "invalid", valid: false, reasons: ["missing required file: scenarios.md"] }
    }
    function approvableFeature() {
        return { slug: "approvable-one", title: "Approvable One", path: "/p/docs/features/approvable-one",
            status: "valid", valid: true, reasons: [] }
    }

    function baseSpecs() {
        return {
            "draft-one": { slug: "draft-one", status: "valid", valid: true, spec_status: "draft",
                content_hash: "h1", latest_review: null, review_current: false },
            "broken-one": { slug: "broken-one", status: "invalid", valid: false, spec_status: "draft",
                content_hash: "h2", latest_review: null, review_current: false },
            "approvable-one": { slug: "approvable-one", status: "valid", valid: true, spec_status: "draft",
                content_hash: "h3",
                latest_review: { approved: true, summary: "Looks solid.", issues: [], questions: ["Ship now?"] },
                review_current: true }
        }
    }

    FeaturesView {
        id: view
        anchors.fill: parent
        open: true
        pending: false
        errorText: ""
        rows: [fixture.draftFeature(), fixture.invalidFeature(), fixture.approvableFeature()]
        specs: fixture.baseSpecs()
        activity: null
        selectedSlug: ""
        detailState: null
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
        onCreateRequested: (slug, title) => fixture.lastCreate = { slug: slug, title: title }
        onChatRequested: (feature, message) => fixture.lastChat = { feature: feature, message: message }
        onReviewRequested: feature => fixture.lastReview = feature
        onApproveSpecRequested: feature => fixture.lastApproveSpec = feature
        onApproveScenariosRequested: feature => fixture.lastApproveScenarios = feature
        onFeatureSelected: feature => fixture.lastSelected = feature
    }

    TestCase {
        name: "FeatureSpecActions"
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
            view.rows = [fixture.draftFeature(), fixture.invalidFeature(), fixture.approvableFeature()]
            view.specs = fixture.baseSpecs()
            view.activity = null
            view.selectedSlug = ""
            view.detailState = null
            view.newFeatureOpen = false
            view.newFeatureError = ""
            fixture.lastCreate = null
            fixture.lastChat = null
            fixture.lastReview = null
            fixture.lastApproveSpec = null
            fixture.lastApproveScenarios = null
            fixture.lastSelected = null
            wait(30)
        }

        function test_spec_status_text_renders_per_row() {
            const statuses = walk(view, "featureSpecStatus")
            compare(statuses.map(t => t.text), ["draft", "draft", "draft"])
        }

        function test_review_and_approve_buttons_respect_enabled_rules() {
            const reviewButtons = walk(view, "featureReviewButton")
            compare(reviewButtons.length, 3)
            verify(reviewButtons[0].enabled, "review is enabled for a valid feature")
            verify(!reviewButtons[1].enabled, "review is disabled for an invalid feature")

            const approveSpecButtons = walk(view, "featureApproveSpecButton")
            verify(!approveSpecButtons[0].enabled, "no approving current review yet")
            verify(!approveSpecButtons[1].enabled, "invalid feature can never approve")
            verify(approveSpecButtons[2].enabled, "approvable-one has a current approving review")

            const approveScenarioButtons = walk(view, "featureApproveScenariosButton")
            for (let i = 0; i < approveScenarioButtons.length; ++i)
                verify(!approveScenarioButtons[i].enabled, "no feature has an approved spec yet")
        }

        function test_review_disabled_while_activity_running() {
            view.activity = { kind: "review", slug: "approvable-one", request_id: 1, status: "running" }
            wait(30)
            const reviewButtons = walk(view, "featureReviewButton")
            verify(!reviewButtons[2].enabled, "review is disabled while its own activity runs")
        }

        function test_review_button_emits_review_requested() {
            const reviewButtons = walk(view, "featureReviewButton")
            mouseClick(reviewButtons[0])
            verify(fixture.lastReview !== null)
            compare(fixture.lastReview.slug, "draft-one")
        }

        function test_approve_spec_button_emits_when_enabled() {
            const approveSpecButtons = walk(view, "featureApproveSpecButton")
            mouseClick(approveSpecButtons[2])
            verify(fixture.lastApproveSpec !== null)
            compare(fixture.lastApproveSpec.slug, "approvable-one")
        }

        function test_approve_scenarios_button_emits_when_spec_approved() {
            view.specs = Object.assign({}, fixture.baseSpecs(), {
                "approvable-one": Object.assign({}, fixture.baseSpecs()["approvable-one"],
                    { spec_status: "spec approved" })
            })
            wait(30)
            const approveScenarioButtons = walk(view, "featureApproveScenariosButton")
            verify(approveScenarioButtons[2].enabled)
            mouseClick(approveScenarioButtons[2])
            verify(fixture.lastApproveScenarios !== null)
            compare(fixture.lastApproveScenarios.slug, "approvable-one")
        }

        function test_chat_button_selects_feature() {
            const chatButtons = walk(view, "featureChatButton")
            mouseClick(chatButtons[0])
            verify(fixture.lastSelected !== null)
            compare(fixture.lastSelected.slug, "draft-one")
        }

        function test_review_verdict_renders_for_selected_feature() {
            view.selectedSlug = "approvable-one"
            wait(30)
            const verdicts = walk(view, "featureReviewVerdict")
            compare(verdicts.length, 1)
            verify(verdicts[0].visible, "a current review renders a verdict block")

            const labels = walk(view, "featureReviewVerdictLabel")
            compare(labels[0].text, "approved")

            const summaries = walk(view, "featureReviewVerdictSummary")
            compare(summaries[0].text, "Looks solid.")

            const questions = walk(view, "featureReviewVerdictQuestion")
            compare(questions.map(t => t.text), ["? Ship now?"])
        }

        function test_no_verdict_when_feature_has_no_review() {
            view.selectedSlug = "draft-one"
            wait(30)
            const verdicts = walk(view, "featureReviewVerdict")
            compare(verdicts.length, 1)
            verify(!verdicts[0].visible, "no review yet means no verdict block")
        }

        function test_new_feature_form_validates_and_emits_create_requested() {
            const newButton = findChild(view, "featureNewButton")
            mouseClick(newButton)
            wait(30)

            const slugField = findChild(view, "featureNewSlugField")
            const titleField = findChild(view, "featureNewTitleField")
            slugField.text = "Not Valid"
            const createButton = findChild(view, "featureCreateButton")
            mouseClick(createButton)
            wait(30)

            const formError = findChild(view, "featureNewFormError")
            verify(formError.visible, "an invalid slug shows an inline validation error")
            verify(fixture.lastCreate === null, "createRequested must not fire for invalid input")

            slugField.text = "valid-slug"
            titleField.text = "Valid Title"
            mouseClick(createButton)
            wait(30)

            verify(fixture.lastCreate !== null)
            compare(fixture.lastCreate.slug, "valid-slug")
            compare(fixture.lastCreate.title, "Valid Title")
        }

        function test_chat_send_disabled_while_running_and_emits_chat_requested() {
            view.selectedSlug = "draft-one"
            wait(30)
            const input = findChild(view, "featureChatInput")
            const sendButton = findChild(view, "featureSendButton")
            verify(!sendButton.enabled, "Send stays disabled while the message is empty")

            input.text = "please add a section"
            wait(30)
            verify(sendButton.enabled, "Send is enabled once there is a message and nothing is running")

            mouseClick(sendButton)
            wait(30)
            verify(fixture.lastChat !== null)
            compare(fixture.lastChat.feature.slug, "draft-one")
            compare(fixture.lastChat.message, "please add a section")

            view.activity = { kind: "chat", slug: "draft-one", request_id: 2, status: "running" }
            input.text = "another message"
            wait(30)
            verify(!sendButton.enabled, "Send is disabled while this feature's activity is running")
        }
    }
}
