import QtQuick

// Interface: the uncommitted diff viewer's state and its API call. DiffView binds
// to these properties; Panel.qml passes host (api()), the project identity, the
// diff list to rewind on open and whether the window is shown and the project
// busy. A project switch closes the diff and drops the earlier project's text and
// responses. Non-visual.
Item {
  id: controller

  property var host: null
  property string lastProject: ""
  property int projectViewRevision: 0
  property bool active: false
  property bool busy: false
  property var listView: null

  property bool diffOpen: false
  property string diffText: ""
  property bool diffPending: false
  property string diffError: ""

  onProjectViewRevisionChanged: {
    diffOpen = false
    diffText = ""
    diffError = ""
    diffPending = false
  }

  function openDiff() {
    diffText = ""
    diffError = ""
    diffOpen = true
    if (listView) listView.positionViewAtBeginning()
    refreshDiff()
  }

  function refreshDiff() {
    if (diffPending) return
    diffPending = true
    const revision = projectViewRevision
    host.api("GET", "/api/diff?project=" + encodeURIComponent(lastProject), null, function(resp) {
      if (revision !== controller.projectViewRevision) return
      controller.diffPending = false
      if (resp && typeof resp.diff === "string") {
        controller.diffError = ""
        controller.diffText = resp.diff
      } else {
        controller.diffError = "Unable to load diff"
      }
    })
  }

  // An open diff follows the work while the project is busy.
  Timer {
    interval: 3000
    repeat: true
    running: controller.active && controller.diffOpen && controller.busy
    onTriggered: controller.refreshDiff()
  }
}
