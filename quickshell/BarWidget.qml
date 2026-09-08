import QtQuick
import qs.Commons
import qs.Ui

BarWidget {
  id: root
  moduleName: "dev.omarchy-ai-build-orchestrator"

  property bool engineOnline: false
  property string phase: "offline"
  property var currentStage: null
  property string currentStep: ""
  property string agentRole: ""
  property string agentTool: ""
  property string goal: ""
  property int committedStages: 0
  property int totalStages: 0
  property int queuedCount: 0
  property bool queueActive: false
  property var sessions: []
  property int activeCount: 0
  property int problemCount: 0
  property int totalQueued: 0
  property string lifecycleText: ""

  function lifecycleSummary(state) {
    const cp = state.architecture || {}, activity = state.architect_activity || {};
    let text = "Architect: " + (cp.context_status === "needs_recovery" ? "needs recovery" : activity.status || cp.context_status || "inactive");
    if (activity.reason) text += " · " + activity.reason;
    const stages = state.plan && Array.isArray(state.plan.stages) ? state.plan.stages : [];
    const stage = stages.find(s => s.id === state.current_stage) || stages.find(s => s.status !== "committed");
    if (stage && stage.model_agreement) {
      const a = stage.model_agreement, e = a.effective || {};
      text += "\n" + e.provider + "/" + e.model + " · " + e.native_effort
        + " · " + (a.verification_state || a.availability || "unverified");
      const gate = stage.review_gate || {}, roles = gate.roles || {};
      text += "\nCurrent gate: " + (gate.status || "pending")
        + " · independent: " + (roles.reviewer || "pending")
        + " · architect: " + (roles.architect === "not_required" ? "not required" : roles.architect || "pending");
    }
    return text;
  }

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
    interval: root.phase === "running" || root.phase === "planning" ? 2000 : 5000
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
          root.currentStage = s.current_stage ?? null
          root.currentStep = s.current_step || ""
          root.agentRole = s.agent ? s.agent.role || "" : ""
          root.agentTool = s.agent ? s.agent.tool || "" : ""
          root.goal = s.goal || ""
          root.lifecycleText = root.lifecycleSummary(s)
          const stages = s.plan ? s.plan.stages : []
          root.committedStages = stages.filter(stage => stage.status === "committed").length
          root.totalStages = stages.length
          root.queuedCount = Array.isArray(s.queue)
            ? s.queue.filter(item => item.status === "queued").length : 0
          root.queueActive = s.queue_active === true
          root.sessions = Array.isArray(s.sessions) ? s.sessions : []
          root.activeCount = Array.isArray(s.sessions)
            ? s.sessions.filter(session => session.busy || session.queue_active).length
            : (s.busy || root.queueActive ? 1 : 0)
          root.problemCount = Array.isArray(s.sessions)
            ? s.sessions.filter(session => session.phase === "blocked" || session.phase === "failed").length
            : (s.phase === "blocked" || s.phase === "failed" ? 1 : 0)
          root.totalQueued = Array.isArray(s.sessions)
            ? s.sessions.reduce((total, session) => total + session.queued, 0)
            : root.queuedCount
          root.engineOnline = true
          root.phase = s.phase
        } catch (e) {
          root.engineOnline = false
          root.phase = "offline"
        }
      }
      xhr.send()
    }
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: (root.engineOnline && root.activeCount > 1
      ? root.statusGlyph + " " + root.activeCount
      : root.phase === "running" && root.totalStages > 0
      ? root.statusGlyph + " " + root.committedStages + "/" + root.totalStages
      : root.statusGlyph)
      + (root.engineOnline && root.totalQueued > 0 ? " +" + root.totalQueued : "")
    active: root.engineOnline && (root.phase !== "idle"
      || (root.sessions.length > 0 && (root.activeCount > 0 || root.problemCount > 0)))
    activeColor: root.engineOnline && root.problemCount > 0
      ? (root.bar ? root.bar.urgent : Color.urgent)
      : (root.bar ? root.bar.foreground : Color.foreground)
    dimmed: !root.engineOnline
    tooltipText: {
      if (!root.engineOnline) return "Forge: engine offline"
      let tooltip = "Forge: " + root.phase
      if (root.lifecycleText) tooltip += "\n" + root.lifecycleText
      if (root.phase === "running") {
        if (root.currentStage !== null)
          tooltip += "\nstage " + root.currentStage + ": " + root.currentStep
        if (root.agentRole)
          tooltip += "\n" + root.agentRole + " · " + root.agentTool
      }
      if (root.goal && root.phase !== "idle")
        tooltip += "\ngoal: " + root.goal.slice(0, 80)
      if (root.queuedCount > 0 || root.queueActive)
        tooltip += "\nqueue: " + root.queuedCount + " waiting" + (root.queueActive ? " · active" : "")
      for (const session of root.sessions) {
        if (session.phase !== "idle")
          tooltip += "\n" + session.name + ": " + session.phase
            + (session.current_step ? " · " + session.current_step : "")
      }
      return tooltip
    }

    onPressed: function(mouseButton) {
      if (!root.bar || mouseButton !== Qt.LeftButton) return
      root.bar.run("omarchy-shell shell toggle dev.omarchy-ai-build-orchestrator '{}'")
    }
  }
}
