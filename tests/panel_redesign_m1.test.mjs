// Executable scenarios for panel-redesign milestone M1 (navigation shell and views).
// Covers S1, S2, S3, S4 (source side), S5 (source side), S6, S8, S9, S10, S11,
// S16, S17, S18, S19, S20, S21 and S22 of docs/features/panel-redesign/scenarios.md.
// The rendered behaviour of the qs-free components is covered by tests/qml/tst_panel_*.qml;
// this suite owns the pure-JS contract modules (PanelNavigation.js, PanelActions.js) and the
// source-level contracts of Panel.qml and the Qt-dependent view wrappers, which qmltestrunner
// cannot load because they import Quickshell / qs.Commons / qs.Ui.
//
// The OLD_* constants below are frozen copies of today's Panel.qml (help rows, action guards,
// endpoints, labels, session marker formula, key constants). They are the behavioural oracle
// for the redesign (decision D3: move, never remove) and must not be edited to fit new code.
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import vm from 'node:vm';

const quickshell = new URL('../quickshell/', import.meta.url);
const plain = value => JSON.parse(JSON.stringify(value));

function readSource(name) {
  assert.ok(existsSync(new URL(name, quickshell)), `quickshell/${name} must exist`);
  return readFileSync(new URL(name, quickshell), 'utf8');
}

// Loads a `.pragma library` module into a fresh context and returns accessors for its
// top-level names (const/let declarations are not context properties, so read them by name).
function loadModule(name) {
  const source = readSource(name).replace(/^\s*\.pragma\s+library\s*$/m, '');
  const context = vm.createContext({});
  vm.runInContext(source, context, { filename: name });
  const get = symbol => plain(vm.runInContext(
    `typeof ${symbol} === 'undefined' ? undefined : ${symbol}`, context));
  const call = (symbol, ...args) => {
    assert.equal(vm.runInContext(`typeof ${symbol}`, context), 'function', `${name} must define ${symbol}()`);
    context.__args = args;
    return plain(vm.runInContext(`${symbol}(...__args)`, context));
  };
  return { get, call };
}

// ---- source slicing ------------------------------------------------------------------
// Returns the text of the brace-matched block that opens at the first '{' at or after `from`.
// Skips strings and comments so braces inside them do not confuse the match.
function blockFrom(source, from) {
  const open = source.indexOf('{', from);
  assert.ok(open >= 0, 'expected an opening brace');
  let depth = 0;
  for (let i = open; i < source.length; i++) {
    const ch = source[i];
    if (ch === '"' || ch === "'") {
      for (i++; i < source.length && source[i] !== ch; i++) if (source[i] === '\\') i++;
    } else if (ch === '/' && source[i + 1] === '/') {
      while (i < source.length && source[i] !== '\n') i++;
    } else if (ch === '/' && source[i + 1] === '*') {
      i = source.indexOf('*/', i + 2);
      if (i < 0) break;
      i++;
    } else if (ch === '{') depth++;
    else if (ch === '}' && --depth === 0) return source.slice(open, i + 1);
  }
  assert.fail('unbalanced braces');
}

function blockAfter(source, marker, what = marker) {
  const at = source.indexOf(marker);
  assert.ok(at >= 0, `${what} must exist`);
  return blockFrom(source, at + marker.length - (marker.endsWith('{') ? 1 : 0));
}

// Every instance block `Type { ... }` of a QML type, e.g. instances(panel, 'OverviewView').
function instances(source, type) {
  const found = [];
  const pattern = new RegExp(`(^|[\\s;])${type}\\s*\\{`, 'gm');
  for (let m; (m = pattern.exec(source));) found.push(blockFrom(source, m.index + m[0].length - 1));
  return found;
}

// The single instance block of `type`; fails with a clear message when it is missing.
function only(source, type) {
  const found = instances(source, type);
  assert.equal(found.length, 1, `${type} must be instantiated exactly once (found ${found.length})`);
  return found[0];
}

const assignsCurrentTab = text =>
  /\bcurrentTab\s*=[^=]/.test(text)
  || /\b(selectTab|showTab|setTab|goToTab|openTab)\s*\(/.test(text);

const panelSource = () => readSource('Panel.qml');

// Overlay and view keys moved out of Panel.qml into the views' handleKey() functions,
// which the keyHandler calls; the routing contract spans both.
const handleKeySource = file => blockAfter(readSource(file), 'function handleKey(');
const viewKeySource = () => ['DiffView.qml', 'ProjectChooser.qml', 'FeaturesView.qml', 'SettingsView.qml']
  .map(handleKeySource).join('\n');

// The normal-mode key handler of the root item.
const keyHandlerBody = source => {
  const at = source.indexOf('id: keyHandler');
  assert.ok(at >= 0, 'Panel.qml must keep the keyHandler item');
  return blockAfter(source.slice(at), 'Keys.onPressed: event => {', 'keyHandler Keys.onPressed');
};

// ---- frozen oracle: today's Panel.qml -------------------------------------------------
const OLD_HELP_ROWS = [
  { key: "", description: "Panel · normal mode" },
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
  { key: "? (Shift+/) / F1 / q / Escape", description: "Close help before any other overlay" },
];

// Enabled expressions of the old PanelButtons and their keyboard mirrors, verbatim.
const OLD_GUARDS = {
  createPlan: f => !f.editingPlan && !f.revisePending && !f.busy && f.goalText.trim() !== '',
  enhance: f => f.engineOnline && !f.busy && !f.editingPlan && !f.revisePending
    && !f.goalEnhancePending && f.goalText.trim() !== '',
  refactor: f => !f.editingPlan && !f.revisePending && !f.busy && f.engineOnline,
  addToQueue: f => f.engineOnline && f.goalText.trim() !== '',
  approve: f => !f.editingPlan && !f.revisePending && !f.busy
    && f.planStatus !== null && f.planStatus === 'draft',
  run: f => !f.editingPlan && !f.revisePending && !f.busy && f.planStatus !== null
    && (f.planStatus === 'approved' || f.planStatus === 'done'),
  stop: f => f.phase === 'running' || f.queueActive,
  editPlan: f => !f.editingPlan && !f.revisePending && f.engineOnline && !f.busy && !f.queueActive
    && f.planStatus !== null && ['draft', 'approved', 'done'].indexOf(f.planStatus) !== -1,
  discard: f => !f.editingPlan && !f.revisePending && !f.busy && f.planStatus !== null,
  diff: f => f.engineOnline,
  features: f => f.engineOnline,
  update: f => f.engineOnline && !f.busy,
  improve: f => f.engineOnline && !f.busy && !f.queueActive
    && !f.editingPlan && !f.revisePending && f.planStatus !== null
    && ['draft', 'approved', 'done'].indexOf(f.planStatus) !== -1
    && f.feedbackText.trim() !== '',
  ask: f => f.engineOnline && !f.busy && !f.queueActive
    && !f.editingPlan && !f.revisePending && !f.chatPending
    && f.planStatus !== null && f.questionText.trim() !== '',
  startQueue: f => !f.editingPlan && f.engineOnline && !f.busy && !f.queueActive && f.hasQueuedGoals,
  changeProject: f => f.engineOnline,
};

const OLD_ENDPOINTS = {
  createPlan: '/api/plan', refactor: '/api/plan', enhance: '/api/goal/enhance',
  addToQueue: '/api/queue/add', approve: '/api/approve', run: '/api/run', stop: '/api/stop',
  discard: '/api/reset_plan', update: '/api/self_update', improve: '/api/plan/revise',
  ask: '/api/plan/chat', startQueue: '/api/queue/start',
};

// Every PanelButton label in today's Panel.qml, normalised: dynamic values are dropped
// ("planner: claude" -> "planner") and toggle variants use their resting label.
const OLD_BUTTON_LABELS = [
  '? Help', 'Change project',
  'planner', 'architect', 'automatic routing', 'implementer', 'reviewer',
  'architect review', 'reviewer review', 'push at end', 'auto-approve',
  'Refresh models', 'Cancel refresh', 'Refresh Claude limits', 'Model settings & options',
  'Create plan', 'Enhance with AI', 'Apply AI description', 'Undo enhance', 'Refactor plan',
  'Add to queue', 'Plan is OK — approve', 'Start implementing', 'Stop', 'Edit plan',
  'Discard plan', 'View diff', 'Features', 'Update Forge', 'Discuss before planning',
  'Improve with AI', 'Plan Q&A', 'Ask', 'Start queue', '↑', '↓', '×',
];
// Non-button indicators of today's page that must also keep a home.
const OLD_INDICATORS = [
  'project session markers', 'phase badge', 'current step', 'background project activity',
  'now working card', 'quota summary', 'catalogue metadata', 'model policy error',
  'goal enhancement status', 'discussion status', 'architecture card', 'role token totals',
  'plan review status', 'stage list', 'live output', 'history', 'reports', 'queue list',
];
const LOCATION_IDS = ['overview', 'plan', 'activity', 'architecture', 'features', 'queue',
  'settings', 'overflow', 'header'];

// Session marker exactly as the old project tabs computed it.
function oldSessionMarker(s) {
  const needsAttention = s.phase === 'blocked' || s.phase === 'failed';
  return (s.busy || s.queue_active ? '●' : needsAttention ? '!' : s.phase === 'done' ? '✓' : '·')
    + (s.queued > 0 ? ' +' + s.queued : '');
}

const TAB_IDS = ['overview', 'plan', 'activity', 'architecture', 'features', 'queue', 'settings'];
const VIEW_FILES = {
  overview: 'OverviewView', plan: 'PlanView', activity: 'ActivityView',
  architecture: 'ArchitectureView', features: 'FeaturesView', queue: 'QueueView',
  settings: 'SettingsView',
};

const BASE_FLAGS = {
  engineOnline: true, busy: false, phase: 'idle', queueActive: false, editingPlan: false,
  revisePending: false, chatPending: false, planStatus: null, goalText: '', feedbackText: '',
  questionText: '', goalEnhancePending: false, hasQueuedGoals: false,
};
const flags = extra => ({ ...BASE_FLAGS, ...extra });

// Realistic flag sets per engine phase (busy mirrors Panel.qml: planning or running).
const PHASE_FIXTURES = {
  idle: flags({ phase: 'idle', goalText: 'Add a dark mode toggle to the settings page' }),
  planning: flags({ phase: 'planning', busy: true, goalText: 'Add a dark mode toggle to the settings page' }),
  plan_ready: flags({ phase: 'plan_ready', planStatus: 'draft' }),
  awaiting_approval: flags({ phase: 'awaiting_approval', planStatus: 'draft' }),
  running: flags({ phase: 'running', busy: true, planStatus: 'approved' }),
  blocked: flags({ phase: 'blocked', planStatus: 'approved' }),
  failed: flags({ phase: 'failed', planStatus: 'approved' }),
  done: flags({ phase: 'done', planStatus: 'done' }),
};
const QUEUE_RUNNING = flags({
  phase: 'running', busy: true, queueActive: true, hasQueuedGoals: true, planStatus: 'approved',
});
const ALL_FIXTURES = { ...PHASE_FIXTURES, queue_running: QUEUE_RUNNING };

// ---- S1, S2, S3: navigation module -----------------------------------------------------
test('S1: the tab bar lists Overview, Plan, Activity, Architecture, Features, Queue, Settings in order', () => {
  const nav = loadModule('PanelNavigation.js');
  assert.deepEqual(nav.get('tabs'), TAB_IDS);
  const labels = TAB_IDS.map(id => nav.call('tabLabel', id, 0));
  assert.deepEqual(labels,
    ['Overview', 'Plan', 'Activity', 'Architecture', 'Features', 'Queue', 'Settings']);
});

test('S1: PanelHeader and PanelTabBar sit outside every per-view scroll area in Panel.qml', () => {
  const panel = panelSource();
  assert.equal(instances(panel, 'PanelHeader').length, 1, 'Panel.qml must instantiate PanelHeader once');
  assert.equal(instances(panel, 'PanelTabBar').length, 1, 'Panel.qml must instantiate PanelTabBar once');
  for (const type of ['Flickable', 'ScrollView']) {
    for (const scroll of instances(panel, type)) {
      assert.ok(!/\bPanelHeader\s*\{/.test(scroll), `PanelHeader must not be inside a ${type}`);
      assert.ok(!/\bPanelTabBar\s*\{/.test(scroll), `PanelTabBar must not be inside a ${type}`);
    }
  }
});

test('S1: the header shows FORGE, switcher, phase badge, current step and the overflow button', () => {
  const header = readSource('PanelHeader.qml');
  for (const name of ['forgeTitle', 'projectSwitcher', 'phaseBadge', 'currentStep', 'overflowButton'])
    assert.ok(header.includes(`objectName: "${name}"`), `PanelHeader.qml must name ${name}`);
});

test('S2: Panel.qml starts on Overview, follows tabRequested and feeds currentTab to the tab bar', () => {
  const panel = panelSource();
  assert.match(panel, /property string currentTab:\s*"overview"/);
  const bar = only(panel, 'PanelTabBar');
  assert.match(bar, /currentTab:\s*(root\.)?currentTab/);
  assert.match(bar, /queueCount:/);
  assert.match(bar, /onTabRequested/);
});

test('S2: exactly one persistent view is visible per tab', () => {
  const panel = panelSource();
  for (const [id, type] of Object.entries(VIEW_FILES)) {
    const found = instances(panel, type);
    assert.equal(found.length, 1, `${type} must be instantiated exactly once`);
    assert.match(found[0], new RegExp(`visible:[^\\n]*currentTab\\s*===\\s*["']${id}["']`),
      `${type} must be shown only while currentTab is ${id}`);
  }
});

test('S3: ] and [ step to the next and previous tab and wrap around', () => {
  const nav = loadModule('PanelNavigation.js');
  assert.equal(nav.call('stepTab', 'overview', 1), 'plan');
  assert.equal(nav.call('stepTab', 'plan', -1), 'overview');
  TAB_IDS.forEach((id, i) => {
    assert.equal(nav.call('stepTab', id, 1), TAB_IDS[(i + 1) % TAB_IDS.length], `] from ${id}`);
    assert.equal(nav.call('stepTab', id, -1),
      TAB_IDS[(i + TAB_IDS.length - 1) % TAB_IDS.length], `[ from ${id}`);
  });
  assert.equal(nav.call('stepTab', 'settings', 1), 'overview');
  assert.equal(nav.call('stepTab', 'overview', -1), 'settings');
});

test('S3: g followed by o, p, a, r, f, q, s selects the matching tab; anything else selects none', () => {
  const nav = loadModule('PanelNavigation.js');
  const expected = {
    o: 'overview', p: 'plan', a: 'activity', r: 'architecture', f: 'features', q: 'queue', s: 'settings',
  };
  for (const [letter, tab] of Object.entries(expected))
    assert.equal(nav.call('tabForGKey', letter), tab, `g ${letter}`);
  for (const other of ['g', 'x', 'O', 'P', '1', '', ' ', 'j', 'i'])
    assert.equal(nav.call('tabForGKey', other), '', `g ${JSON.stringify(other)} must select nothing`);
});

test('S3: keyHandler handles [ and ] and the g prefix while gg keeps selecting the first stage or report', () => {
  const body = keyHandlerBody(panelSource());
  assert.match(body, /Qt\.Key_BracketLeft/);
  assert.match(body, /Qt\.Key_BracketRight/);
  assert.match(body, /PanelNavigation\.stepTab\(/);
  assert.match(body, /PanelNavigation\.tabForGKey\(/);
  assert.match(body, /prefix === "g"/);
  // gg is unchanged: first report in Reports, otherwise first stage, and a lone g arms the prefix.
  const flat = body.replace(/\s+/g, ' ');
  assert.ok(flat.includes('if (root.reportsVisible) selectReport(0) else selectStage(0)'),
    'gg must still select the first report or stage');
  assert.ok(flat.includes('pendingKey = "g"'), 'a lone g must still arm the g prefix');
});

// ---- S4, S5: overview source contracts -------------------------------------------------
test('S4: Overview is a qs-free view with its now-working card, progress summary and stage rows', () => {
  const overview = readSource('OverviewView.qml');
  for (const name of ['nowWorkingCard', 'progressSummary', 'overviewStageRow', 'lastRunCard'])
    assert.ok(overview.includes(`objectName: "${name}"`), `OverviewView.qml must name ${name}`);
  assert.ok(!/Review policy|Routing:|Execution:/.test(overview),
    'Overview stage rows must not carry routing or review policy lines');
  assert.ok(!/^\s*import\s+(Quickshell|qs\.)/m.test(overview),
    'OverviewView.qml must stay qs-free so qmltestrunner can load it');
});

test('S5: pressing i focuses the Overview goal field through the view', () => {
  const panel = panelSource();
  assert.equal(instances(panel, 'OverviewView').length, 1, 'Panel.qml must instantiate OverviewView');
  const body = keyHandlerBody(panel);
  const at = [...body.matchAll(/Qt\.Key_I && event\.modifiers === Qt\.NoModifier/g)].map(m => m.index);
  assert.ok(at.some(i => /focusGoal\(\)|goalField\.forceActiveFocus\(\)/.test(body.slice(i, i + 400))),
    'the i key must focus the goal field');
  assert.match(readSource('OverviewView.qml'), /function focusGoal\(\)/);
});

// ---- S6: overview actions follow the phase ------------------------------------------------
function everyFlagSet() {
  const combos = [];
  const bools = ['engineOnline', 'busy', 'queueActive', 'editingPlan', 'revisePending',
    'chatPending', 'goalEnhancePending', 'hasQueuedGoals'];
  const statuses = [null, 'draft', 'approved', 'done', 'running'];
  const phases = Object.keys(PHASE_FIXTURES);
  const texts = ['', '   ', 'Refactor the queue module'];
  let n = 0;
  for (let mask = 0; mask < 1 << bools.length; mask++) {
    for (const planStatus of statuses) {
      for (const phase of phases) {
        const f = flags({ planStatus, phase });
        bools.forEach((name, i) => { f[name] = !!(mask & (1 << i)); });
        f.goalText = texts[n % 3];
        f.feedbackText = texts[(n + 1) % 3];
        f.questionText = texts[(n + 2) % 3];
        n++;
        combos.push(f);
      }
    }
  }
  return combos;
}

test('S6: guards(f) reproduces every old enabled expression for every flag combination', () => {
  const actions = loadModule('PanelActions.js');
  let checked = 0;
  for (const f of everyFlagSet()) {
    const got = actions.call('guards', f);
    for (const [name, oracle] of Object.entries(OLD_GUARDS)) {
      assert.strictEqual(got[name], oracle(f), `guards.${name} for ${JSON.stringify(f)}`);
      checked++;
    }
  }
  assert.ok(checked > 100000);
});

test('S6: guards(f) also holds for each realistic engine phase', () => {
  const actions = loadModule('PanelActions.js');
  for (const [phase, f] of Object.entries(ALL_FIXTURES)) {
    const got = actions.call('guards', f);
    for (const [name, oracle] of Object.entries(OLD_GUARDS))
      assert.strictEqual(got[name], oracle(f), `${phase}: guards.${name}`);
  }
});

test('S6: Overview lists an action only when its guard is true, in every phase', () => {
  const actions = loadModule('PanelActions.js');
  for (const [phase, f] of Object.entries(ALL_FIXTURES)) {
    const guards = actions.call('guards', f);
    const ids = actions.call('overviewActions', f, guards);
    assert.ok(Array.isArray(ids), `${phase}: overviewActions must return a list`);
    assert.equal(new Set(ids).size, ids.length, `${phase}: no duplicate action ids`);
    for (const id of ids) {
      if (id in guards) assert.strictEqual(guards[id], true, `${phase}: ${id} is shown but disabled`);
      else assert.match(id, /^(discuss|.*activity.*|.*diff.*)$/i,
        `${phase}: ${id} has no guard, so it must be discuss or a navigation link`);
    }
  }
});

test('S6: idle Overview with a goal offers Create plan, Discuss first, Enhance with AI and Add to queue', () => {
  const actions = loadModule('PanelActions.js');
  const f = PHASE_FIXTURES.idle;
  assert.deepEqual(actions.call('overviewActions', f, actions.call('guards', f)),
    ['createPlan', 'discuss', 'enhance', 'addToQueue']);
});

test('S6: a draft plan offers Approve and Edit, an approved or done plan offers Start and Edit', () => {
  const actions = loadModule('PanelActions.js');
  for (const phase of ['plan_ready', 'awaiting_approval']) {
    const f = PHASE_FIXTURES[phase];
    assert.deepEqual(actions.call('overviewActions', f, actions.call('guards', f)),
      ['approve', 'editPlan'], phase);
  }
  for (const f of [flags({ phase: 'idle', planStatus: 'approved' }), PHASE_FIXTURES.done]) {
    assert.deepEqual(actions.call('overviewActions', f, actions.call('guards', f)),
      ['run', 'editPlan'], `plan ${f.planStatus}`);
  }
});

test('S6: a running plan or an active queue offers Stop, and other phases never do', () => {
  const actions = loadModule('PanelActions.js');
  for (const f of [PHASE_FIXTURES.running, QUEUE_RUNNING]) {
    const ids = actions.call('overviewActions', f, actions.call('guards', f));
    assert.ok(ids.includes('stop'), `stop must be offered while ${f.phase}`);
    assert.ok(!ids.includes('run') && !ids.includes('approve') && !ids.includes('createPlan'));
  }
  for (const phase of ['idle', 'plan_ready', 'awaiting_approval', 'done']) {
    const f = PHASE_FIXTURES[phase];
    assert.ok(!actions.call('overviewActions', f, actions.call('guards', f)).includes('stop'), phase);
  }
});

test('S6: actions keep the API endpoints they had before the redesign', () => {
  const actions = loadModule('PanelActions.js');
  assert.deepEqual(actions.get('endpoints'), OLD_ENDPOINTS);
});

test('S6: Panel.qml feeds Overview its phase-driven action list and handles the requests', () => {
  const overview = only(panelSource(), 'OverviewView');
  assert.ok(overview, 'Panel.qml must instantiate OverviewView');
  assert.match(overview, /actions:[^\n]*(overviewActions|PanelActions)/);
  assert.match(overview, /onActionRequested/);
  assert.match(overview, /onOpenTab/);
});

test('S6: every old action label and indicator has a home after the redesign', () => {
  const actions = loadModule('PanelActions.js');
  const locations = actions.get('locations');
  assert.ok(locations && typeof locations === 'object', 'PanelActions.locations must be a map');
  const where = key => [].concat(locations[key] ?? []);
  for (const key of [...OLD_BUTTON_LABELS, ...OLD_INDICATORS]) {
    assert.ok(key in locations, `locations must cover ${JSON.stringify(key)}`);
    assert.ok(where(key).length > 0, `${key} needs at least one location`);
    for (const place of where(key)) assert.ok(LOCATION_IDS.includes(place), `${key}: unknown location ${place}`);
  }
});

test('S6: moved actions live where the README says they do', () => {
  const actions = loadModule('PanelActions.js');
  const locations = actions.get('locations');
  const where = key => [].concat(locations[key] ?? []);
  const expectations = [
    ['overview', ['Create plan', 'Enhance with AI', 'Add to queue', 'Discuss before planning', 'Stop',
      'Plan is OK — approve', 'Start implementing', 'Apply AI description', 'Undo enhance']],
    ['plan', ['Improve with AI', 'Plan Q&A', 'Ask', 'stage list']],
    ['queue', ['Start queue', '↑', '↓', '×', 'queue list']],
    ['features', ['Features']],
    ['settings', ['planner', 'architect', 'automatic routing', 'implementer', 'reviewer',
      'architect review', 'reviewer review', 'push at end', 'auto-approve', 'Refresh models',
      'Cancel refresh', 'Refresh Claude limits', 'Model settings & options', 'Update Forge',
      'Change project', 'quota summary', 'catalogue metadata', 'model policy error']],
    ['overflow', ['Update Forge', 'Discard plan', 'Refactor plan', 'View diff', 'Change project', '? Help']],
    ['activity', ['live output', 'history', 'reports']],
    ['architecture', ['architecture card', 'role token totals', 'plan review status']],
    ['header', ['phase badge', 'current step', 'project session markers']],
  ];
  for (const [place, keys] of expectations)
    for (const key of keys) assert.ok(where(key).includes(place), `${key} must live in ${place}`);
  // Nothing settings-like or maintenance-like remains on Overview (S9).
  for (const key of ['Update Forge', 'Refresh models', 'Refresh Claude limits', 'Model settings & options',
    'Change project', 'quota summary', 'catalogue metadata'])
    assert.ok(!where(key).includes('overview'), `${key} must not appear on Overview`);
});

// ---- S8, S9: settings ---------------------------------------------------------------------
test('S8: Panel.qml routes every settings row through its old API calls', () => {
  const panel = panelSource();
  const settings = only(panel, 'SettingsView');
  assert.ok(settings, 'Panel.qml must instantiate SettingsView');
  assert.match(settings, /onSettingActivated/);
  assert.match(settings, /onActionRequested/);
  for (const key of ['planner', 'architect', 'implementer', 'reviewer', 'automatic_routing',
    'architect_review', 'reviewer_review', 'auto_push', 'queue_auto_approve'])
    assert.ok(new RegExp(`\\b${key}\\b`).test(panel), `Panel.qml must still handle the ${key} setting`);
  for (const fn of ['cycleTool', 'cycleReviewer', 'toggleReviewCadence', 'reviewerLabel'])
    assert.ok(panel.includes(`function ${fn}(`), `Panel.qml must keep ${fn}()`);
  for (const endpoint of ['/api/settings', '/api/models/refresh', '/api/models/cancel', '/api/quota/refresh'])
    assert.ok(panel.includes(endpoint), `Panel.qml must still call ${endpoint}`);
});

test('S9: Overview never shows models, limits or maintenance actions', () => {
  const overview = readSource('OverviewView.qml');
  assert.ok(!/Refresh models|Refresh Claude limits|Update Forge|Change project|Model settings|quota|catalogue/i
    .test(overview), 'model, limit and maintenance controls belong to Settings and the ⋯ menu');
});

// ---- S10: overflow menu ---------------------------------------------------------------------
test('S10: the overflow menu lists Update Forge, Discard plan, Refactor plan, View diff, Change project and Keyboard help', () => {
  const actions = loadModule('PanelActions.js');
  for (const [phase, f] of Object.entries(ALL_FIXTURES)) {
    const items = actions.call('overflowItems', f);
    assert.deepEqual(items.map(i => i.id),
      ['update', 'discard', 'refactor', 'diff', 'changeProject', 'help'], phase);
    assert.deepEqual(items.map(i => i.label), ['Update Forge', 'Discard plan', 'Refactor plan',
      'View diff', 'Change project', 'Keyboard help'], phase);
  }
});

test('S10: overflow items are enabled by exactly the old guards, and help is always enabled', () => {
  const actions = loadModule('PanelActions.js');
  for (const f of everyFlagSet().filter((_, i) => i % 7 === 0)) {
    const enabled = Object.fromEntries(actions.call('overflowItems', f).map(i => [i.id, i.enabled]));
    for (const id of ['update', 'discard', 'refactor', 'diff', 'changeProject'])
      assert.strictEqual(enabled[id], OLD_GUARDS[id](f), `${id} for ${JSON.stringify(f)}`);
    assert.strictEqual(enabled.help, true);
  }
});

test('S10: Panel.qml opens the overflow menu from the header and performs the chosen action', () => {
  const panel = panelSource();
  const header = only(panel, 'PanelHeader');
  assert.match(header, /onOverflowRequested/);
  const menu = only(panel, 'OverflowMenu');
  assert.ok(menu, 'Panel.qml must instantiate OverflowMenu');
  assert.match(menu, /items:[^\n]*overflowItems/);
  assert.match(menu, /onItemChosen/);
});

// ---- S11: project switcher --------------------------------------------------------------------
test('S11: sessionMarker equals the old project tab marker formula', () => {
  const nav = loadModule('PanelNavigation.js');
  const phases = ['idle', 'planning', 'plan_ready', 'awaiting_approval', 'running', 'blocked', 'failed', 'done'];
  let checked = 0;
  for (const phase of phases) for (const busy of [false, true]) for (const queue_active of [false, true])
    for (const queued of [0, 1, 3]) {
      const session = { name: 'forge', project: '/p/forge', phase, busy, queue_active, queued };
      assert.equal(nav.call('sessionMarker', session), oldSessionMarker(session), JSON.stringify(session));
      checked++;
    }
  assert.equal(checked, 8 * 2 * 2 * 3);
  const busy = { phase: 'running', busy: true, queue_active: false, queued: 0 };
  const blocked = { phase: 'blocked', busy: false, queue_active: false, queued: 2 };
  assert.equal(nav.call('sessionMarker', busy), '●');
  assert.equal(nav.call('sessionMarker', blocked), '! +2');
});

test('S11: Panel.qml selects the chosen project, opens the chooser and leaves currentTab alone', () => {
  const panel = panelSource();
  const header = only(panel, 'PanelHeader');
  assert.ok(header, 'Panel.qml must instantiate PanelHeader');
  assert.match(header, /onProjectSelected/);
  assert.ok(panel.includes('/api/project/select'), 'project selection keeps its endpoint');
  assert.match(header, /onChangeProjectRequested[^\n]*openChooser\(\)/);
  assert.ok(!assignsCurrentTab(header), 'switching projects must not change the selected tab');
  assert.match(readSource('PanelHeader.qml'), /sessionMarker/, 'the switcher uses PanelNavigation.sessionMarker');
});

// ---- S16: Activity ----------------------------------------------------------------------------
test('S16: ActivityView hosts AgentOutputView at the full view height', () => {
  const activity = readSource('ActivityView.qml');
  const output = only(activity, 'AgentOutputView');
  assert.ok(output, 'ActivityView.qml must contain AgentOutputView');
  assert.match(output, /anchors\.fill:\s*parent|height:\s*[\w.]*height\s*(\n|$)/,
    'AgentOutputView must fill the Activity view');
  assert.match(output, /panelHeight:\s*[\w.]*[hH]eight\s*(\n|$)/,
    'panelHeight must be the whole view height, not a fraction of it');
});

test('S16: Tab, h/l, Ctrl+d/u and the digit filters are still handled and routed to Activity', () => {
  const panel = panelSource();
  const body = keyHandlerBody(panel);
  for (const key of ['Key_Tab', 'Key_H', 'Key_L', 'Key_1', 'Key_6', 'Key_D', 'Key_U'])
    assert.ok(body.includes(`Qt.${key}`), `keyHandler must still handle ${key}`);
  assert.match(body, /liveTab = !root\.liveTab|liveTab = !/);
  assert.match(body, /historyFilter = \["all", "runs", "git", "reviews", "errors", "reports"\]/);
  assert.match(body, /scrollOutput\(/);
  // The keys act on the view that shows their target: Activity.
  assert.match(panel.slice(panel.indexOf('id: keyHandler')), /activity/i,
    'these keys must be routed to the Activity view');
  assert.equal(instances(panel, 'ActivityView').length, 1, 'Panel.qml must instantiate ActivityView');
});

// ---- S17: Architecture ------------------------------------------------------------------------
test('S17: ArchitectureView hosts the architecture card, role token totals and plan review status', () => {
  const architecture = readSource('ArchitectureView.qml');
  assert.ok(instances(architecture, 'ArchitectureReviewView').length >= 1,
    'ArchitectureView.qml must contain ArchitectureReviewView');
  assert.match(architecture, /role_usage|roleUsage|tokens/i, 'role token totals belong to Architecture');
  assert.match(architecture, /planReview|plan_review/, 'plan review status and history belong to Architecture');
  assert.equal(instances(panelSource(), 'ArchitectureView').length, 1, 'Panel.qml must instantiate ArchitectureView');
});

test('S17: architecture, role usage and plan review status appear nowhere on Overview', () => {
  const overview = readSource('OverviewView.qml');
  assert.ok(!/architect|role[_ ]?usage|role token|plan[_ ]?review/i.test(overview),
    'OverviewView.qml must not reference architecture, role usage or plan review status');
  assert.equal(instances(panelSource(), 'ArchitectureReviewView').length, 0,
    'Panel.qml must not host the architecture card on the page any more');
});

// ---- S18: Queue and Features tabs -----------------------------------------------------------
test('S18: the Queue tab label shows the number of queued goals', () => {
  const nav = loadModule('PanelNavigation.js');
  assert.equal(nav.call('tabLabel', 'queue', 0), 'Queue');
  assert.equal(nav.call('tabLabel', 'queue', 1), 'Queue 1');
  assert.equal(nav.call('tabLabel', 'queue', 2), 'Queue 2');
  for (const id of TAB_IDS.filter(id => id !== 'queue'))
    assert.equal(nav.call('tabLabel', id, 2), nav.call('tabLabel', id, 0), `${id} label ignores the count`);
});

test('S18: Queue and Features are tabs, and f and g f both select Features', () => {
  const panel = panelSource();
  only(panel, 'QueueView');
  only(panel, 'FeaturesView');
  assert.match(panel, /queueCount:\s*(root\.)?queue\.length/);
  const nav = loadModule('PanelNavigation.js');
  assert.equal(nav.call('tabForGKey', 'f'), 'features');
  const opener = blockAfter(readSource('FeaturesController.qml'), 'function openFeatures()');
  assert.match(opener, /currentTab\s*=\s*["']features["']/, 'openFeatures() must select the Features tab');
  assert.match(keyHandlerBody(panel), /Qt\.Key_F\)[^}]{0,160}openFeatures\(\)/,
    'f keeps opening the feature list');
});

test('S18: Features keeps its actions and shortcuts', () => {
  const panel = panelSource();
  const body = keyHandlerBody(panel);
  assert.ok(body.includes('featuresView.handleKey(event)'), 'keyHandler must route Features keys to the view');
  const keys = handleKeySource('FeaturesView.qml');
  for (const call of ['openNewFeatureForm()', 'view.reviewRequested(row)', 'view.approveSpecRequested(row)',
    'view.approveScenariosRequested(row)', 'focusChatInput()', 'featuresList.activateSelection()'])
    assert.ok(keys.includes(call), `FeaturesView.handleKey must still call ${call}`);
  for (const key of ['Key_N', 'Key_V', 'Key_C', 'Key_A', 'Key_O'])
    assert.ok(keys.includes(`Qt.${key}`), key);
  const view = only(panel, 'FeaturesView');
  for (const call of ['features.requestFeatureReview(feature)', 'features.approveFeatureSpec(feature)',
    'features.approveFeatureScenarios(feature)', 'features.selectFeature(feature)'])
    assert.ok(view.includes(call), `the Features tab must still reach ${call}`);
});

test('S18: QueueView keeps Start queue and the ↑ ↓ × row actions', () => {
  const queue = readSource('QueueView.qml');
  for (const name of ['startQueue', 'queueRow', 'queueUp', 'queueDown', 'queueRemove'])
    assert.ok(queue.includes(`objectName: "${name}"`), `QueueView.qml must name ${name}`);
  const panel = panelSource();
  const view = only(panel, 'QueueView');
  for (const endpoint of ['/api/queue/start', '/api/queue/move', '/api/queue/remove'])
    assert.ok(panel.includes(endpoint), `Panel.qml keeps ${endpoint}`);
  assert.match(view, /onStartRequested/);
  assert.match(view, /onMoveRequested/);
  assert.match(view, /onRemoveRequested/);
});

// ---- S19: pushed pages return to their tab --------------------------------------------------
test('S19: closing the diff, chooser, discussion, catalogue or help never assigns currentTab', () => {
  const panel = panelSource();
  const closers = {
    onDiffOpenChanged: blockAfter(panel, 'onDiffOpenChanged: {'),
    onChooserOpenChanged: blockAfter(panel, 'onChooserOpenChanged: {'),
    onHelpOpenChanged: blockAfter(panel, 'onHelpOpenChanged: {'),
    closeDiscussion: blockAfter(panel, 'function closeDiscussion()'),
  };
  if (panel.includes('onCatalogueOpenChanged'))
    closers.onCatalogueOpenChanged = blockAfter(panel, 'onCatalogueOpenChanged: {');
  for (const [name, text] of Object.entries(closers))
    assert.ok(!assignsCurrentTab(text), `${name} must leave the selected tab alone`);
});

test('S19: the key handler branches that close pushed pages never assign currentTab', () => {
  const body = keyHandlerBody(panelSource());
  const start = body.indexOf('if (catalogueController.catalogueOpen)');
  assert.ok(start >= 0, 'the catalogue branch must exist');
  const ends = ['else if (features.featuresOpen)', 'else if (root.discussionOpen)', 'else if (question)']
    .map(marker => body.indexOf(marker, start)).filter(i => i > start);
  assert.ok(ends.length > 0, 'the modal branches must be followed by the discussion or help branch');
  const modal = body.slice(start, Math.min(...ends));
  for (const close of ['root.helpOpen = false', 'catalogueController.catalogueOpen = false'])
    assert.ok(modal.includes(close), `the key handler must still close pages with ${close}`);
  // The diff and the chooser close through their views' handleKey(), whose closeRequested()
  // Panel.qml wires to the same flag assignments.
  for (const call of ['diffView.handleKey(', 'projectChooser.handleKey('])
    assert.ok(modal.includes(call), `the key handler must still route keys with ${call}`);
  for (const file of ['DiffView.qml', 'ProjectChooser.qml'])
    assert.ok(handleKeySource(file).includes('closeRequested()'), `${file} must close on q / Escape`);
  const panel = panelSource();
  assert.match(only(panel, 'DiffView'), /onCloseRequested:\s*root\.diffOpen = false/);
  assert.match(only(panel, 'ProjectChooser'), /onCloseRequested:\s*root\.chooserOpen = false/);
  assert.ok(!assignsCurrentTab(modal), 'closing a pushed page must not change the tab');
  const at = body.indexOf('else if (root.discussionOpen)');
  assert.ok(at >= 0, 'the discussion branch must exist');
  assert.ok(!assignsCurrentTab(blockFrom(body, at)), 'closing the discussion must not change the tab');
});

// ---- S20: state survives polling and reopening ----------------------------------------------
test('S20: tab views are persistent instances, never Loaders', () => {
  const panel = panelSource();
  assert.ok(!/\bLoader\s*\{/.test(panel), 'Panel.qml must not use Loader for tab views');
  for (const file of Object.values(VIEW_FILES).filter(name => name !== 'FeaturesView')) {
    assert.ok(!/\bLoader\s*\{/.test(readSource(`${file}.qml`)), `${file}.qml must not use Loader`);
    assert.equal(instances(panel, file).length, 1, `${file} must exist exactly once`);
  }
});

test('S20: polling, project changes, open() and close() never assign currentTab', () => {
  const panel = panelSource();
  assert.match(panel, /property string currentTab:\s*"overview"/);
  const open = blockAfter(panel, 'function open(payloadJson)');
  const close = blockAfter(panel, 'function close()');
  const stateChanged = blockAfter(panel, 'onEngineStateChanged: {');
  const refresh = blockAfter(panel, 'function refresh(');
  assert.ok(!assignsCurrentTab(open), 'reopening must restore the last tab');
  assert.ok(!assignsCurrentTab(close), 'closing must keep the tab');
  assert.ok(!assignsCurrentTab(stateChanged), 'switching project must keep the selected tab');
  assert.ok(!assignsCurrentTab(refresh), 'polling must not change the tab');
});

// ---- S21: every existing shortcut still works -----------------------------------------------
test('S21: helpRows lists every old shortcut and the new view keys', () => {
  const nav = loadModule('PanelNavigation.js');
  const rows = nav.call('helpRows');
  assert.ok(Array.isArray(rows) && rows.every(r => typeof r.key === 'string' && typeof r.description === 'string'));
  const have = new Set(rows.map(r => `${r.key} ${r.description}`));
  for (const row of OLD_HELP_ROWS)
    assert.ok(have.has(`${row.key} ${row.description}`), `help must keep ${JSON.stringify(row)}`);
  const keys = rows.map(r => r.key);
  assert.ok(keys.includes('[ / ]'), 'help must list [ / ]');
  assert.ok(keys.includes('g o / p / a / r / f / q / s'), 'help must list the g view keys');
  for (const key of ['[ / ]', 'g o / p / a / r / f / q / s'])
    assert.ok(rows.find(r => r.key === key).description.trim() !== '', `${key} needs a description`);
});

test('S21: the keyboard help page renders PanelNavigation.helpRows()', () => {
  assert.match(panelSource(), /model:\s*PanelNavigation\.helpRows\(\)/);
});

test('S21: the bottom hint line follows the insert mode and names the current view keys', () => {
  const nav = loadModule('PanelNavigation.js');
  for (const tab of TAB_IDS) {
    assert.equal(nav.call('hintText', tab, true), 'INSERT - Esc to normal mode', tab);
    const hint = nav.call('hintText', tab, false);
    assert.match(hint, /^NORMAL/, tab);
    assert.ok(hint.includes('[ ]'), `${tab}: hint must mention [ ]`);
    assert.match(hint, /\bg\b/, `${tab}: hint must mention g view switching`);
    assert.ok(hint.includes('?'), `${tab}: hint must mention ? help`);
  }
  const hints = TAB_IDS.map(tab => nav.call('hintText', tab, false));
  assert.ok(new Set(hints).size >= 4, 'the hint must differ between views');
  assert.match(nav.call('hintText', 'activity', false), /Tab/);
  assert.match(nav.call('hintText', 'activity', false), /h\/l/);
  assert.match(nav.call('hintText', 'plan', false), /j\/k/);
  assert.match(nav.call('hintText', 'features', false), /j\/k/);
  assert.match(panelSource(), /PanelNavigation\.hintText\(/, 'Panel.qml must render the hint from PanelNavigation');
});

test('S21: keyHandler still handles every key it handled before the redesign', () => {
  const panel = panelSource();
  const body = keyHandlerBody(panel) + '\n' + viewKeySource();
  const constants = ['Question', 'Slash', 'F1', 'Escape', 'I', 'E', 'T', 'G', 'P', 'D', 'U',
    'PageDown', 'PageUp', 'Home', 'End', 'A', 'R', 'X', 'C', 'F', 'Tab', 'H', 'L',
    '1', '6', 'J', 'K', 'Return', 'Enter', 'O', 'Space', 'Q', 'N', 'V'];
  for (const key of constants) assert.ok(body.includes(`Qt.Key_${key}`), `keyHandler must handle Qt.Key_${key}`);
  const flat = body.replace(/\s+/g, ' ');
  for (const key of ['I', 'E', 'G', 'P'])
    assert.match(flat, new RegExp(`Qt\\.Key_${key}\\b[^;{}]{0,80}Qt\\.ShiftModifier`), `Shift+${key}`);
  for (const key of ['D', 'U'])
    assert.match(flat, new RegExp(`Qt\\.ControlModifier[^;{}]{0,80}Qt\\.Key_${key}\\b`), `Ctrl+${key}`);
  for (const call of ['root.enhanceGoal()', 'root.openDiscussion()', 'root.planFromDiscussion()',
    'root.beginPlanEdit()', 'root.openDiff()', 'root.openChooser()', 'features.openFeatures()',
    'root.cancelPlanEdit()', 'scrollOutput(', 'selectStage(', 'selectReport('])
    assert.ok(body.includes(call), `keyHandler must still reach ${call}`);
  // Diff, chooser, features, help, catalogue and discussion branches keep their own keys.
  for (const branch of ['root.diffOpen', 'root.chooserOpen', 'features.featuresOpen', 'root.helpOpen',
    'catalogueController.catalogueOpen', 'root.discussionOpen'])
    assert.ok(body.includes(branch), `keyHandler must keep the ${branch} branch`);
  assert.match(flat, /refreshRequested\(\)/);
  assert.match(panel, /onRefreshRequested:\s*root\.refreshDiff\(\)/);
  assert.match(flat, /chooserList\.activateSelection\(\)/);
});

// ---- S22: views live in their own files --------------------------------------------------------
test('S22: each tab view is a separate QML file', () => {
  for (const file of Object.values(VIEW_FILES))
    assert.ok(existsSync(new URL(`${file}.qml`, quickshell)), `quickshell/${file}.qml must exist`);
  for (const file of ['PanelHeader.qml', 'PanelTabBar.qml', 'OverflowMenu.qml', 'PanelNavigation.js',
    'PanelActions.js'])
    assert.ok(existsSync(new URL(file, quickshell)), `quickshell/${file} must exist`);
});

test('S22: Panel.qml is under 1,800 lines and instantiates each view exactly once', () => {
  const panel = panelSource();
  assert.ok(panel.split('\n').length < 1800, `Panel.qml has ${panel.split('\n').length} lines`);
  for (const type of [...Object.values(VIEW_FILES), 'PanelHeader', 'PanelTabBar', 'OverflowMenu'])
    assert.equal(instances(panel, type).length, 1, `${type} must be instantiated exactly once`);
  for (const fn of ['function open(', 'function close(', 'function api(', 'function act(', 'function refresh('])
    assert.ok(panel.includes(fn), `Panel.qml keeps ${fn}...)`);
  assert.ok(panel.includes('onEngineStateChanged'), 'Panel.qml keeps state and polling');
});
