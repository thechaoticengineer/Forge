import QtQuick
import Quickshell
import Quickshell.Io

// Semantic status colors from the active theme's full palette; the shell's
// Color singleton only exposes five roles, so read colors.toml directly.
// Non-visual.
Item {
  id: palette

  property color success: "#4faf72"
  property color working: "#d5a542"
  property color info: "#56a8c7"

  function loadPalette(raw) {
    function grab(key, fallback) {
      const m = String(raw).match(new RegExp('^' + key + '\\s*=\\s*"([^"]+)"', "m"))
      return m ? m[1] : fallback
    }
    success = grab("green", success)
    working = grab("yellow", working)
    info = grab("cyan", info)
  }

  FileView {
    path: Quickshell.env("HOME") + "/.local/state/omarchy/current/theme/colors.toml"
    watchChanges: true
    printErrors: false
    onLoaded: palette.loadPalette(text())
    onFileChanged: reload()
  }
}
