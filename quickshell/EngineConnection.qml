import QtQuick
import Quickshell
import Quickshell.Io

Item {
  id: root

  width: 0
  height: 0
  visible: false

  readonly property string runtimeDirectory: Quickshell.env("XDG_RUNTIME_DIR") || ""
  readonly property string socketPath: runtimeDirectory === ""
    ? ""
    : runtimeDirectory + "/omarchy-ai-build-orchestrator/engine.sock"

  readonly property bool connected: socket.connected
  property bool hasSnapshot: false
  property int sequence: 0
  property string engineStatus: "offline"
  property var activeRun: null
  property bool requiresAttention: false
  property string lastError: ""

  signal snapshotChanged()

  function reconnect() {
    if (socketPath === "") {
      lastError = "XDG_RUNTIME_DIR is not available"
      return
    }

    socket.connected = false
    Qt.callLater(function() { socket.connected = true })
  }

  function acceptMessage(line) {
    var message
    try {
      message = JSON.parse(String(line))
    } catch (error) {
      lastError = "The engine sent invalid JSON"
      return
    }

    if (!message || message.version !== 1) {
      lastError = "The engine uses an unsupported protocol version"
      return
    }

    if (message.type === "error") {
      lastError = String(message.message || "The engine rejected a request")
      return
    }

    if (message.type !== "snapshot" || !message.snapshot) return

    var snapshot = message.snapshot
    var validStatuses = [
      "idle",
      "running",
      "blocked",
      "failed",
      "completed",
      "waiting_for_user"
    ]
    if (validStatuses.indexOf(snapshot.status) === -1) {
      lastError = "The engine sent an unknown status"
      return
    }

    sequence = Number(snapshot.sequence || 0)
    engineStatus = snapshot.status
    activeRun = snapshot.active_run || null
    requiresAttention = snapshot.requires_attention === true
    hasSnapshot = true
    lastError = ""
    snapshotChanged()
  }

  Socket {
    id: socket
    path: root.socketPath
    connected: root.socketPath !== ""

    parser: SplitParser {
      splitMarker: "\n"
      onRead: function(line) { root.acceptMessage(line) }
    }

    onConnectionStateChanged: {
      if (connected) {
        retryTimer.stop()
        root.lastError = ""
      } else {
        root.engineStatus = "offline"
        root.lastError = "The orchestration engine is not available"
        retryTimer.restart()
      }
    }
  }

  Timer {
    id: retryTimer
    interval: 2000
    repeat: false
    onTriggered: root.reconnect()
  }
}
