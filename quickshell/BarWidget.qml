import QtQuick
import qs.Commons
import qs.Ui

BarWidget {
  id: root
  moduleName: "dev.omarchy-ai-build-orchestrator"

  readonly property string statusGlyph: {
    if (!engine.connected) return "◇"
    if (engine.engineStatus === "running") return "●"
    if (engine.engineStatus === "blocked" || engine.engineStatus === "waiting_for_user") return "!"
    if (engine.engineStatus === "failed") return "×"
    if (engine.engineStatus === "completed") return "✓"
    return "◈"
  }
  readonly property color statusColor: {
    if (engine.engineStatus === "failed" || engine.engineStatus === "blocked")
      return root.bar ? root.bar.urgent : Color.urgent
    if (engine.engineStatus === "waiting_for_user")
      return root.bar ? root.bar.urgent : Color.urgent
    return root.bar ? root.bar.foreground : Color.foreground
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  EngineConnection { id: engine }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.statusGlyph
    active: engine.connected && engine.engineStatus !== "idle"
    activeColor: root.statusColor
    dimmed: !engine.connected
    tooltipText: engine.connected
      ? "Build orchestrator: " + engine.engineStatus.replace(/_/g, " ")
      : "Build orchestrator: engine offline"

    onPressed: function(mouseButton) {
      if (!root.bar || mouseButton !== Qt.LeftButton) return
      root.bar.run("omarchy-shell shell toggle dev.omarchy-ai-build-orchestrator '{}'")
    }
  }
}
