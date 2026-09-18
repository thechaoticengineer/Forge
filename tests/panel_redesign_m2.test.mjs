// Executable scenarios for panel-redesign milestone M2 (compact plan and stage detail).
// Covers S7 (source side), S12 (pure-JS and source side), S13, S14, S15 and S23 of
// docs/features/panel-redesign/scenarios.md, plus the Settings quota polish (prefixed "polish:").
// The rendered behaviour of the qs-free components is covered by tests/qml/tst_panel_plan.qml (S12),
// tst_panel_stage_detail.qml (S13, S14), tst_panel_overview_attention.qml (S7) and
// tst_panel_settings_compact.qml (polish); this suite owns the pure-JS contract modules
// (PanelNavigation.js, StagePresentation.js, PlanReview.js, UsageFormat.js) and the source-level
// contracts of Panel.qml, PlanView.qml, PlanEditorView.qml and StageDetailPage.qml.
//
// OLD_HELP_ROWS is a frozen copy of PanelNavigation.helpRows() as shipped by M1 (head 22c9be2).
// It is the behavioural oracle for decision D3 (move, never remove) and must not be edited to fit
// new code. The helper functions below are copied from tests/panel_redesign_m1.test.mjs because
// the suites are independent files.
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import vm from 'node:vm';

const quickshell = new URL('../quickshell/', import.meta.url);
const plain = value => (value === undefined ? undefined : JSON.parse(JSON.stringify(value)));

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

// The normal-mode key handler of the root item.
const keyHandlerBody = source => {
  const at = source.indexOf('id: keyHandler');
  assert.ok(at >= 0, 'Panel.qml must keep the keyHandler item');
  return blockAfter(source.slice(at), 'Keys.onPressed: event => {', 'keyHandler Keys.onPressed');
};

const stageDetailSource = () => readSource('StageDetailPage.qml');
const flat = text => text.replace(/\s+/g, ' ');

// ---- frozen oracle: PanelNavigation.helpRows() as shipped by M1 ------------------------
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
  { key: "", description: "Views" },
  { key: "[ / ]", description: "Switch to the previous / next view tab" },
  { key: "g o / p / a / r / f / q / s", description: "Go to Overview / Plan / Activity / Architecture / Features / Queue / Settings; gg is unchanged" },
  { key: "PgDn / PgUp / Home / End", description: "Scroll the current view (Activity scrolls with Ctrl+d / Ctrl+u)" },
  { key: "Tab / h / l / 1-6", description: "Act on Activity, the view that shows the output: Live / History and the History filters" },
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

const STAGE_DETAIL_TABS = ['instructions', 'acceptance', 'review', 'routing', 'output'];

// ---- S14: navigation helpers -------------------------------------------------------------
test('S14: stageDetailTabs lists Instructions, Acceptance, Review, Routing and Output in order', () => {
  const nav = loadModule('PanelNavigation.js');
  assert.ok(nav.get('stageDetailTabs'), 'PanelNavigation.js must export stageDetailTabs');
  assert.deepEqual(nav.get('stageDetailTabs'), STAGE_DETAIL_TABS);
});

test('S14: stepStageIndex moves to the previous / next stage, clamps at both ends and never wraps', () => {
  const nav = loadModule('PanelNavigation.js');
  assert.equal(nav.call('stepStageIndex', 5, 2, 1), 3, ']');
  assert.equal(nav.call('stepStageIndex', 5, 2, -1), 1, '[');
  assert.equal(nav.call('stepStageIndex', 5, 4, 1), 4, '] on the last stage stays on it');
  assert.equal(nav.call('stepStageIndex', 5, 0, -1), 0, '[ on the first stage stays on it');
  assert.equal(nav.call('stepStageIndex', 1, 0, 1), 0);
  assert.equal(nav.call('stepStageIndex', 1, 0, -1), 0);
  assert.equal(nav.call('stepStageIndex', 5, 9, 1), 4, 'an index past the end clamps');
  assert.equal(nav.call('stepStageIndex', 5, -3, -1), 0, 'an index before the start clamps');
  assert.equal(nav.call('stepStageIndex', 0, 0, 1), -1, 'no stages, no index');
  assert.equal(nav.call('stepStageIndex', 0, 0, -1), -1, 'no stages, no index');
});

test('S14: stepDetailTab moves to the previous / next sub-tab and wraps around', () => {
  const nav = loadModule('PanelNavigation.js');
  STAGE_DETAIL_TABS.forEach((id, i) => {
    assert.equal(nav.call('stepDetailTab', id, 1), STAGE_DETAIL_TABS[(i + 1) % 5], `l from ${id}`);
    assert.equal(nav.call('stepDetailTab', id, -1), STAGE_DETAIL_TABS[(i + 4) % 5], `h from ${id}`);
  });
  assert.equal(nav.call('stepDetailTab', 'output', 1), 'instructions');
  assert.equal(nav.call('stepDetailTab', 'instructions', -1), 'output');
});

test('S14: the stage detail hint names the sub-tab, stage and back keys', () => {
  const nav = loadModule('PanelNavigation.js');
  const hint = nav.call('hintText', 'stageDetail', false);
  assert.match(hint, /^NORMAL/);
  for (const part of ['h/l', '[ ]', 'q/Esc', '?'])
    assert.ok(hint.includes(part), `the stage detail hint must mention ${part}: ${hint}`);
  assert.equal(nav.call('hintText', 'stageDetail', true), 'INSERT - Esc to normal mode');
  assert.notEqual(hint, nav.call('hintText', 'plan', false), 'stage detail has its own hint');
});

test('S13: the Plan hint names the stage keys j/k and Enter', () => {
  const nav = loadModule('PanelNavigation.js');
  const hint = nav.call('hintText', 'plan', false);
  assert.ok(hint.includes('j/k'), hint);
  assert.ok(hint.includes('Enter'), hint);
});

test('S14: helpRows keeps every M1 row and adds a Stage detail section with [ / ], h / l and q / Escape', () => {
  const nav = loadModule('PanelNavigation.js');
  const rows = nav.call('helpRows');
  assert.ok(Array.isArray(rows) && rows.every(r => typeof r.key === 'string' && typeof r.description === 'string'));
  const have = new Set(rows.map(r => `${r.key} ${r.description}`));
  for (const row of OLD_HELP_ROWS)
    assert.ok(have.has(`${row.key} ${row.description}`), `help must keep ${JSON.stringify(row)}`);
  // The M1 rows keep their relative order.
  let at = -1;
  for (const row of OLD_HELP_ROWS) {
    const next = rows.findIndex((r, i) => i > at && r.key === row.key && r.description === row.description);
    assert.ok(next > at, `help must keep ${JSON.stringify(row)} after the row before it`);
    at = next;
  }
  const section = rows.findIndex(r => r.key === '' && r.description === 'Stage detail');
  assert.ok(section >= 0, 'help must have a "Stage detail" section row');
  const rest = rows.slice(section + 1);
  const end = rest.findIndex(r => r.key === '');
  const inSection = end < 0 ? rest : rest.slice(0, end);
  const find = pattern => inSection.find(r => pattern.test(r.key));
  const brackets = find(/^\[ \/ \]$/);
  assert.ok(brackets, 'the Stage detail section must list [ / ]');
  assert.match(brackets.description, /previous \/ next stage/i);
  const tabs = find(/^h \/ l$/);
  assert.ok(tabs, 'the Stage detail section must list h / l');
  assert.match(tabs.description, /sub-?tabs?/i);
  const back = find(/\bq\b.*\bEsc/);
  assert.ok(back, 'the Stage detail section must list q / Escape');
  assert.match(back.description, /Plan/);
});

// ---- S13: stage status text ----------------------------------------------------------------
// The status string PlanEditorView.qml builds today, verbatim (the oracle for statusText).
function oldStatusText(stage, editingPlan) {
  const gate = stage.review_gate && stage.review_gate.status;
  return (editingPlan && stage.status === 'committed'
    ? 'committed — locked' : stage.status === 'blocked'
      ? (gate === 'scope_blocked' ? 'blocked · stage cannot be built as written'
        : gate === 'design_blocked' ? 'blocked · pen.dev export unavailable'
          : gate === 'exhausted' ? 'blocked · fix rounds exhausted' : 'blocked') : stage.status)
    + (stage.sha ? ' ' + stage.sha : '');
}

test('S13: statusText returns exactly the status string the inline stage row built', () => {
  const presentation = loadModule('StagePresentation.js');
  const stages = [];
  for (const status of ['pending', 'in_progress', 'committed', 'blocked', 'failed'])
    for (const gate of [undefined, 'pending', 'approved', 'scope_blocked', 'design_blocked', 'exhausted', 'blocked'])
      for (const sha of [undefined, 'a1b2c3d'])
        stages.push({ id: 3, title: 'Stage', status, sha, review_gate: gate ? { status: gate } : undefined });
  stages.push({ id: 1, title: 'No gate', status: 'blocked' });
  for (const stage of stages)
    for (const editing of [false, true])
      assert.equal(presentation.call('statusText', stage, editing), oldStatusText(stage, editing),
        JSON.stringify({ stage, editing }));
  assert.equal(presentation.call('statusText', { status: 'committed', sha: 'a1b2c3d' }, true),
    'committed — locked a1b2c3d');
  assert.equal(presentation.call('statusText', { status: 'committed', sha: 'a1b2c3d' }, false), 'committed a1b2c3d');
  assert.equal(presentation.call('statusText',
    { status: 'blocked', review_gate: { status: 'scope_blocked' } }, false),
  'blocked · stage cannot be built as written');
  assert.equal(presentation.call('statusText',
    { status: 'blocked', review_gate: { status: 'design_blocked' } }, false),
  'blocked · pen.dev export unavailable');
  assert.equal(presentation.call('statusText',
    { status: 'blocked', review_gate: { status: 'exhausted' }, sha: 'deadbee' }, false),
  'blocked · fix rounds exhausted deadbee');
  assert.equal(presentation.call('statusText', { status: 'in_progress' }, false), 'in_progress');
});

test('S7: attentionReason explains blocked and failed stages and is empty for every other stage', () => {
  const presentation = loadModule('StagePresentation.js');
  for (const gate of [undefined, 'scope_blocked', 'design_blocked', 'exhausted', 'blocked']) {
    const stage = { id: 2, title: 'Blocked stage', status: 'blocked', review_gate: gate ? { status: gate } : undefined };
    const reason = presentation.call('attentionReason', stage);
    assert.equal(typeof reason, 'string');
    assert.notEqual(reason.trim(), '', `a blocked stage (gate ${gate}) needs a reason`);
    assert.ok(!reason.includes('\n'), 'the reason is one line');
  }
  const failed = presentation.call('attentionReason', { id: 4, title: 'Failed stage', status: 'failed' });
  assert.notEqual(String(failed).trim(), '', 'a failed stage needs a reason');
  for (const status of ['pending', 'in_progress', 'committed'])
    assert.equal(presentation.call('attentionReason', { id: 1, title: 'Fine', status }), '', status);
});

test('S13: StagePresentation.js is a pure .pragma library module', () => {
  const source = readSource('StagePresentation.js');
  assert.match(source, /^\s*\.pragma\s+library\s*$/m, 'StagePresentation.js must be a .pragma library module');
  assert.ok(!/^\s*\.import\s+"?qs/m.test(source), 'no shell imports');
});

// ---- S12: plan review strip -------------------------------------------------------------
// The gate roles the engine emits (src/review_verdict.rs, src/review_orchestration.rs): the keys
// are "architect" and "reviewer"; the values are approved, changes_requested, not_required,
// deferred and pending. The UI relabels "reviewer" as "Independent" (decision M2-REVIEW-01).
const planReview = (overrides = {}) => ({
  status: 'approved', rounds: 2, budget: 3, attempt_id: 'attempt-1',
  gate: { status: 'approved', roles: { architect: 'approved', reviewer: 'approved' } },
  ...overrides,
});

test('S12: planReviewStripText is empty without a plan review', () => {
  const review = loadModule('PlanReview.js');
  assert.equal(review.call('planReviewStripText', null), '');
});

test('S12: planReviewStripText summarises verdict, round and both roles on one line', () => {
  const review = loadModule('PlanReview.js');
  const line = review.call('planReviewStripText', planReview());
  assert.equal(typeof line, 'string');
  assert.ok(!line.includes('\n'), `the strip is one line: ${JSON.stringify(line)}`);
  assert.ok(line.includes('approved'), line);
  assert.ok(line.includes('round 2 of 4'), line);
  assert.match(line, /architect/i, line);
  assert.match(line, /independent/i, line);
  assert.doesNotMatch(line, /reviewer/i, 'the gate role "reviewer" is shown as Independent');
});

test('S12: planReviewStripText follows the verdict, the round and the role outcomes', () => {
  const review = loadModule('PlanReview.js');
  const changes = review.call('planReviewStripText', planReview({
    status: 'changes_requested', rounds: 1, budget: 3,
    gate: { status: 'blocked', roles: { architect: 'changes_requested', reviewer: 'approved' } },
  }));
  assert.ok(!changes.includes('\n'), changes);
  assert.ok(changes.includes('changes_requested') || /changes requested/i.test(changes), changes);
  assert.ok(changes.includes('round 1 of 4'), changes);
  const notRequired = review.call('planReviewStripText', planReview({
    gate: { status: 'approved', roles: { architect: 'not_required', reviewer: 'deferred' } },
  }));
  assert.ok(!notRequired.includes('\n'), notRequired);
  assert.match(notRequired, /architect/i);
  assert.match(notRequired, /independent/i);
  assert.ok(/not required|not_required/i.test(notRequired), `a not_required role is shown: ${notRequired}`);
  assert.ok(/deferred/i.test(notRequired), `a deferred role is shown: ${notRequired}`);
  const unknown = review.call('planReviewStripText', { status: 'pending', gate: {} });
  assert.ok(!unknown.includes('\n'), unknown);
  assert.notEqual(unknown, '', 'an incomplete review still gets a strip');
});

test('S12: planReviewStatusText keeps its Architecture-tab wording', () => {
  const review = loadModule('PlanReview.js');
  const text = review.call('planReviewStatusText', planReview());
  assert.ok(text.includes('Plan review: approved · round 2 of 4'), text);
  assert.ok(text.includes('Architect: approved · Independent: approved'), text);
});

// ---- polish: compact quota lines --------------------------------------------------------
const WEEKDAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const pad = n => String(n).padStart(2, '0');
// The reset text the compact line must carry, computed with the same local Date methods.
const resetText = unix => {
  const d = new Date(unix * 1000);
  return `${WEEKDAYS[d.getDay()]} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
};
const nowUnix = () => Math.floor(Date.now() / 1000);

function quota(extra = {}) {
  const soon = nowUnix() + 3600;
  return {
    status: 'available', error: null, refreshing: false, extra_usage_enabled: false,
    windows: [
      { name: 'Claude · 5h', model: null, used_percent: 14.0, resets_unix: soon,
        resets_at: new Date(soon * 1000).toISOString() },
      { name: 'Claude · weekly', model: null, used_percent: 33.4, resets_unix: soon + 86400,
        resets_at: new Date((soon + 86400) * 1000).toISOString() },
    ],
    ...extra,
  };
}

test('polish: quotaCompactLines gives one single-line string per window with remaining percentage and reset', () => {
  const usage = loadModule('UsageFormat.js');
  const q = quota();
  const lines = usage.call('quotaCompactLines', q);
  assert.ok(Array.isArray(lines));
  assert.equal(lines.length, 2, 'one line per window');
  assert.equal(lines[0], `Claude · 5h 86% remaining · resets ${resetText(q.windows[0].resets_unix)}`);
  assert.equal(lines[1], `Claude · weekly 67% remaining · resets ${resetText(q.windows[1].resets_unix)}`);
  for (const line of lines) assert.ok(!line.includes('\n'), JSON.stringify(line));
});

test('polish: quotaCompactLines shows awaiting refresh for expired windows and never a negative percentage', () => {
  const usage = loadModule('UsageFormat.js');
  const past = nowUnix() - 600;
  const q = quota();
  q.windows[0] = { name: 'Claude · 5h', used_percent: 100, resets_unix: past,
    resets_at: new Date(past * 1000).toISOString() };
  q.windows[1].used_percent = 120;
  const lines = usage.call('quotaCompactLines', q);
  assert.equal(lines.length, 2);
  assert.ok(lines[0].startsWith('Claude · 5h'), lines[0]);
  assert.ok(lines[0].includes('awaiting refresh'), lines[0]);
  assert.ok(!lines[0].includes('% remaining'), lines[0]);
  assert.ok(lines[1].includes('0% remaining'), lines[1]);
  assert.ok(!lines[1].includes('-'), lines[1]);
});

test('polish: quotaCompactLines marks a stale reading and keeps every line single', () => {
  const usage = loadModule('UsageFormat.js');
  const q = quota({ status: 'stale', error: 'refresh failed:\nnetwork down' });
  const lines = usage.call('quotaCompactLines', q);
  assert.ok(lines.length >= 2);
  assert.ok(lines.some(line => line.endsWith(' · previous reading')), JSON.stringify(lines));
  for (const line of lines) assert.ok(!line.includes('\n'), JSON.stringify(line));
});

test('polish: quotaCompactLines gives one line for pending or unavailable quota, equal to the first summary line', () => {
  const usage = loadModule('UsageFormat.js');
  for (const q of [null, { status: 'pending' }, { status: 'unavailable', error: 'no credentials\nfound' },
    { status: 'unavailable' }]) {
    const lines = usage.call('quotaCompactLines', q);
    assert.deepEqual(lines, [usage.call('quotaSummary', q).split('\n')[0]], JSON.stringify(q));
    assert.ok(!lines[0].includes('\n'));
  }
});

// ---- S13, S14, S23: Panel and StageDetailPage source contracts ------------------------------------
test('S23: the stage detail page is its own qs-free file, so qmltestrunner can load it', () => {
  const page = stageDetailSource();
  assert.ok(!/^\s*import\s+(Quickshell|qs\.)/m.test(page),
    'StageDetailPage.qml must stay qs-free so qmltestrunner can load it');
  assert.match(page, /function handleKey\(/, 'the page owns its keys');
});

test('S23: Panel.qml is under 1,800 lines and instantiates StageDetailPage once, as a persistent page outside any Loader', () => {
  const panel = panelSource();
  assert.ok(panel.split('\n').length < 1800, `Panel.qml has ${panel.split('\n').length} lines`);
  const page = only(panel, 'StageDetailPage');
  assert.match(page, /\bid:\s*stageDetailPage\b/);
  assert.match(page, /\bvisible:\s*false\b/, 'the pushed page starts hidden, like DiscussionView');
  assert.ok(!/\bLoader\s*\{/.test(panel), 'stage detail must not be built by a Loader');
  for (const loader of instances(panel, 'Loader'))
    assert.ok(!/\bStageDetailPage\s*\{/.test(loader), 'StageDetailPage must not sit inside a Loader');
  const discussion = only(panel, 'DiscussionView');
  assert.match(discussion, /\bvisible:\s*false\b/);
});

test('S13: Panel.qml pushes stage detail onto panelStack when a stage is opened', () => {
  const panel = panelSource();
  assert.match(panel,
    /readonly property bool stageDetailOpen:\s*panelStack\.currentItem\s*===\s*stageDetailPage/);
  const open = blockAfter(panel, 'function openStageDetail(');
  assert.match(open, /panelStack\.push\(\s*stageDetailPage\s*,\s*StackView\.Immediate\s*\)/,
    'openStageDetail pushes the page with StackView.Immediate');
  assert.ok(panel.includes('function closeStageDetail('), 'Panel.qml must define closeStageDetail()');
});

test('S14: closeStageDetail pops to panelPage and leaves the tab and the Plan scroll position alone', () => {
  const panel = panelSource();
  const close = blockAfter(panel, 'function closeStageDetail(');
  assert.match(close, /panelStack\.pop\(\s*panelPage\s*,\s*StackView\.Immediate\s*\)/);
  assert.ok(!assignsCurrentTab(close), 'closing stage detail must not assign currentTab');
  assert.ok(!/planView\.contentY\s*=[^=]/.test(close), 'closing stage detail must not reset the Plan scroll position');
  assert.ok(!/\.contentY\s*=[^=]/.test(close), 'closing stage detail must not touch any scroll position');
});

test('S14: the keyHandler routes keys to the stage detail page before it treats [ and ] as tab keys', () => {
  const body = keyHandlerBody(panelSource());
  const branch = body.indexOf('root.stageDetailOpen');
  assert.ok(branch >= 0, 'keyHandler must have a root.stageDetailOpen branch');
  const tabBranch = body.indexOf('viewTab !== ""');
  assert.ok(tabBranch >= 0, 'the viewTab branch still exists');
  assert.ok(branch < tabBranch, 'the stage detail branch must come before the viewTab !== "" branch');
  const stageBranch = blockFrom(body, branch);
  assert.ok(stageBranch.includes('stageDetailPage.handleKey('),
    'the stage detail branch must call stageDetailPage.handleKey(');
  assert.ok(!assignsCurrentTab(stageBranch), 'stage detail keys must not switch tabs');
});

test('S14: StageDetailPage.handleKey handles [ ] h l Escape and q', () => {
  const keys = blockAfter(stageDetailSource(), 'function handleKey(');
  for (const key of ['Key_BracketLeft', 'Key_BracketRight', 'Key_H', 'Key_L', 'Key_Escape', 'Key_Q'])
    assert.ok(keys.includes(`Qt.${key}`), `handleKey must handle Qt.${key}`);
  for (const name of ['backRequested', 'stageStepRequested', 'subTabRequested'])
    assert.ok(keys.includes(name), `handleKey must emit ${name}`);
});

test('S14: the hint line switches to the stage detail keys while stage detail is open', () => {
  const panel = panelSource();
  const at = panel.indexOf('PanelNavigation.hintText(');
  assert.ok(at >= 0, 'Panel.qml renders the hint from PanelNavigation');
  const call = panel.slice(at, at + 240);
  assert.ok(call.includes('stageDetailOpen'), `the hint must depend on stageDetailOpen: ${call}`);
  assert.match(call, /["']stageDetail["']/, `the hint must use the stageDetail view: ${call}`);
});

test('S14: Panel.qml wires the page signals to stage navigation and back', () => {
  const page = only(panelSource(), 'StageDetailPage');
  for (const handler of ['onBackRequested', 'onStageStepRequested', 'onSubTabRequested',
    'onLoadStageReviews', 'onRetryStageReviews', 'onStageRoutingExpandedRequested'])
    assert.ok(page.includes(handler), `Panel.qml must handle ${handler} of the stage detail page`);
  assert.match(page, /onBackRequested:[^\n]*root\.closeStageDetail\(\)/);
});

test('S13: stage detail keeps everything the inline expansion showed', () => {
  const page = stageDetailSource();
  for (const label of ['Instructions', 'Acceptance criteria'])
    assert.ok(page.includes(`label: "${label}"`), `StageDetailPage.qml must show ${label}`);
  for (const name of ['stageRoutingToggle', 'stageHistoricalReviews', 'stageReviewStatus',
    'stageReviewsOlder', 'stageReviewsRetry'])
    assert.ok(page.includes(`objectName: "${name}"`), `StageDetailPage.qml must name ${name}`);
  for (const name of ['stageBreadcrumb', 'stagePrevious', 'stageNext', 'stageStatusLine'])
    assert.ok(page.includes(`objectName: "${name}"`), `StageDetailPage.qml must name ${name}`);
  assert.ok(page.includes('"stageSubTab_"') || page.includes("'stageSubTab_'") || /objectName:\s*"stageSubTab_/.test(page),
    'StageDetailPage.qml must name its sub-tabs stageSubTab_<id>');
  for (const call of ['UsageFormat.usageSummary', 'ModelRouting.stageModelStatus', 'stageModelErrors'])
    assert.ok(page.includes(call), `StageDetailPage.qml must still show ${call}`);
});

test('S13: the inline expansion is gone from PlanEditorView', () => {
  const editor = readSource('PlanEditorView.qml');
  assert.ok(!editor.includes('label: "Acceptance criteria"'), 'the Acceptance criteria prose field moved to stage detail');
  const prose = instances(editor, 'StageProseField');
  assert.ok(!prose.some(block => /label:\s*"Instructions"/.test(block)),
    'the Instructions prose field moved to stage detail');
  assert.ok(!prose.some(block => /label:\s*"Review policy rationale"/.test(block)),
    'the review policy prose field moved to stage detail');
  for (const name of ['stageRoutingToggle', 'stageHistoricalReviews', 'stageReviewStatus',
    'stageReviewsOlder', 'stageReviewsRetry'])
    assert.ok(!editor.includes(`objectName: "${name}"`), `${name} moved to StageDetailPage.qml`);
  // Editing keeps its own fields.
  assert.ok(editor.includes('component PlanEditField'), 'plan editing stays in PlanEditorView');
  assert.ok(editor.includes('label: "Instructions"'), 'the plan editor still edits the stage instructions');
});

// ---- S12, S15: Plan view source contracts -------------------------------------------------------------
test('S12: PlanView lists stages with PlanStageList while not editing and PlanEditorView while editing', () => {
  const plan = readSource('PlanView.qml');
  const list = only(plan, 'PlanStageList');
  assert.match(list, /visible:\s*!\s*(view\.)?editingPlan\b/, 'PlanStageList is shown only while not editing');
  const editor = only(plan, 'PlanEditorView');
  assert.match(editor, /visible:\s*(view\.)?editingPlan\b/, 'PlanEditorView is shown while editing');
  assert.ok(!/visible:\s*!/.test(editor), 'PlanEditorView is not the read-only list any more');
  const stageList = readSource('PlanStageList.qml');
  assert.ok(!/^\s*import\s+(Quickshell|qs\.)/m.test(stageList), 'PlanStageList.qml must stay qs-free');
  for (const name of ['planStageRow', 'planReviewStrip'])
    assert.ok(stageList.includes(`objectName: "${name}"`), `PlanStageList.qml must name ${name}`);
  assert.ok(stageList.includes('OverviewStageRow'), 'the rows reuse OverviewStageRow');
});

test('S15: Plan keeps the feedback field, Plan Q&A and their actions', () => {
  const plan = readSource('PlanView.qml');
  for (const name of ['feedbackField', 'questionField', 'askPlanButton', 'chatSection', 'chatToggle'])
    assert.ok(plan.includes(name), `PlanView.qml must still contain ${name}`);
  const view = only(panelSource(), 'PlanView');
  const handlers = flat(view);
  assert.match(handlers, /id === "improve"\) root\.revisePlan\(\)/);
  assert.match(handlers, /id === "ask"\) root\.askPlanQuestion\(\)/);
  assert.match(handlers, /id === "editPlan"\) root\.beginPlanEdit\(\)/);
});

test('S15: the e key still starts plan editing', () => {
  const body = keyHandlerBody(panelSource());
  assert.match(flat(body), /Qt\.Key_E\) \{ if \(root\.guards\.editPlan\) root\.beginPlanEdit\(\)/);
  assert.ok(body.includes('root.cancelPlanEdit()'), 'Escape still cancels plan editing');
});

// ---- S7: Overview opens stage detail --------------------------------------------------------------------
test('S7: Overview stage rows and the needs-attention stage open the stage detail page', () => {
  const overview = only(panelSource(), 'OverviewView');
  const at = overview.indexOf('onStageRequested');
  assert.ok(at >= 0, 'Panel.qml must handle onStageRequested of OverviewView');
  assert.ok(overview.slice(at, at + 300).includes('root.openStageDetail('),
    'onStageRequested must call root.openStageDetail(');
  const view = readSource('OverviewView.qml');
  assert.ok(view.includes('objectName: "overviewAttention"'));
  assert.ok(view.includes('stageRequested('), 'OverviewView emits stageRequested(index)');
});
