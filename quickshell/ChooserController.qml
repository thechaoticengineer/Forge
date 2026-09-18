import QtQuick

// Interface: the project chooser's data and row actions. ProjectChooser binds to
// projectsData and manualEntry; Panel.qml passes host (api(), act() and the
// chooserOpen flag it keeps for its keys), the chooser view and its key handler.
// Non-visual.
Item {
  id: controller

  property var host: null
  property var chooser: null
  property var keyTarget: null

  property var projectsData: null
  property bool manualEntry: false

  function openChooser() {
    manualEntry = false
    host.api("GET", "/api/projects", null, function(resp) {
      if (!resp) return
      controller.projectsData = resp
      controller.host.chooserOpen = true
    })
  }

  function chooseRow(row) {
    if (row.kind === "local") {
      host.act("/api/project/select", { path: row.path })
      host.chooserOpen = false
    } else if (row.kind === "remote") {
      host.act("/api/project/select", { repo: row.name })
      host.chooserOpen = false
    } else if (row.kind === "path") {
      manualEntry = !manualEntry
      if (manualEntry) chooser.manualField.forceActiveFocus()
      else keyTarget.forceActiveFocus()
    }
  }
}
