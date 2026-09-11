import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const qml = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
const slice = (start, end) => qml.slice(qml.indexOf(start), qml.indexOf(end));
const helpers = slice('  function goalEnhancementAction(', '  function revisePlan(');
const stateHandler = slice('  onEngineStateChanged: {', '  onBusyChanged: {')
  .replace('onEngineStateChanged: {', 'function engineStateChanged() {');
const enabled = qml.match(/id: enhanceGoalButton[\s\S]*?enabled: ([\s\S]*?)\n\s*onClicked:/)[1];
const defaults = Object.fromEntries([...qml.matchAll(/property (?:bool|int|string) (goalEnhance\w+): (.+)/g)]
  .map(([, key, value]) => [key, JSON.parse(value)]));
const state = ctx => Object.fromEntries(Object.keys(defaults).map(key => [key, ctx[key]]));
const plain = value => JSON.parse(JSON.stringify(value));

function fixture() {
  const view = () => ({ resetView(){}, positionViewAtBeginning(){} });
  const ctx = { ...defaults, goalField: {text: '  rough\nidea  '}, projectViewRevision: 1,
    engineState: {project: '/a'}, lastProject: '/a', engineOnline: true, busy: false,
    editingPlan: false, revisePending: false, localError: '', calls: [], goalDrafts: {},
    feedbackField: {text: ''}, questionField: {text: ''}, chatList: {}, goalFlick: {},
    liveEntries: {clear(){}}, historyEntries: {clear(){}}, liveOutput: view(),
    historyList: view(), reportList: view(), logFeed: {},
    DetailView: { invalidate(){}, newFeed(){ return {}; } }, Qt: {callLater(){}},
    cancelPlanEdit(){}, syncHistory(){}, syncReviewViews(){}, refreshAgentLog(){} };
  ctx.root = ctx;
  ctx.act = (path, body, done) => ctx.calls.push({path, body, done});
  ctx.enhanceGoalButton = { get enabled() { return vm.runInNewContext(enabled, ctx); } };
  vm.runInNewContext(helpers + stateHandler, ctx);
  ctx.poll = snapshot => {
    ctx.engineState = {project: ctx.lastProject, goal_enhancement: snapshot};
    ctx.engineStateChanged();
  };
  ctx.start = (id = 1) => { ctx.enhanceGoal(); ctx.calls.at(-1).done({request_id: id}, 200); };
  return ctx;
}
const ready = (request_id = 1) => ({status: 'ready', request_id, original: 'rough\nidea', goal: 'Clear description'});

test('polling auto-applies against exact submitted text once; undo survives later polls', () => {
  const ctx = fixture(), original = ctx.goalField.text;
  ctx.enhanceGoal();
  assert.deepEqual(plain(ctx.calls[0].body), {goal: original});
  assert.equal(ctx.calls[0].path, '/api/goal/enhance');
  assert.equal(ctx.goalEnhancePending, true);
  assert.equal(ctx.goalEnhanceSent, original);
  assert.equal(ctx.goalEnhanceRequest, -1);
  ctx.calls[0].done({request_id: 1}, 200);
  ctx.poll(ready());
  assert.equal(ctx.goalField.text, 'Clear description');
  assert.equal(ctx.goalEnhanceUndo, original);
  assert.equal(ctx.goalEnhancePending, false);
  assert.equal(ctx.goalEnhanceRequest, -1);
  const consumed = state(ctx);
  ctx.poll(ready());
  assert.deepEqual(state(ctx), consumed);
  ctx.undoGoalEnhancement();
  ctx.poll(ready());
  assert.equal(ctx.goalField.text, original);
  assert.equal(ctx.goalEnhanceUndo, '');
  assert.equal(ctx.goalEnhanceReady, '');
});

test('edited field offers once; explicit Apply and Undo preserve the newer draft through polling', () => {
  const ctx = fixture();
  ctx.start();
  ctx.goalField.text = 'new typing';
  ctx.poll(ready());
  assert.equal(ctx.goalField.text, 'new typing');
  assert.equal(ctx.goalEnhanceReady, 'Clear description');
  assert.equal(ctx.goalEnhancePending, false);
  assert.equal(ctx.goalEnhanceRequest, -1);
  const offered = state(ctx);
  ctx.poll(ready());
  assert.deepEqual(state(ctx), offered);
  ctx.goalField.text = 'even more typing';
  ctx.applyGoalEnhancement();
  ctx.poll(ready());
  assert.equal(ctx.goalField.text, 'Clear description');
  assert.equal(ctx.goalEnhanceReady, '');
  assert.equal(ctx.goalEnhanceUndo, 'even more typing');
  ctx.undoGoalEnhancement();
  ctx.poll(ready());
  assert.equal(ctx.goalField.text, 'even more typing');
  assert.equal(ctx.goalEnhanceReady, '');
  assert.equal(ctx.goalEnhanceUndo, '');
});

test('helper ignores absent, inactive, mismatched and running snapshots without state changes', () => {
  const ctx = fixture();
  ctx.start(2);
  const before = state(ctx), text = ctx.goalField.text;
  for (const snapshot of [null, undefined, ready(1), ready(3), {...ready(2), status: 'running'}]) {
    assert.deepEqual(plain(ctx.goalEnhancementAction(snapshot, 2, text, text)), {action: 'none'});
    ctx.poll(snapshot);
    assert.deepEqual(state(ctx), before);
    assert.equal(ctx.goalField.text, text);
  }
  assert.deepEqual(plain(ctx.goalEnhancementAction(ready(-1), -1, text, text)), {action: 'none'});
  assert.deepEqual(plain(ctx.goalEnhancementAction(ready(2), 2, text, text)), {action: 'apply', text: 'Clear description'});
  assert.deepEqual(plain(ctx.goalEnhancementAction(ready(2), 2, 'edited', text)), {action: 'offer', text: 'Clear description'});
});

test('worker failures retain the goal and expose reported or nonblank fallback errors once', () => {
  for (const error of ['provider failed', '', ' \n ', null, undefined]) {
    const ctx = fixture(), original = ctx.goalField.text;
    ctx.start();
    const snapshot = {status: 'failed', request_id: 1, error};
    const outcome = ctx.goalEnhancementAction(snapshot, 1, original, original);
    assert.equal(outcome.action, 'error');
    assert.ok(outcome.error.trim());
    if (error === 'provider failed') assert.equal(outcome.error, error);
    ctx.poll(snapshot);
    assert.equal(ctx.goalField.text, original);
    assert.equal(ctx.goalEnhanceError, outcome.error);
    assert.equal(ctx.goalEnhancePending, false);
    assert.equal(ctx.goalEnhanceRequest, -1);
    ctx.goalEnhanceError = '';
    ctx.poll(snapshot);
    assert.equal(ctx.goalEnhanceError, '');
  }
});

test('new submission invalidates the old ID before act refresh and reconciles a fast result after callback', () => {
  const ctx = fixture();
  ctx.goalEnhanceRequest = 1;
  ctx.goalEnhanceReady = 'old offer';
  ctx.goalEnhanceUndo = 'previous undo';
  ctx.goalEnhanceError = 'old error';
  ctx.act = (path, body, done) => {
    assert.equal(ctx.goalEnhanceRequest, -1);
    ctx.poll(ready(1));
    assert.equal(ctx.goalField.text, body.goal);
    assert.equal(ctx.goalEnhancePending, true);
    assert.equal(ctx.goalEnhanceReady, '');
    assert.equal(ctx.goalEnhanceError, '');
    assert.equal(ctx.goalEnhanceUndo, 'previous undo');
    ctx.poll(ready(2));
    assert.equal(ctx.goalField.text, body.goal);
    done({request_id: 2}, 200);
  };
  ctx.enhanceGoal();
  assert.equal(ctx.goalField.text, 'Clear description');
  assert.equal(ctx.goalEnhancePending, false);
  assert.equal(ctx.goalEnhanceRequest, -1);
});

test('HTTP errors are shown and transport diagnostics use the shared local error', () => {
  for (const [status, resp] of [[400, {error: 'goal too long'}], [409, {error: 'busy'}], [0, null], [502, {}]]) {
    const ctx = fixture(), original = ctx.goalField.text;
    ctx.enhanceGoal();
    ctx.calls[0].done(resp, status);
    assert.equal(ctx.goalField.text, original);
    assert.equal(ctx.goalEnhancePending, false);
    assert.equal(ctx.goalEnhanceRequest, -1);
    if (resp?.error) {
      assert.equal(ctx.goalEnhanceError, resp.error);
      assert.equal(ctx.localError, '');
    } else {
      assert.equal(ctx.goalEnhanceError, '');
      assert.equal(ctx.localError, 'Could not enhance the description. Check the engine connection and try again.');
    }
  }
});

test('project changes reset all six properties and callbacks cannot cross project visits', () => {
  assert.deepEqual(defaults, {goalEnhancePending: false, goalEnhanceRequest: -1,
    goalEnhanceSent: '', goalEnhanceReady: '', goalEnhanceUndo: '', goalEnhanceError: ''});
  for (const [status, resp] of [[200, {request_id: 1}], [400, {error: 'old failure'}], [0, null]]) {
    const ctx = fixture();
    ctx.enhanceGoal();
    const old = ctx.calls[0];
    ctx.goalEnhanceRequest = 1;
    ctx.goalEnhanceReady = 'offer';
    ctx.goalEnhanceUndo = 'undo';
    ctx.goalEnhanceError = 'error';
    ctx.goalDrafts['/b'] = 'other goal';
    for (const project of ['/b', '/a']) {
      ctx.engineState = {project, goal_enhancement: ready()};
      ctx.engineStateChanged();
      assert.deepEqual(state(ctx), defaults);
      const text = ctx.goalField.text;
      old.done(resp, status);
      ctx.poll(ready());
      assert.deepEqual(state(ctx), defaults);
      assert.equal(ctx.goalField.text, text);
      assert.equal(ctx.localError, '');
    }
    ctx.start(2);
    const newer = state(ctx);
    old.done(resp, status);
    assert.deepEqual(state(ctx), newer);
    assert.equal(ctx.projectViewRevision, 3);
  }
});

test('button enablement and enhanceGoal share every required guard', () => {
  for (const [key, value] of [['engineOnline', false], ['busy', true], ['editingPlan', true],
    ['revisePending', true], ['goalEnhancePending', true], ['text', ' \n ']]) {
    const ctx = fixture();
    assert.equal(ctx.enhanceGoalButton.enabled, true);
    if (key === 'text') ctx.goalField.text = value;
    else ctx[key] = value;
    assert.equal(ctx.enhanceGoalButton.enabled, false);
    const before = state(ctx);
    ctx.enhanceGoal();
    assert.equal(ctx.calls.length, 0);
    assert.deepEqual(state(ctx), before);
  }
});

test('goal buttons, wrapping plain status and exact Shift+E shortcut are wired with lowercase e preserved', () => {
  for (const [label, action] of [['Enhance with AI', 'enhanceGoal'], ['Apply AI description', 'applyGoalEnhancement'], ['Undo enhance', 'undoGoalEnhancement']])
    assert.match(qml, new RegExp(`label: "${label}"[\\s\\S]*?onClicked: root\\.${action}\\(\\)`));
  assert.match(qml, /visible: root.goalEnhanceReady !== ""/);
  assert.match(qml, /visible: root.goalEnhanceUndo !== ""/);
  const status = slice('          text: root.goalEnhancePending ?', '            id: createPlanButton');
  for (const text of ['enhancing the description…', 'root.goalEnhanceError', 'AI rewrite ready — press Apply AI description',
    'Text.PlainText', 'Text.Wrap', 'root.urgent', 'root.mutedForeground', 'root.fontFamily', 'root.fs(11)']) assert.ok(status.includes(text));
  assert.match(qml, /event.key === Qt.Key_E && event.modifiers === Qt.ShiftModifier\) \{\s*root.enhanceGoal\(\)\s*event.accepted = true/);
  assert.match(qml, /event.modifiers === Qt.NoModifier\) \{[\s\S]*?event.key === Qt.Key_E\) \{\s*if \(editPlanButton.enabled\) root.beginPlanEdit\(\)/);
  assert.match(qml, /\{ key: "E", description: "Enhance the goal description with AI" \}/);
});
