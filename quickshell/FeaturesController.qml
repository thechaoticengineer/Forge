import QtQuick
import QtQuick.Controls
import Quickshell
import "Features.js" as Features

// Interface: the feature specs list's state and API calls. The Features tab's
// view (FeaturesView) binds to these properties; Panel.qml owns navigation and
// passes host (currentTab, previousTab, api()), the project identity and whether
// the window is shown. It also opens and closes the feature page over the tab
// views (like Panel's stage detail) so Panel.qml stays small. Non-visual.
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
  // The read-only content of the feature page (GET /api/features/content),
  // the slug it was loaded for, and its loading state.
  property var featureContent: null
  property string featureContentSlug: ""
  property bool featureContentPending: false
  property string featureContentError: ""
  // The feature page is pushed on pageStack over its first item; focusItem (the
  // key handler) regains focus when it closes. Panel.qml wires these three.
  property var pageStack: null
  property var page: null
  property var focusItem: null
  readonly property bool featurePageOpen: !!pageStack && !!page && pageStack.currentItem === page
  readonly property var featurePageFeature: featureList.find(function(row) { return row.slug === selectedFeatureSlug }) || null
  readonly property var featurePageSpec: featureSpecs[selectedFeatureSlug] || null
  // A refused or failed action, shown inline on the page.
  readonly property var featurePageRefusal: featuresError !== "" ? { message: featuresError } : null

  // A project switch drops the previous project's list and selection.
  function reset() {
    closeFeaturePage()
    featureList = []
    featuresPending = false
    featuresError = ""
    featureSpecs = ({})
    featureActivity = null
    selectedFeatureSlug = ""
    featureDetailState = null
    planRefusal = null
    featureContent = null
    featureContentSlug = ""
    featureContentPending = false
    featureContentError = ""
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
      if (root.featurePageOpen) root.loadFeatureContent(root.selectedFeatureSlug)
    }, true)
  }

  // The page follows the Features tab: leaving the tab closes it.
  onFeaturesOpenChanged: if (!featuresOpen) closeFeaturePage()

  // Pushes the page for a feature, selects it and loads its content and state.
  function openFeaturePage(feature) {
    if (!feature || !pageStack || !page) return
    root.selectFeature(feature)
    root.featuresError = ""
    root.loadFeatureContent(feature.slug)
    root.loadFeatureState(feature.slug)
    if (!featurePageOpen) {
      page.subTab = "README"
      pageStack.push(page, StackView.Immediate)
    }
    root.focusHandler()
  }

  // Pops back to the tab views; the selection is untouched.
  function closeFeaturePage() {
    if (featurePageOpen) pageStack.pop(null, StackView.Immediate)
    root.focusHandler()
  }

  function focusHandler() {
    if (!focusItem) return
    focusItem.pendingKey = ""
    focusItem.forceActiveFocus()
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

  // Loads the documents, scenarios, milestones and designs of one feature for
  // its page. The previous feature's content is dropped at once; reloading the
  // same feature keeps what is shown until the answer arrives.
  function loadFeatureContent(slug) {
    const revision = projectViewRevision
    if (root.featureContentSlug !== slug) root.featureContent = null
    root.featureContentSlug = slug
    root.featureContentPending = true
    root.featureContentError = ""
    api("GET", "/api/features/content?project=" + encodeURIComponent(root.lastProject)
      + "&slug=" + encodeURIComponent(slug), null, function(resp, status) {
      if (revision !== root.projectViewRevision) return
      if (root.featureContentSlug !== slug) return
      root.featureContentPending = false
      if (status === 200 && resp && typeof resp === "object") {
        root.featureContent = resp
      } else {
        root.featureContent = null
        root.featureContentError = Features.errorMessage(resp)
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
        const wasRunning = !!root.featureActivity && root.featureActivity.status === "running"
        root.featureList = Features.featureRows(resp)
        root.featureSpecs = Features.featureSpecsBySlug(resp)
        root.featureActivity = resp.activity || null
        // A finished activity may have changed what the page shows.
        if (wasRunning && !(resp.activity && resp.activity.status === "running") && root.featurePageOpen)
          root.loadFeatureContent(root.selectedFeatureSlug)
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
