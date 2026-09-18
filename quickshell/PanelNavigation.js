.pragma library

// Panel navigation: the tab order, tab labels, view keys, session markers, the
// keyboard help rows and the bottom hint line. Pure JS, so node tests load it too.

var tabs = ["overview", "plan", "activity", "architecture", "features", "queue", "settings"]

var tabLabels = {
  overview: "Overview",
  plan: "Plan",
  activity: "Activity",
  architecture: "Architecture",
  features: "Features",
  queue: "Queue",
  settings: "Settings"
}

// g followed by one of these letters jumps to a tab; gg keeps its own meaning.
var gKeys = {
  o: "overview",
  p: "plan",
  a: "activity",
  r: "architecture",
  f: "features",
  q: "queue",
  s: "settings"
}

function tabLabel(id, queueCount) {
  const label = tabLabels[id] || ""
  return id === "queue" && queueCount > 0 ? label + " " + queueCount : label
}

// The previous (step -1) or next (step +1) tab, wrapping around.
function stepTab(id, step) {
  const at = Math.max(0, tabs.indexOf(id))
  return tabs[(at + step % tabs.length + tabs.length) % tabs.length]
}

function tabForGKey(letter) {
  return Object.prototype.hasOwnProperty.call(gKeys, letter) ? gKeys[letter] : ""
}

// The status marker the project tabs showed: ● busy, ! needs attention, ✓ done,
// · otherwise, followed by +N when goals are queued.
function sessionMarker(session) {
  const needsAttention = session.phase === "blocked" || session.phase === "failed"
  return (session.busy || session.queue_active ? "●" : needsAttention ? "!"
    : session.phase === "done" ? "✓" : "·")
    + (session.queued > 0 ? " +" + session.queued : "")
}

var viewHints = {
  overview: "i goal · p plan · a approve · r run · x stop · t discuss",
  plan: "j/k stage · Enter open · e edit · I feedback",
  activity: "Tab/h/l live/history · 1-6 filter · Ctrl+d/u scroll · j/k report",
  architecture: "PgDn/PgUp scroll · Home/End",
  features: "j/k feature · Enter open · n new · c chat · v review · a approve",
  queue: "x stop · d diff · c project",
  settings: "c project · f features · d diff"
}

function hintText(tab, insertMode) {
  if (insertMode) return "INSERT - Esc to normal mode"
  return "NORMAL · " + (viewHints[tab] || viewHints.overview) + " · [ ] or g o/p/a/r/f/q/s views · ? help"
}

function helpRows() {
  return [
    { key: "", description: "Panel · normal mode" },
    { key: "[ / ]", description: "Switch to the previous / next view tab" },
    { key: "g o / p / a / r / f / q / s", description: "Go to Overview / Plan / Activity / Architecture / Features / Queue / Settings" },
    { key: "i", description: "Edit the goal (insert mode)" },
    { key: "I", description: "Edit plan feedback (insert mode); Enter improves with AI" },
    { key: "Escape", description: "Leave a text field, close the top overlay, or cancel plan editing" },
    { key: "j / k", description: "Select next / previous stage (report in Reports)" },
    { key: "gg / G", description: "Select first / last stage (report in Reports)" },
    { key: "Enter / o / Space", description: "Expand or collapse selected stage or report; focus stage title when editing" },
    { key: "Tab", description: "Toggle Live / History" },
    { key: "h / l", description: "Select Live / History" },
    { key: "Ctrl+d / Ctrl+u", description: "Scroll Live / History half a page down / up" },
    { key: "Page Down / Page Up", description: "Scroll the whole panel down / up" },
    { key: "Home / End", description: "Jump to the top / bottom of the panel" },
    { key: "1 / 2 / 3 / 4 / 5", description: "History: All / Runs / Git / Reviews / Errors" },
    { key: "6", description: "History: Reports (when available)" },
    { key: "p", description: "Create plan from goal" },
    { key: "t", description: "Open the discussion chat" },
    { key: "P", description: "Create plan from discussion" },
    { key: "E", description: "Enhance the goal description with AI" },
    { key: "e", description: "Edit plan stages by hand" },
    { key: "a", description: "Approve draft plan" },
    { key: "r", description: "Run approved or completed plan" },
    { key: "x", description: "Stop run or active queue" },
    { key: "d", description: "Open uncommitted diff" },
    { key: "c", description: "Change project" },
    { key: "f", description: "Open the feature specs list" },
    { key: "? (Shift+/) / F1", description: "Open keyboard help" },
    { key: "", description: "Diff viewer" },
    { key: "j / k", description: "Scroll down / up" },
    { key: "Ctrl+d / Ctrl+u", description: "Scroll half a page down / up" },
    { key: "gg / G", description: "Jump to top / bottom" },
    { key: "R", description: "Refresh diff" },
    { key: "q / Escape", description: "Close diff" },
    { key: "", description: "Feature specs list" },
    { key: "j / k", description: "Select next / previous feature" },
    { key: "Enter / o", description: "Open the selected feature in nvim" },
    { key: "R", description: "Refresh the feature list" },
    { key: "n", description: "New feature (slug and title form)" },
    { key: "c", description: "Chat with the co-authoring agent about the selected feature" },
    { key: "v", description: "Request an architect spec review of the selected feature" },
    { key: "a / A", description: "Approve the selected feature's spec / scenarios" },
    { key: "q / Escape", description: "Close the feature list" },
    { key: "", description: "Project chooser · normal mode" },
    { key: "j / k", description: "Select next / previous project" },
    { key: "Enter", description: "Open selection; in filter, open first match; in path field, set path" },
    { key: "/ / i", description: "Edit project filter (insert mode)" },
    { key: "q / Escape", description: "Close chooser (Escape leaves a text field first)" },
    { key: "", description: "Discussion chat" },
    { key: "i", description: "Edit the message (insert mode)" },
    { key: "Enter / Shift+Enter", description: "Send the message / insert a newline" },
    { key: "q / Escape", description: "Close the chat (Escape leaves the message field first)" },
    { key: "", description: "Keyboard help" },
    { key: "? (Shift+/) / F1 / q / Escape", description: "Close help before any other overlay" }
  ]
}
