import QtQuick
import Quickshell
import "Features.js" as Features

// Interface: the feature specs list's state and API calls. The Features tab's
// view (FeaturesView) binds to these properties; Panel.qml owns navigation and
// passes host (currentTab, previousTab, api()), the project identity and whether
// the window is shown. Non-visual.
Item {
  id: root

  property var host: null
  property string lastProject: ""
  property int projectViewRevision: 0
  property bool active: false
  readonly property var api: host ? host.api : null

  // True while the Features tab is shown: drives its keys and activity polling.
  property bool featuresOpen: false
  property int featuresLoadedRevision: -1
  property var featureList: []
  property bool featuresPending: false
  property string featuresError: ""
  property var featureSpecs: ({})
  property var featureActivity: null
  property string selectedFeatureSlug: ""
  property var featureDetailState: null
  // The latest refused "Plan milestone" request: {slug, milestone, message}.
  property var planRefusal: null

  // A project switch drops the previous project's list and selection.
  function reset() {
    featureList = []
    featuresPending = false
    featuresError = ""
    featureSpecs = ({})
    featureActivity = null
    selectedFeatureSlug = ""
    featureDetailState = null
    planRefusal = null
  }

  // Shows the Features tab and loads its list.
  function openFeatures() {
    host.currentTab = "features"
    if (root.featuresPending) return
    root.featuresPending = true
    const revision = projectViewRevision
    api("GET", "/api/features?project=" + encodeURIComponent(lastProject), null, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      root.featuresPending = false
      root.featuresLoadedRevision = revision
      if (resp && Array.isArray(resp.features)) {
        root.featureList = Features.featureRows(resp)
        root.featureSpecs = Features.featureSpecsBySlug(resp)
        root.featureActivity = resp.activity || null
        root.featuresError = ""
      } else {
        root.featuresError = "Unable to load feature specs"
      }
      root.featuresOpen = host.currentTab === "features"
    }, true)
  }

  // Features is a tab now: closing it returns to the tab shown before, and
  // the list keeps its selection and reading position.
  function closeFeatures() {
    host.currentTab = host.previousTab
  }

  // Reloads the list after a feature action or a project switch without
  // changing tabs; a hidden Features tab reloads when it is shown again.
  function refreshFeatures() {
    if (featuresOpen) openFeatures()
    else featuresLoadedRevision = -1
  }
  onProjectViewRevisionChanged: if (featuresOpen) Qt.callLater(refreshFeatures)

  function openFeatureInEditor(feature) {
    Quickshell.execDetached(Features.editorCommand(feature))
  }

  function selectFeature(feature) {
    if (!feature || root.selectedFeatureSlug === feature.slug) return
    root.selectedFeatureSlug = feature.slug
    root.featureDetailState = null
    root.loadFeatureState(feature.slug)
  }

  function loadFeatureState(slug) {
    const revision = projectViewRevision
    api("GET", "/api/features/state?project=" + encodeURIComponent(root.lastProject)
      + "&slug=" + encodeURIComponent(slug), null, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (root.selectedFeatureSlug !== slug) return
      if (status === 200) {
        root.featureDetailState = resp
      } else {
        root.featuresError = Features.errorMessage(resp)
      }
    }, true)
  }

  // Re-fetches the feature list and, while its activity is still running,
  // the selected feature's detail state; only called by the polling Timer.
  function pollFeatureActivity() {
    const revision = projectViewRevision
    api("GET", "/api/features?project=" + encodeURIComponent(root.lastProject), null, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (status === 200 && resp && Array.isArray(resp.features)) {
        root.featureList = Features.featureRows(resp)
        root.featureSpecs = Features.featureSpecsBySlug(resp)
        root.featureActivity = resp.activity || null
        if (root.featureActivity && root.featureActivity.status === "failed")
          root.featuresError = root.featureActivity.error || "Feature activity failed"
      }
      if (root.selectedFeatureSlug !== "") root.loadFeatureState(root.selectedFeatureSlug)
    }, true)
  }

  function createFeature(slug, title) {
    const revision = projectViewRevision
    api("POST", "/api/features/create", { project: root.lastProject, slug: slug, title: title }, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (status === 200) {
        root.featuresError = ""
        // Inlined rather than delegated to openFeatures(): keeps this
        // function self-contained for isolated testing and reuse.
        api("GET", "/api/features?project=" + encodeURIComponent(root.lastProject), null, function(resp2, status2) {
          if (revision !== root.projectViewRevision) return
          if (resp2 && Array.isArray(resp2.features)) {
            root.featureList = Features.featureRows(resp2)
            root.featureSpecs = Features.featureSpecsBySlug(resp2)
            root.featureActivity = resp2.activity || null
            root.featuresError = ""
          }
          root.featuresOpen = host.currentTab === "features"
        }, true)
      } else {
        root.featuresError = Features.errorMessage(resp)
      }
    }, true)
  }

  function sendFeatureChat(feature, message) {
    const revision = projectViewRevision
    api("POST", "/api/features/chat", { project: root.lastProject, slug: feature.slug, message: message }, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (status === 200) {
        root.featuresError = ""
        root.refreshFeatures()
        if (root.selectedFeatureSlug === feature.slug) root.loadFeatureState(feature.slug)
      } else {
        root.featuresError = Features.errorMessage(resp)
      }
    }, true)
  }

  function requestFeatureReview(feature) {
    const revision = projectViewRevision
    api("POST", "/api/features/review", { project: root.lastProject, slug: feature.slug }, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (status === 200) {
        root.featuresError = ""
        root.refreshFeatures()
        if (root.selectedFeatureSlug === feature.slug) root.loadFeatureState(feature.slug)
      } else {
        root.featuresError = Features.errorMessage(resp)
      }
    }, true)
  }

  function approveFeatureSpec(feature) {
    const revision = projectViewRevision
    api("POST", "/api/features/approve_spec", { project: root.lastProject, slug: feature.slug }, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (status === 200) {
        root.featuresError = ""
        root.refreshFeatures()
        if (root.selectedFeatureSlug === feature.slug) root.loadFeatureState(feature.slug)
      } else {
        root.featuresError = Features.errorMessage(resp)
      }
    }, true)
  }

  function approveFeatureScenarios(feature) {
    const revision = projectViewRevision
    api("POST", "/api/features/approve_scenarios", { project: root.lastProject, slug: feature.slug }, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (status === 200) {
        root.featuresError = ""
        root.refreshFeatures()
        if (root.selectedFeatureSlug === feature.slug) root.loadFeatureState(feature.slug)
      } else {
        root.featuresError = Features.errorMessage(resp)
      }
    }, true)
  }

  // Starts planning a milestone of an approved feature. A refusal keeps the
  // engine's reason for that milestone; success opens the Plan tab.
  function planMilestone(feature, milestone) {
    const revision = projectViewRevision
    root.planRefusal = null
    api("POST", "/api/features/plan", Features.featurePlanRequest(root.lastProject, feature.slug, milestone.id), function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (status === 200) {
        root.featuresError = ""
        root.refreshFeatures()
        if (root.selectedFeatureSlug === feature.slug) root.loadFeatureState(feature.slug)
        if (host.refresh) host.refresh()
        host.currentTab = "plan"
      } else {
        root.featuresError = Features.errorMessage(resp)
        root.planRefusal = { slug: feature.slug, milestone: milestone.id, message: Features.errorMessage(resp) }
      }
    }, true)
  }

  Timer {
    interval: 1000
    repeat: true
    running: root.active && root.featuresOpen && root.featureActivity !== null
      && root.featureActivity.status === "running"
    onTriggered: root.pollFeatureActivity()
  }
}
