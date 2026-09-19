import QtQuick
import QtTest
import "PanelFixtures.js" as Fx

// Business tests for the feature page of milestone M5 (panel viewer): S36 (sub-tabs,
// rendered Markdown, back), S39 (scenarios with results), S40 (designs and Mermaid
// source), S41 (status header) and S42 (actions, inline refusal, chat and review
// details). Loads the not-yet-existing quickshell/FeaturePage.qml by URL inside each
// test, so a missing component fails that test with a clear message instead of
// breaking the whole `qmltestrunner -input tests/qml` run.
//
// Contract of FeaturePage (later stages implement it exactly): properties feature
// ({slug, title, path, status, valid, reasons}), spec (a Features.featureSpecsBySlug
// entry), content (the GET /api/features/content response, or null while loading),
// detailState (the GET /api/features/state response or null), activity (the running
// feature activity or null), refusal (null or {message}), subTab (string, one of
// README, Scenarios, Decisions, Milestones, Design; a tab click changes it) and the
// shared palette properties; signals backRequested(), reviewRequested(var feature),
// approveSpecRequested(var feature), approveScenariosRequested(var feature),
// planMilestoneRequested(var feature, var milestone), chatRequested(var feature),
// reviewDetailsRequested(var feature); and the function handleKey(event) returning
// "handled" or "".
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
        name: "FeaturePage"
        when: windowShown

        property var created: []

        function cleanup() {
            for (let i = 0; i < created.length; ++i) created[i].destroy()
            created = []
            wait(10)
        }

        // ---- fixtures ------------------------------------------------------------------

        readonly property string mermaidSource: "graph TD\n  A --> B"
        readonly property string readmeText: "# Demo feature\n\nA **bold** intro with a [link](https://example.com).\n\n"
            + "- first item\n- second item\n\n```js\nconst x = 1\n```\n\n```mermaid\n" + mermaidSource + "\n```\n\nClosing words.\n"

        function feature() {
            return { slug: "demo", title: "Demo Feature", path: "/p/docs/features/demo",
                status: "valid", valid: true, reasons: [] }
        }

        function spec(specStatus, extra) {
            return Object.assign({ slug: "demo", status: "valid", valid: true, spec_status: specStatus || "scenarios approved",
                content_hash: "h1", latest_review: { approved: true, summary: "Consistent.", issues: [], questions: [] },
                review_current: true, progress: "in progress",
                milestones: [
                    { id: "M1", title: "Done", status: "implemented", covers: ["S1"], business_tests: [], plan: { status: "completed" } },
                    { id: "M2", title: "Planned", status: "planned", covers: ["S2", "S3"], business_tests: [], plan: { status: "planning" } },
                    { id: "M3", title: "Uncovered", status: "planned", covers: [], business_tests: [], plan: null }
                ] }, extra || {})
        }

        function file(text) { return { text: text, error: null } }

        function content(extra) {
            return Object.assign({ slug: "demo",
                files: {
                    "README.md": file(readmeText),
                    "scenarios.md": file("# Scenarios\n"),
                    "decisions.md": file("## D1: A decision\n\nDecided **firmly**.\n"),
                    "milestones.md": file("# Milestones\n\n## M1: Done\n")
                },
                scenarios: [
                    { id: "S1", title: "First scenario", given: "a starting point", when: "something happens",
                        then: "an outcome follows", milestone: "M1",
                        result: { status: "passed", evidence: "the S1 test passes", role: "reviewer", plan_id: "plan-20260919-1",
                            milestone: "M1", unix: 1800000000, out_of_date: false } },
                    { id: "S2", title: "Second scenario", given: "another start", when: "something else happens",
                        then: "another outcome follows", milestone: "M2",
                        result: { status: "failed", evidence: "no executable test names S2", role: "reviewer",
                            plan_id: "plan-20260919-2", milestone: "M2", unix: 1800000100, out_of_date: true } },
                    { id: "S3", title: "Third scenario", given: "a third start", when: "a third thing happens",
                        then: "a third outcome follows", milestone: null, result: null }
                ],
                milestones: [],
                design: [
                    { path: "design/main.pen", kind: "pen", png: "design/main.png" },
                    { path: "design/main.png", kind: "png", absolute_path: "/p/docs/features/demo/design/main.png" },
                    { path: "design/lonely.pen", kind: "pen", png: null },
                    { path: "design/flow.mmd", kind: "mermaid", text: "graph LR\n  A --> B\n" }
                ] }, extra || {})
        }

        function props(extra) {
            return Object.assign({ width: 728, height: 600, subTab: "README", feature: feature(), spec: spec(),
                content: content(), detailState: { slug: "demo", valid: true, reasons: [] },
                activity: null, refusal: null }, Fx.palette, extra || {})
        }

        // Creates the feature page, or fails this test with the reason it cannot be loaded.
        function make(extra) {
            const comp = Qt.createComponent("../../quickshell/FeaturePage.qml")
            if (comp.status !== Component.Ready)
                fail("FeaturePage.qml cannot be loaded (it does not exist yet, or is broken): " + comp.errorString())
            const values = props(extra)
            const obj = comp.createObject(area, values)
            if (!obj) fail("FeaturePage.qml rejected the contract properties: " + comp.errorString())
            created.push(obj)
            assignData(obj, values, ["feature", "spec", "content", "detailState", "activity", "refusal"])
            wait(30)
            return obj
        }

        // createObject() hands arrays over as list wrappers for which Array.isArray() is false,
        // while the panel binds real JavaScript arrays. Assigning the data properties again makes
        // the page see what Panel.qml would give it.
        function assignData(obj, values, names) {
            for (const name of names) obj[name] = values[name]
        }

        function visibleNamed(view, name) {
            return Fx.walk(view, name).filter(o => o.visible)
        }

        function one(view, name) {
            const found = visibleNamed(view, name)
            compare(found.length, 1, "FeaturePage must show exactly one " + name)
            return found[0]
        }

        function textOf(item) {
            return item.label !== undefined ? item.label : item.text
        }

        function keyEvent(key, text, modifiers) {
            return eventComponent.createObject(fixture,
                { key: key, text: text || "", modifiers: modifiers || Qt.NoModifier })
        }

        // Sends a key to handleKey and returns {result, events}.
        function press(view, key, text, modifiers) {
            const events = []
            view.backRequested.connect(() => events.push("back"))
            const event = keyEvent(key, text, modifiers)
            const result = view.handleKey(event)
            event.destroy()
            return { result: result, events: events }
        }

        // ---- S36: sub-tabs, Markdown, back -----------------------------------------------

        function test_S36_the_sub_tab_bar_has_the_five_tabs() {
            const view = make()
            for (const name of ["README", "Scenarios", "Decisions", "Milestones", "Design"])
                compare(textOf(one(view, "featurePageTab_" + name)), name)
            compare(Fx.walkPrefix(view, "featurePageTab_").filter(o => o.visible).length, 5, "exactly five tabs")
        }

        function test_S36_clicking_a_tab_switches_the_shown_file() {
            const view = make()
            mouseClick(one(view, "featurePageTab_Decisions"))
            wait(30)
            compare(view.subTab, "Decisions")
            const shownText = visibleNamed(view, "featureMarkdown").map(part => part.text).join("\n")
            verify(shownText.indexOf("Decided **firmly**.") !== -1, "the Decisions tab shows decisions.md")
            verify(shownText.indexOf("Demo feature") === -1, "README text is gone")
            mouseClick(one(view, "featurePageTab_Milestones"))
            wait(30)
            verify(visibleNamed(view, "featureMarkdown").map(part => part.text).join("\n").indexOf("## M1: Done") !== -1,
                "the Milestones tab shows milestones.md")
        }

        function test_S36_the_README_tab_renders_Markdown_through_a_MarkdownText_Text() {
            const view = make()
            const parts = visibleNamed(view, "featureMarkdown")
            verify(parts.length >= 1, "the README is shown through featureMarkdown Text items")
            for (const part of parts) compare(part.textFormat, Text.MarkdownText)
            const text = parts.map(part => part.text).join("\n")
            verify(text.indexOf("# Demo feature") !== -1, "headings reach the Markdown Text")
            verify(text.indexOf("A **bold** intro with a [link](https://example.com).") !== -1, "emphasis and links")
            verify(text.indexOf("- first item") !== -1, "lists")
            verify(text.indexOf("```js\nconst x = 1\n```") !== -1, "code blocks")
            verify(text.indexOf("Closing words.") !== -1, "text after a mermaid block stays Markdown")
            verify(text.indexOf("```mermaid") === -1, "the mermaid block is not part of the Markdown text")
        }

        function test_S36_a_file_that_could_not_be_read_shows_its_reason_instead_of_Markdown() {
            const files = content().files
            files["decisions.md"] = { text: null, error: "file not found: decisions.md" }
            const view = make({ subTab: "Decisions", content: content({ files: files }) })
            compare(one(view, "featureFileError").text, "file not found: decisions.md")
            compare(visibleNamed(view, "featureMarkdown").length, 0)
        }

        function test_S36_the_page_waits_for_its_content_without_failing() {
            const view = make({ content: null })
            compare(visibleNamed(view, "featureMarkdown").length, 0)
            verify(one(view, "featurePageBack"), "Back stays available while loading")
        }

        function test_S36_a_long_document_scrolls_and_wraps_to_the_page_width() {
            let text = "# Long document\n\n"
            for (let i = 0; i < 400; ++i)
                text += "Paragraph " + i + " keeps going with enough words to wrap across the width of the page several times over.\n\n"
            const files = content().files
            files["README.md"] = file(text)
            const view = make({ content: content({ files: files }) })
            const scroll = one(view, "featurePageScroll")
            verify(scroll.contentHeight > scroll.height, "a long document is taller than the page and scrolls")
            for (const markdown of visibleNamed(view, "featureMarkdown"))
                verify(markdown.width <= view.width, "the Markdown text wraps within the page: " + markdown.width + " > " + view.width)
        }

        function test_S36_Escape_q_and_Back_return_to_the_Features_tab() {
            let view = make()
            let sent = press(view, Qt.Key_Escape, "")
            compare(sent.result, "handled")
            compare(sent.events, ["back"])
            sent = press(view, Qt.Key_Q, "q")
            compare(sent.result, "handled")
            compare(sent.events, ["back"])
            sent = press(view, Qt.Key_X, "x")
            compare(sent.result, "")
            compare(sent.events.length, 0)
            sent = press(view, Qt.Key_Q, "q", Qt.ControlModifier)
            compare(sent.result, "", "Ctrl+q is not a page key")

            const backs = []
            view.backRequested.connect(() => backs.push("back"))
            mouseClick(one(view, "featurePageBack"))
            compare(backs, ["back"])
        }

        // ---- S39: scenarios with results -------------------------------------------------

        function scenarioRows(view) {
            const rows = visibleNamed(view, "featureScenario")
            compare(rows.length, 3, "one row per scenario")
            return rows
        }

        function shown(row, name) {
            return Fx.walk(row, name).filter(o => o.visible)
        }

        function test_S39_each_scenario_shows_its_ID_title_given_when_then_and_milestone() {
            const view = make({ subTab: "Scenarios" })
            const rows = scenarioRows(view)
            const first = Fx.allText(rows[0])
            for (const part of ["S1", "First scenario", "a starting point", "something happens", "an outcome follows", "M1"])
                verify(first.indexOf(part) !== -1, "the first scenario shows " + part + ": " + first)
            verify(Fx.allText(rows[1]).indexOf("M2") !== -1, "the second scenario shows its covering milestone")
        }

        function test_S39_the_latest_result_is_passed_failed_with_evidence_or_not_recorded() {
            const view = make({ subTab: "Scenarios" })
            const rows = scenarioRows(view)
            compare(shown(rows[0], "featureScenarioResult").map(t => t.text), ["passed"])
            compare(shown(rows[1], "featureScenarioResult").map(t => t.text), ["failed"])
            compare(shown(rows[2], "featureScenarioResult").map(t => t.text), ["not recorded"])
            compare(shown(rows[0], "featureScenarioEvidence").length, 0, "passed results show no evidence")
            compare(shown(rows[1], "featureScenarioEvidence").map(t => t.text), ["no executable test names S2"])
            compare(shown(rows[2], "featureScenarioEvidence").length, 0)
        }

        function test_S39_the_time_and_plan_of_a_result_are_shown() {
            const view = make({ subTab: "Scenarios" })
            const rows = scenarioRows(view)
            const meta = shown(rows[0], "featureScenarioMeta")
            compare(meta.length, 1)
            verify(meta[0].text.indexOf("plan-20260919-1") !== -1, "the plan of the result: " + meta[0].text)
            verify(meta[0].text.indexOf("2027") !== -1, "the time of the result: " + meta[0].text)
            compare(shown(rows[2], "featureScenarioMeta").length, 0, "a scenario without a result has no time or plan")
        }

        function test_S39_a_scenario_changed_after_its_result_is_marked_possibly_out_of_date() {
            const view = make({ subTab: "Scenarios" })
            const rows = scenarioRows(view)
            compare(shown(rows[0], "featureScenarioOutOfDate").length, 0)
            const marked = shown(rows[1], "featureScenarioOutOfDate")
            compare(marked.length, 1)
            verify(/out of date/i.test(marked[0].text), "the marker says the result may be out of date: " + marked[0].text)
            compare(shown(rows[2], "featureScenarioOutOfDate").length, 0)
        }

        // ---- S40: designs and diagrams ---------------------------------------------------

        function test_S40_the_Design_tab_shows_each_exported_PNG_labelled_with_its_pen_file() {
            const view = make({ subTab: "Design" })
            const images = visibleNamed(view, "featureDesignImage")
            compare(images.length, 1, "one image per exported PNG")
            verify(images[0].source.toString().endsWith("/p/docs/features/demo/design/main.png"),
                "the image shows the exported PNG: " + images[0].source)
            const labels = visibleNamed(view, "featureDesignLabel").map(t => t.text)
            verify(labels.indexOf("design/main.pen") !== -1, "the image is labelled with its .pen source: " + labels)
            verify(labels.indexOf("design/lonely.pen") !== -1, "the .pen without export is listed: " + labels)
        }

        function test_S40_a_pen_file_without_a_PNG_shows_a_note() {
            const view = make({ subTab: "Design" })
            compare(visibleNamed(view, "featureDesignNote").map(t => t.text), ["no export exists"])
        }

        function test_S40_a_Mermaid_file_is_shown_as_labelled_monospace_source() {
            const view = make({ subTab: "Design" })
            const diagrams = visibleNamed(view, "featureMermaid")
            compare(diagrams.length, 1)
            compare(diagrams[0].text.trim(), "graph LR\n  A --> B")
            verify(/mono/i.test(diagrams[0].font.family), "the diagram is monospace, got " + diagrams[0].font.family)
            verify(diagrams[0].textFormat !== Text.MarkdownText && diagrams[0].textFormat !== Text.RichText,
                "the source is shown as plain text")
            verify(Fx.texts(view).indexOf("Mermaid diagram") !== -1, "the block is labelled Mermaid diagram")
            verify(Fx.allText(view).indexOf("design/flow.mmd") !== -1, "the diagram names its file")
        }

        function test_S40_a_fenced_mermaid_block_in_a_Markdown_tab_is_shown_as_labelled_source() {
            const view = make({ subTab: "README" })
            const diagrams = visibleNamed(view, "featureMermaid")
            compare(diagrams.length, 1)
            compare(diagrams[0].text.trim(), mermaidSource)
            verify(/mono/i.test(diagrams[0].font.family), "the diagram is monospace, got " + diagrams[0].font.family)
            verify(Fx.texts(view).indexOf("Mermaid diagram") !== -1, "the block is labelled Mermaid diagram")
        }

        function test_S40_a_feature_without_design_files_shows_no_images() {
            const view = make({ subTab: "Design", content: content({ design: [] }) })
            compare(visibleNamed(view, "featureDesignImage").length, 0)
            compare(visibleNamed(view, "featureMermaid").length, 0)
        }

        // ---- S41: status header ----------------------------------------------------------

        function header(view) {
            return Fx.allText(one(view, "featurePageHeader"))
        }

        function test_S41_the_header_shows_validation_spec_status_progress_and_plan_statuses() {
            const text = header(make())
            verify(/\bvalid\b/.test(text) && !/invalid/.test(text), "a valid feature says valid: " + text)
            verify(text.indexOf("scenarios approved") !== -1, "the spec status: " + text)
            verify(text.indexOf("1/3 implemented") !== -1, "the milestone progress: " + text)
            verify(text.indexOf("M2") !== -1 && text.indexOf("planning") !== -1, "the plan status of M2: " + text)
            verify(text.indexOf("M1") !== -1 && text.indexOf("completed") !== -1, "the plan status of M1: " + text)
        }

        function test_S41_an_invalid_feature_shows_its_reasons() {
            const reasons = ["milestone M1 covers unknown scenario S99", "missing required file: decisions.md"]
            const text = header(make({
                feature: Object.assign(feature(), { status: "invalid", valid: false, reasons: reasons }),
                spec: spec("draft", { status: "invalid", valid: false }),
                detailState: { slug: "demo", valid: false, reasons: reasons } }))
            verify(text.indexOf("invalid") !== -1, "the validation status: " + text)
            for (const reason of reasons) verify(text.indexOf(reason) !== -1, "the reason " + reason + ": " + text)
            verify(text.indexOf("draft") !== -1, "the spec status: " + text)
        }

        function test_S41_the_header_shows_the_review_verdict_and_whether_it_is_current() {
            let text = header(make())
            verify(text.indexOf("approved") !== -1, "the verdict: " + text)
            verify(/current/.test(text) && !/not current|outdated|out of date|stale/.test(text), "the review is current: " + text)
            text = header(make({ spec: spec("draft", {
                latest_review: { approved: false, summary: "Needs work.", issues: ["x"], questions: [] }, review_current: false }) }))
            verify(text.indexOf("changes requested") !== -1, "the verdict: " + text)
            verify(/not current|outdated|out of date|stale/.test(text), "the review no longer covers the content: " + text)
        }

        function test_S41_the_header_follows_a_refreshed_state() {
            const view = make({ spec: spec("draft") })
            verify(header(view).indexOf("scenarios approved") === -1)
            view.spec = spec("scenarios approved")
            wait(30)
            verify(header(view).indexOf("scenarios approved") !== -1, "a refreshed spec updates the header: " + header(view))
        }

        // ---- S42: actions ----------------------------------------------------------------

        function collectRequests(view) {
            const events = []
            view.reviewRequested.connect(f => events.push(["review", f.slug]))
            view.approveSpecRequested.connect(f => events.push(["approveSpec", f.slug]))
            view.approveScenariosRequested.connect(f => events.push(["approveScenarios", f.slug]))
            view.planMilestoneRequested.connect((f, m) => events.push(["plan", f.slug, m.id]))
            view.chatRequested.connect(f => events.push(["chat", f.slug]))
            view.reviewDetailsRequested.connect(f => events.push(["reviewDetails", f.slug]))
            return events
        }

        function test_S42_the_buttons_emit_the_matching_request_signals() {
            const view = make({ spec: spec("scenarios approved", { review_current: true }) })
            const events = collectRequests(view)
            mouseClick(one(view, "featurePageReview"))
            compare(events.pop(), ["review", "demo"])
            mouseClick(one(view, "featurePagePlan_M2"))
            compare(events.pop(), ["plan", "demo", "M2"])
            mouseClick(one(view, "featurePageChat"))
            compare(events.pop(), ["chat", "demo"])
            mouseClick(one(view, "featurePageReviewDetails"))
            compare(events.pop(), ["reviewDetails", "demo"])

            const spec1 = make({ spec: spec("spec approved") })
            const events1 = collectRequests(spec1)
            mouseClick(one(spec1, "featurePageApproveScenarios"))
            compare(events1.pop(), ["approveScenarios", "demo"])

            const draft = make({ spec: spec("draft") })
            const events2 = collectRequests(draft)
            mouseClick(one(draft, "featurePageApproveSpec"))
            compare(events2.pop(), ["approveSpec", "demo"])
        }

        function test_S42_a_plan_button_exists_only_for_plannable_milestones() {
            const view = make()
            compare(Fx.walkPrefix(view, "featurePagePlan_").filter(o => o.visible).map(o => o.objectName), ["featurePagePlan_M2"],
                "implemented milestones and milestones without covered scenarios have no Plan button")
        }

        function test_S42_a_disabled_action_emits_nothing_and_shows_why() {
            const view = make({ spec: spec("spec approved") })
            const events = collectRequests(view)
            const plan = one(view, "featurePagePlan_M2")
            verify(!plan.enabled, "planning needs scenarios approved")
            mouseClick(plan)
            compare(events.length, 0)
            verify(Fx.allText(view).indexOf("scenarios approved") !== -1, "the reason names the missing status")
            verify(!one(view, "featurePageApproveSpec").enabled, "the spec is already approved")
            verify(one(view, "featurePageApproveScenarios").enabled, "the scenarios can be approved")
        }

        function test_S42_a_refusal_is_shown_inline_and_cleared_with_the_property() {
            const view = make()
            compare(visibleNamed(view, "featurePageRefusal").length, 0, "no refusal, nothing shown")
            view.refusal = { message: "engine is busy" }
            wait(30)
            compare(one(view, "featurePageRefusal").text, "engine is busy")
            view.refusal = null
            wait(30)
            compare(visibleNamed(view, "featurePageRefusal").length, 0)
        }

        function test_S42_the_actions_are_enabled_under_the_same_conditions_as_in_the_Features_tab() {
            const comp = Qt.createComponent("../../quickshell/FeaturesView.qml")
            if (comp.status !== Component.Ready) fail("FeaturesView.qml cannot be loaded: " + comp.errorString())
            const cases = [
                { spec: spec("draft") },
                { spec: spec("draft", { review_current: false }) },
                { spec: spec("draft", { latest_review: { approved: false, summary: "no" } }) },
                { spec: spec("spec approved") },
                { spec: spec("scenarios approved") },
                { spec: spec("scenarios approved"), activity: { slug: "demo", status: "running" } },
                { spec: spec("draft", { status: "invalid", valid: false }), invalid: true }
            ]
            for (let i = 0; i < cases.length; ++i) {
                const c = cases[i]
                const row = c.invalid ? Object.assign(feature(), { status: "invalid", valid: false, reasons: ["broken"] }) : feature()
                const list = comp.createObject(area, Object.assign({ width: 728, height: 600, open: true, pending: false,
                    errorText: "", rows: [], specs: ({}), selectedSlug: "demo", activity: null }, Fx.palette))
                if (!list) fail("FeaturesView.qml rejected its properties: " + comp.errorString())
                created.push(list)
                assignData(list, { rows: [row], specs: { demo: c.spec }, activity: c.activity || null }, ["rows", "specs", "activity"])
                const page = make({ feature: row, spec: c.spec, activity: c.activity || null })
                wait(30)
                const label = "case " + i + " (" + c.spec.spec_status + ")"
                compare(one(page, "featurePageReview").enabled, one(list, "featureReviewButton").enabled, "Request review " + label)
                compare(one(page, "featurePageApproveSpec").enabled, one(list, "featureApproveSpecButton").enabled, "Approve spec " + label)
                compare(one(page, "featurePageApproveScenarios").enabled, one(list, "featureApproveScenariosButton").enabled,
                    "Approve scenarios " + label)
                const tabPlan = visibleNamed(list, "featurePlanMilestoneButton")
                compare(tabPlan.length, 1, "the Features tab offers one plannable milestone")
                compare(one(page, "featurePagePlan_M2").enabled, tabPlan[0].enabled, "Plan milestone " + label)
                // The Features tab keeps every action it has today.
                for (const name of ["featureChatButton", "featureReviewButton", "featureApproveSpecButton",
                    "featureApproveScenariosButton", "featureOpenButton"])
                    compare(Fx.walk(list, name).length, 1, name + " stays in the Features tab")
            }
        }
    }
}
