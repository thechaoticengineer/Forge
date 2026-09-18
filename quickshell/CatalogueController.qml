import QtQuick
import "CataloguePresentation.js" as CataloguePresentation

// Interface: the model policy editor's state and API calls. CatalogueEditor
// binds to these properties; Panel.qml passes host (localError, api(), act(),
// refresh()), the project identity, the engine's catalogue snapshot and the
// editor view. Non-visual.
Item {
  id: root

  property var host: null
  property var engineState: null
  property var catalogue: null
  property var catalogueEditorView: null
  property string lastProject: ""
  property int projectViewRevision: 0
  readonly property var api: host ? host.api : null
  readonly property var act: host ? host.act : null
  readonly property var refresh: host ? host.refresh : null

  // A project switch drops an unfinished AI suggestion.
  function reset() {
    catalogueAiPending = false
    catalogueAiRequest = -1
    catalogueAiMessage = ""
  }

  property var catalogueDetails: null
  property bool catalogueOpen: false
  property string catalogueDraft: ""
  property bool catalogueAiPending: false
  property int catalogueAiRequest: -1
  property string catalogueAiSent: ""
  property string catalogueAiReady: ""
  property string catalogueAiUndo: ""
  property string catalogueAiMessage: ""
  property bool catalogueWasRefreshing: false
  onCatalogueChanged: {
    const refreshing = !!catalogue && catalogue.refreshing
    if (catalogueWasRefreshing && !refreshing && catalogueOpen) {
      api("GET", "/api/models", null, function(resp, status) {
        if (status === 200 && resp) root.catalogueDetails = resp
      })
    }
    catalogueWasRefreshing = refreshing
  }

  function openCatalogue() {
    if (root.catalogueAiPending || root.catalogueAiUndo || root.catalogueAiReady) {
      root.catalogueOpen = true
      return
    }
    api("GET", "/api/models", null, function(resp, status) {
      if (status !== 200 || !resp) return
      root.catalogueDetails = resp
      root.catalogueDraft = JSON.stringify(resp.policy, null, 2)
      root.catalogueOpen = true
    })
  }
  function saveCatalogue() {
    let policy
    try { policy = JSON.parse(catalogueEditorView.editor.text) }
    catch (e) { host.localError = "Model policy must be valid JSON: " + e; return }
    act("/api/settings", { model_catalogue: policy,
      expected_model_policy: root.catalogueDetails ? root.catalogueDetails.policy : undefined }, function(resp, status) {
      if (status === 200) {
        root.catalogueAiUndo = ""
        root.catalogueAiReady = ""
        root.catalogueAiMessage = ""
        root.openCatalogue()
      } else {
        root.catalogueAiMessage = resp && resp.error ? resp.error : "Could not save model policy."
      }
    })
  }

  function reloadCatalogue() {
    catalogueAiUndo = ""
    catalogueAiReady = ""
    catalogueAiMessage = ""
    root.openCatalogue()
  }

  function suggestCatalogue() {
    if (catalogueAiPending) return
    let policy
    try { policy = JSON.parse(catalogueEditorView.editor.text) }
    catch (e) { root.catalogueAiMessage = "Model policy must be valid JSON: " + e; return }
    const revision = projectViewRevision
    catalogueAiSent = catalogueEditorView.editor.text
    catalogueAiPending = true
    catalogueAiRequest = -1
    catalogueAiReady = ""
    catalogueAiMessage = "AI is checking official sources and selecting up to 4 models per provider…"
    api("POST", "/api/models/suggest", { project: lastProject, policy: policy }, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (status === 202 && resp) {
        root.catalogueAiRequest = resp.request_id
        root.refresh()
        root.syncCatalogueSuggestion()
      } else {
        root.catalogueAiPending = false
        root.catalogueAiMessage = resp && resp.error ? resp.error : "Could not request AI tiers. Check the engine connection."
      }
    }, true)
  }

  function syncCatalogueSuggestion() {
    const snapshot = engineState ? engineState.model_policy_suggestion : null
    if (catalogueAiRequest < 0 || !snapshot || snapshot.request_id !== catalogueAiRequest
        || snapshot.status === "running") return
    const requestId = catalogueAiRequest
    if (snapshot.status === "failed") {
      catalogueAiRequest = -1
      catalogueAiPending = false
      catalogueAiMessage = snapshot.error || "Could not assign model tiers."
      return
    }
    if (snapshot.status !== "ready") return
    catalogueAiRequest = -1
    const revision = projectViewRevision
    api("GET", "/api/models/suggestion?project=" + encodeURIComponent(lastProject), null, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      root.catalogueAiPending = false
      if (status !== 200 || !resp || resp.request_id !== requestId || resp.status !== "ready" || !resp.policy) {
        root.catalogueAiMessage = "Could not load AI tiers. Try again."
        return
      }
      root.catalogueAiReady = JSON.stringify(resp.policy, null, 2)
      root.catalogueAiMessage = CataloguePresentation.suggestionMessage(resp)
      if (catalogueEditorView.editor.text === root.catalogueAiSent) root.applyCatalogueSuggestion()
    }, true)
  }

  function applyCatalogueSuggestion() {
    if (catalogueAiReady === "") return
    catalogueAiUndo = catalogueEditorView.editor.text
    catalogueEditorView.editor.text = catalogueAiReady
    catalogueDraft = catalogueAiReady
    catalogueAiReady = ""
  }

  function undoCatalogueSuggestion() {
    catalogueEditorView.editor.text = catalogueAiUndo
    catalogueDraft = catalogueAiUndo
    catalogueAiUndo = ""
    catalogueAiMessage = "AI changes undone."
  }
}
