import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const qml = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
const discussionJsSource = readFileSync(new URL('../quickshell/Discussion.js', import.meta.url), 'utf8');
const slice = (start, end) => qml.slice(qml.indexOf(start), qml.indexOf(end));
const discussionCallbacks = slice('  function sendDiscussionMessage(', '  function beginPlanEdit(');
const stateHandler = slice('  onEngineStateChanged: {', '  onBusyChanged: {')
  .replace('onEngineStateChanged: {', 'function engineStateChanged() {');
const defaults = Object.fromEntries([...qml.matchAll(/property (?:bool|int|string) (discussion(?:Pending|Request|Sent|Error|Expanded)): (.+)/g)]
  .map(([, key, value]) => [key, JSON.parse(value)]));
const state = ctx => Object.fromEntries(Object.keys(defaults).map(key => [key, ctx[key]]));
const plain = value => JSON.parse(JSON.stringify(value));

function propExpr(name, until) {
  const re = new RegExp('readonly property bool ' + name + ': ([\\s\\S]*?)\\n\\s*' + until);
  return qml.match(re)[1];
}
const canSendExpr = propExpr('discussionCanSend', 'readonly property bool discussionCanPlan');
const canPlanExpr = propExpr('discussionCanPlan', 'readonly property bool discussionCanClear');
const canClearExpr = propExpr('discussionCanClear', 'readonly property var planReview');

function loadDiscussion() {
  const module = {};
  vm.runInNewContext(discussionJsSource, module);
  return module;
}

function fixture() {
  const view = () => ({ resetView(){}, positionViewAtBeginning(){} });
  const liveOutput = view(), historyList = view(), reportList = view();
  const ctx = { ...defaults, goalField: {text: 'refined goal'}, projectViewRevision: 1,
    engineState: {project: '/a', discussion: [], discussion_activity: null}, lastProject: '/a',
    engineOnline: true, busy: false, queueActive: false,
    editingPlan: false, revisePending: false, localError: '', calls: [], goalDrafts: {},
    feedbackField: {text: ''}, questionField: {text: ''}, chatList: {}, goalFlick: {},
    discussion: [], discussionView: {input: {text: ''}},
    liveEntries: {clear(){}}, historyEntries: {clear(){}}, liveOutput,
    historyList, reportList, agentOutput: {liveOutput, historyList, reportList}, logFeed: {},
    DetailView: { invalidate(){}, newFeed(){ return {}; } }, Qt: {callLater(){}},
    cancelPlanEdit(){}, syncHistory(){}, syncGoalEnhancement(){}, syncCatalogueSuggestion(){},
    syncReviewViews(){}, refreshAgentLog(){} };
  ctx.root = ctx;
  ctx.act = (path, body, done) => ctx.calls.push({path, body, done});
  ctx.Discussion = loadDiscussion();
  Object.defineProperty(ctx, 'discussionCanSend', { get() { return vm.runInNewContext(canSendExpr, ctx); } });
  Object.defineProperty(ctx, 'discussionCanPlan', { get() { return vm.runInNewContext(canPlanExpr, ctx); } });
  Object.defineProperty(ctx, 'discussionCanClear', { get() { return vm.runInNewContext(canClearExpr, ctx); } });
  vm.runInNewContext(discussionCallbacks + stateHandler, ctx);
  ctx.poll = activity => {
    ctx.engineState = {project: ctx.lastProject, discussion: ctx.discussion, discussion_activity: activity};
    ctx.engineStateChanged();
  };
  return ctx;
}

test('Discussion.js: discussionAction only decides on a matching, terminal activity', () => {
  const d = loadDiscussion();
  assert.deepEqual(plain(d.discussionAction(null, 1)), {action: 'none'});
  assert.deepEqual(plain(d.discussionAction(undefined, 1)), {action: 'none'});
  assert.deepEqual(plain(d.discussionAction({status: 'running', request_id: 1}, 1)), {action: 'none'});
  assert.deepEqual(plain(d.discussionAction({status: 'ready', request_id: 1}, -1)), {action: 'none'});
  assert.deepEqual(plain(d.discussionAction({status: 'ready', request_id: 2}, 1)), {action: 'none'});
  assert.deepEqual(plain(d.discussionAction({status: 'ready', request_id: 1}, 1)), {action: 'ready'});
  assert.deepEqual(plain(d.discussionAction({status: 'failed', request_id: 1, error: 'boom', message: 'hi'}, 1)),
    {action: 'error', error: 'boom', message: 'hi'});
  for (const error of ['', ' \n ', null, undefined]) {
    assert.deepEqual(plain(d.discussionAction({status: 'failed', request_id: 1, error, message: 'hi'}, 1)),
      {action: 'error', error: 'Could not get a reply. Try again.', message: 'hi'});
  }
  assert.deepEqual(plain(d.discussionAction({status: 'failed', request_id: 1}, 1)),
    {action: 'error', error: 'Could not get a reply. Try again.', message: ''});
});

test('Discussion.js: canPlanFromDiscussion needs a user entry directly followed by a reply', () => {
  const d = loadDiscussion();
  assert.equal(d.canPlanFromDiscussion([]), false);
  assert.equal(d.canPlanFromDiscussion(undefined), false);
  assert.equal(d.canPlanFromDiscussion([{role: 'user', text: 'a'}]), false);
  assert.equal(d.canPlanFromDiscussion([{role: 'assistant', text: 'a'}, {role: 'user', text: 'b'}]), false);
  assert.equal(d.canPlanFromDiscussion([{role: 'user', text: 'a'}, {role: 'assistant', text: 'b'}]), true);
  assert.equal(d.canPlanFromDiscussion([{role: 'user', text: 'a'}, {role: 'user', text: 'b'}, {role: 'assistant', text: 'c'}]), true);
});

test('sendDiscussionMessage posts exactly {message} and never posts /api/plan', () => {
  const ctx = fixture();
  ctx.discussionView.input.text = 'hello there';
  ctx.sendDiscussionMessage('hello there');
  assert.equal(ctx.calls.length, 1);
  assert.equal(ctx.calls[0].path, '/api/discussion/message');
  assert.deepEqual(plain(ctx.calls[0].body), {message: 'hello there'});
  assert.equal(ctx.discussionPending, true);
  assert.equal(ctx.discussionSent, 'hello there');
  ctx.calls[0].done({ok: true, request_id: 1}, 200);
  ctx.poll({status: 'ready', request_id: 1, unix: 1});
  assert.ok(!ctx.calls.some(call => call.path === '/api/plan'));
  assert.equal(ctx.calls.length, 1);
});

test('a blank message is never sent', () => {
  const ctx = fixture();
  ctx.sendDiscussionMessage('   ');
  assert.equal(ctx.calls.length, 0);
  assert.equal(ctx.discussionPending, false);
});

test('pending stays until a matching ready poll; stale and running polls are ignored', () => {
  const ctx = fixture();
  ctx.discussionView.input.text = 'hello';
  ctx.sendDiscussionMessage('hello');
  ctx.calls[0].done({ok: true, request_id: 5}, 200);
  assert.equal(ctx.discussionRequest, 5);
  assert.equal(ctx.discussionPending, true);
  assert.equal(ctx.discussionView.input.text, '');
  ctx.poll({status: 'ready', request_id: 4, unix: 1});
  assert.equal(ctx.discussionPending, true);
  assert.equal(ctx.discussionRequest, 5);
  ctx.poll({status: 'running', request_id: 5, unix: 1});
  assert.equal(ctx.discussionPending, true);
  ctx.poll({status: 'ready', request_id: 5, unix: 1});
  assert.equal(ctx.discussionPending, false);
  assert.equal(ctx.discussionRequest, -1);
});

test('input clears on 200 only when it still equals the sent text', () => {
  const ctx = fixture();
  ctx.discussionView.input.text = 'hello';
  ctx.sendDiscussionMessage('hello');
  ctx.discussionView.input.text = 'edited while sending';
  ctx.calls[0].done({ok: true, request_id: 1}, 200);
  assert.equal(ctx.discussionView.input.text, 'edited while sending');
});

test('a failed activity shows the error and restores the message into an empty input', () => {
  const ctx = fixture();
  ctx.discussionView.input.text = 'hello';
  ctx.sendDiscussionMessage('hello');
  ctx.calls[0].done({ok: true, request_id: 1}, 200);
  assert.equal(ctx.discussionView.input.text, '');
  ctx.poll({status: 'failed', request_id: 1, message: 'hello', error: 'agent failed', unix: 1});
  assert.equal(ctx.discussionPending, false);
  assert.equal(ctx.discussionRequest, -1);
  assert.equal(ctx.discussionError, 'agent failed');
  assert.equal(ctx.discussionView.input.text, 'hello');
});

test('a failed activity never overwrites text already typed after the request', () => {
  const ctx = fixture();
  ctx.sendDiscussionMessage('hello');
  ctx.calls[0].done({ok: true, request_id: 1}, 200);
  ctx.discussionView.input.text = 'new draft';
  ctx.poll({status: 'failed', request_id: 1, message: 'hello', error: 'agent failed', unix: 1});
  assert.equal(ctx.discussionView.input.text, 'new draft');
  assert.equal(ctx.discussionError, 'agent failed');
});

test('HTTP failure clears pending and surfaces the reported or a connection error', () => {
  for (const [status, resp] of [[400, {error: 'message required'}], [409, {error: 'busy'}], [0, null]]) {
    const ctx = fixture();
    ctx.sendDiscussionMessage('hello');
    ctx.calls[0].done(resp, status);
    assert.equal(ctx.discussionPending, false);
    assert.equal(ctx.discussionRequest, -1);
    if (resp && resp.error) assert.equal(ctx.discussionError, resp.error);
    else assert.match(ctx.discussionError, /Could not send the message/);
  }
});

test('planFromDiscussion posts /api/plan with discussion:true and the goal text', () => {
  const ctx = fixture();
  ctx.discussion = [{role: 'user', text: 'a'}, {role: 'assistant', text: 'b'}];
  ctx.goalField.text = 'refined goal';
  ctx.planFromDiscussion();
  assert.equal(ctx.calls.length, 1);
  assert.equal(ctx.calls[0].path, '/api/plan');
  assert.deepEqual(plain(ctx.calls[0].body), {discussion: true, goal: 'refined goal'});
});

test('discussionCanPlan requires a replied discussion and shares every Send guard', () => {
  const ctx = fixture();
  assert.equal(ctx.discussionCanPlan, false);
  ctx.discussion = [{role: 'user', text: 'a'}];
  assert.equal(ctx.discussionCanPlan, false);
  ctx.discussion = [{role: 'user', text: 'a'}, {role: 'assistant', text: 'b'}];
  assert.equal(ctx.discussionCanPlan, true);
  for (const [key, value] of [['busy', true], ['queueActive', true], ['editingPlan', true],
    ['revisePending', true], ['discussionPending', true], ['engineOnline', false]]) {
    const before = ctx[key];
    ctx[key] = value;
    assert.equal(ctx.discussionCanPlan, false, key);
    assert.equal(ctx.discussionCanSend, false, key);
    ctx[key] = before;
  }
  assert.equal(ctx.discussionCanPlan, true);
  ctx.planFromDiscussion();
  assert.equal(ctx.calls.length, 1);
  ctx.busy = true;
  ctx.planFromDiscussion();
  assert.equal(ctx.calls.length, 1);
});

test('clearDiscussion posts the reset endpoint and is gated like the Clear button', () => {
  const ctx = fixture();
  assert.equal(ctx.discussionCanClear, false);
  ctx.clearDiscussion();
  assert.equal(ctx.calls.length, 0);
  ctx.discussion = [{role: 'user', text: 'a'}];
  assert.equal(ctx.discussionCanClear, true);
  ctx.clearDiscussion();
  assert.equal(ctx.calls.length, 1);
  assert.equal(ctx.calls[0].path, '/api/discussion/reset');
});

test('project switch resets discussion state and the input', () => {
  assert.deepEqual(defaults, {discussionPending: false, discussionRequest: -1,
    discussionSent: '', discussionError: '', discussionExpanded: false});
  const ctx = fixture();
  ctx.discussionPending = true;
  ctx.discussionRequest = 3;
  ctx.discussionSent = 'x';
  ctx.discussionError = 'err';
  ctx.discussionExpanded = true;
  ctx.discussionView.input.text = 'typed but unsent';
  ctx.engineState = {project: '/b', discussion: [], discussion_activity: null};
  ctx.engineStateChanged();
  assert.deepEqual(state(ctx), defaults);
  assert.equal(ctx.discussionView.input.text, '');
  assert.equal(ctx.lastProject, '/b');
});
