import QtQuick
import qs.Commons
import qs.Ui

BarWidget {
  id: root
  moduleName: "dev.omarchy-ai-build-orchestrator"

  property bool engineOnline: false
  property string phase: "offline"

  readonly property string statusGlyph: {
    if (!engineOnline) return "◇"
    if (phase === "running" || phase === "planning") return "●"
    if (phase === "blocked") return "!"
    if (phase === "failed") return "×"
    if (phase === "done") return "✓"
    return "◈"
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  Timer {
    interval: 5000
    repeat: true
    running: true
    triggeredOnStart: true
    onTriggered: {
      const xhr = new XMLHttpRequest()
      xhr.open("GET", "http://127.0.0.1:8734/api/state")
      xhr.onreadystatechange = function() {
        if (xhr.readyState !== XMLHttpRequest.DONE) return
        if (xhr.status !== 200) {
          root.engineOnline = false
          root.phase = "offline"
          return
        }
        try {
          const s = JSON.parse(xhr.responseText)
          root.engineOnline = true
          root.phase = s.phase
        } catch (e) {
          root.engineOnline = false
        }
      }
      xhr.send()
    }
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.statusGlyph
    active: root.engineOnline && root.phase !== "idle"
    activeColor: root.phase === "failed" || root.phase === "blocked"
      ? (root.bar ? root.bar.urgent : Color.urgent)
      : (root.bar ? root.bar.foreground : Color.foreground)
    dimmed: !root.engineOnline
    tooltipText: root.engineOnline
      ? "Forge: " + root.phase
      : "Forge: engine offline"

    onPressed: function(mouseButton) {
      if (!root.bar || mouseButton !== Qt.LeftButton) return
      root.bar.run("omarchy-shell shell toggle dev.omarchy-ai-build-orchestrator '{}'")
    }
  }
}
