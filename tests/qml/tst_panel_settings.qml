import QtQuick
import QtTest
import "PanelFixtures.js" as Fx

// Panel redesign M1, Settings view. Covers S8 (every run and agent setting is a label/value row
// that emits settingActivated(key) when clicked) and S9 (quota, catalogue metadata and the
// policy error are shown, together with Model settings & options, Refresh models, Cancel
// refresh, Refresh Claude limits, Update Forge and Change project). Loads the qs-free
// SettingsView from quickshell/ by URL.
Item {
    id: fixture
    width: 728
    height: 600

    Item {
        id: area
        anchors.fill: parent
    }

    TestCase {
        name: "PanelSettings"
        when: windowShown

        property var created: []

        // key -> [label, value] exactly as the old settings buttons read them.
        readonly property var rows: [
            { key: "planner", label: "planner", value: "claude" },
            { key: "architect", label: "architect", value: "codex" },
            { key: "implementer", label: "implementer", value: "claude" },
            { key: "reviewer", label: "reviewer", value: "codex" },
            { key: "automatic_routing", label: "automatic routing", value: "yes" },
            { key: "architect_review", label: "architect review", value: "per plan" },
            { key: "reviewer_review", label: "reviewer review", value: "per stage" },
            { key: "auto_push", label: "push at end", value: "yes" },
            { key: "queue_auto_approve", label: "auto-approve", value: "no" }
        ]

        function make(extra) {
            const comp = Qt.createComponent("../../quickshell/SettingsView.qml")
            if (comp.status !== Component.Ready)
                fail("SettingsView.qml cannot be loaded: " + comp.errorString())
            const state = Fx.engineState("idle", null)
            const obj = comp.createObject(area, Object.assign({
                width: 728, height: 600, engineState: state, catalogue: state.model_catalogue,
                quota: state.claude_quota, engineOnline: true, busy: false, catalogueOpen: false,
                reviewerLabel: "reviewer: codex",
                cadenceLabels: { architect: "per plan", reviewer: "per stage" }
            }, Fx.palette, extra || {}))
            if (!obj) fail("SettingsView.qml rejected the contract properties: " + comp.errorString())
            created.push(obj)
            return obj
        }

        function cleanup() {
            for (let i = 0; i < created.length; ++i) created[i].destroy()
            created = []
            wait(10)
        }

        function one(view, name) {
            const found = Fx.walk(view, name)
            compare(found.length, 1, "SettingsView must contain exactly one " + name)
            return found[0]
        }

        function shown(view, name) {
            return Fx.walk(view, name).filter(o => o.visible).length > 0
        }

        // ---- S8 -------------------------------------------------------------------------

        function test_S8_the_nine_run_and_agent_settings_are_label_value_rows() {
            const view = make()
            wait(30)
            compare(Fx.walkPrefix(view, "settingRow_").length, 9, "exactly nine setting rows")
            for (const row of rows) {
                const item = one(view, "settingRow_" + row.key)
                verify(item.visible, row.key + " row must be visible")
                verify(String(item.label).toLowerCase().indexOf(row.label) >= 0,
                    row.key + " label is '" + item.label + "', expected it to name '" + row.label + "'")
                verify(String(item.value).indexOf(row.value) >= 0,
                    row.key + " value is '" + item.value + "', expected '" + row.value + "'")
                const drawn = Fx.allText(item)
                verify(drawn.indexOf(row.value) >= 0, row.key + " draws its value\n" + drawn)
            }
        }

        function test_S8_values_follow_the_engine_settings() {
            const state = Fx.engineState("idle", null)
            state.settings.planner = "codex"
            state.settings.automatic_routing = false
            state.settings.auto_push = false
            state.settings.queue_auto_approve = true
            const view = make({ engineState: state, cadenceLabels: { architect: "per stage", reviewer: "per plan" },
                reviewerLabel: "reviewer: auto (other provider)" })
            wait(30)
            compare(one(view, "settingRow_planner").value, "codex")
            compare(one(view, "settingRow_automatic_routing").value, "no")
            compare(one(view, "settingRow_auto_push").value, "no")
            compare(one(view, "settingRow_queue_auto_approve").value, "yes")
            compare(one(view, "settingRow_architect_review").value, "per stage")
            compare(one(view, "settingRow_reviewer_review").value, "per plan")
            verify(String(one(view, "settingRow_reviewer").value).indexOf("auto (other provider)") >= 0)
        }

        function test_S8_clicking_a_row_asks_the_panel_to_cycle_that_setting() {
            const view = make()
            const activated = []
            view.settingActivated.connect(key => activated.push(key))
            wait(30)
            for (const row of rows) {
                mouseClick(one(view, "settingRow_" + row.key))
                compare(activated[activated.length - 1], row.key, "clicking " + row.key)
            }
            compare(activated, rows.map(r => r.key))
        }

        function test_S8_every_row_fits_the_panel_width() {
            const view = make()
            wait(30)
            const items = Fx.walkPrefix(view, "settingRow_")
            for (const item of items) {
                const p = item.mapToItem(fixture, 0, 0)
                verify(p.x >= 0 && p.x + item.width <= fixture.width + 0.5, item.objectName + " must not overflow the width")
            }
        }

        // ---- S9 -------------------------------------------------------------------------

        function policyErrorProps() {
            return { catalogue: Fx.catalogue({ policy_error: "tier standard: unknown model \"gpt-9\" for codex" }) }
        }

        function test_S9_quota_lines_catalogue_metadata_and_the_policy_error_are_shown() {
            const view = make(policyErrorProps())
            wait(30)
            const quota = one(view, "quotaSummary")
            verify(quota.visible)
            const quotaText = Fx.allText(quota)
            verify(quotaText.indexOf("Claude · 5h") >= 0, "the 5h window is listed\n" + quotaText)
            verify(quotaText.indexOf("86% remaining") >= 0, "the 5h remaining percentage is listed\n" + quotaText)
            verify(quotaText.indexOf("Claude · weekly") >= 0, "the weekly window is listed\n" + quotaText)

            const catalogue = one(view, "catalogueSummary")
            verify(catalogue.visible)
            verify(Fx.allText(catalogue).indexOf("revision ai-1789285303642989665-1") >= 0)
            verify(Fx.allText(catalogue).indexOf("8 explicit entries") >= 0)

            const metadata = one(view, "metadataSummary")
            verify(metadata.visible)
            verify(Fx.allText(metadata).indexOf("11 records") >= 0)
            verify(Fx.allText(metadata).indexOf("2 source errors") >= 0)

            const error = one(view, "policyError")
            verify(error.visible, "the policy error is shown")
            verify(Fx.allText(error).indexOf("unknown model \"gpt-9\" for codex") >= 0)
        }

        function test_S9_the_policy_error_is_hidden_when_the_catalogue_reports_none() {
            const view = make()
            wait(30)
            verify(!shown(view, "policyError"), "no policy error, nothing to show")
        }

        function test_S9_models_limits_and_maintenance_actions_live_in_settings() {
            const view = make()
            wait(30)
            for (const id of ["modelSettings", "refreshModels", "refreshLimits", "update", "changeProject"]) {
                const button = one(view, "settingsAction_" + id)
                verify(button.visible, id + " must be visible in Settings")
            }
            verify(Fx.allText(one(view, "settingsAction_modelSettings")).indexOf("Model settings & options") >= 0)
            verify(Fx.allText(one(view, "settingsAction_refreshModels")).indexOf("Refresh models") >= 0)
            verify(Fx.allText(one(view, "settingsAction_refreshLimits")).indexOf("Refresh Claude limits") >= 0)
            verify(Fx.allText(one(view, "settingsAction_update")).indexOf("Update Forge") >= 0)
            verify(Fx.allText(one(view, "settingsAction_changeProject")).indexOf("Change project") >= 0)
        }

        function test_S9_cancel_refresh_is_visible_only_while_the_catalogue_is_refreshing() {
            const idle = make()
            wait(30)
            verify(!shown(idle, "settingsAction_cancelRefresh"), "no refresh, no Cancel refresh")
            idle.destroy()
            created = []

            const refreshing = make({ catalogue: Fx.catalogue({ refreshing: true }) })
            wait(30)
            const cancel = one(refreshing, "settingsAction_cancelRefresh")
            verify(cancel.visible, "Cancel refresh appears while refreshing")
            verify(Fx.allText(cancel).indexOf("Cancel refresh") >= 0)
            verify(Fx.allText(one(refreshing, "settingsAction_refreshModels")).indexOf("Models: refreshing…") >= 0,
                "Refresh models reads 'Models: refreshing…' while refreshing")
            compare(one(refreshing, "settingsAction_refreshModels").enabled, false)

            refreshing.catalogue = Fx.catalogue({ refreshing: false })
            wait(30)
            verify(!shown(refreshing, "settingsAction_cancelRefresh"), "Cancel refresh disappears when the refresh ends")
        }

        function test_S9_clicking_an_action_requests_it() {
            const state = Fx.engineState("idle", null)
            const view = make({ catalogue: Fx.catalogue({ refreshing: true }), engineState: state })
            const requested = []
            view.actionRequested.connect(id => requested.push(id))
            wait(30)
            for (const id of ["modelSettings", "cancelRefresh", "refreshLimits", "update", "changeProject"]) {
                mouseClick(one(view, "settingsAction_" + id))
                compare(requested[requested.length - 1], id, "clicking " + id)
            }
            const idle = make()
            idle.actionRequested.connect(id => requested.push(id))
            wait(30)
            mouseClick(one(idle, "settingsAction_refreshModels"))
            compare(requested[requested.length - 1], "refreshModels")
        }

        function test_S9_the_actions_keep_their_old_enabled_guards() {
            const offline = make({ engineOnline: false })
            wait(30)
            for (const id of ["refreshModels", "refreshLimits", "update", "changeProject"])
                compare(one(offline, "settingsAction_" + id).enabled, false, id + " needs the engine online")
            compare(one(offline, "settingsAction_modelSettings").enabled, true, "Model settings has no guard")
            offline.destroy()
            created = []

            const busy = make({ busy: true })
            const requested = []
            busy.actionRequested.connect(id => requested.push(id))
            wait(30)
            compare(one(busy, "settingsAction_update").enabled, false, "Update Forge waits for an idle engine")
            mouseClick(one(busy, "settingsAction_update"))
            compare(requested.length, 0, "a disabled action emits nothing")
            compare(one(busy, "settingsAction_refreshModels").enabled, true)
            compare(one(busy, "settingsAction_changeProject").enabled, true)

            const checking = make({ engineState: Object.assign(Fx.engineState("idle", null),
                { claude_quota: Object.assign(Fx.quota(), { refreshing: true }) }),
                quota: Object.assign(Fx.quota(), { refreshing: true }) })
            wait(30)
            compare(one(checking, "settingsAction_refreshLimits").enabled, false, "limits are already being checked")
            verify(Fx.allText(one(checking, "settingsAction_refreshLimits")).indexOf("Limits: checking…") >= 0)
        }

        function test_S9_the_model_settings_action_toggles_its_label() {
            const view = make({ catalogueOpen: true })
            wait(30)
            verify(Fx.allText(one(view, "settingsAction_modelSettings")).indexOf("Close model settings") >= 0)
        }
    }
}
