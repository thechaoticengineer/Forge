import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const qml = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
const discussionViewQml = readFileSync(new URL('../quickshell/DiscussionView.qml', import.meta.url), 'utf8');
const discussionChatQml = readFileSync(new URL('../quickshell/DiscussionChat.qml', import.meta.url), 'utf8');
const discussionMessageQml = readFileSync(new URL('../quickshell/DiscussionMessage.qml', import.meta.url), 'utf8');
const discussionJsSource = readFileSync(new URL('../quickshell/Discussion.js', import.meta.url), 'utf8');
const slice = (start, end) => qml.slice(qml.indexOf(start), qml.indexOf(end));
const discussionCallbacks = slice('  function sendDiscussionMessage(', '  function beginPlanEdit(');
const stateHandler = slice('  onEngineStateChanged: {', '  onBusyChanged: {')
  .replace('onEngineStateChanged: {', 'function engineStateChanged() {');
const defaults = Object.fromEntries([...qml.matchAll(/property (?:bool|int|string) (discussion(?:Pending|Request|Sent|Error)): (.+)/g)]
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
    panelPage: {}, StackView: {Immediate: 0},
    keyHandler: {pendingKey: '', forceActiveFocus(){}},
    liveEntries: {clear(){}}, historyEntries: {clear(){}}, liveOutput,
    historyList, reportList, agentOutput: {liveOutput, historyList, reportList}, logFeed: {},
    DetailView: { invalidate(){}, newFeed(){ return {}; } }, Qt: {callLater(){}},
    cancelPlanEdit(){}, syncHistory(){}, syncGoalEnhancement(){}, syncCatalogueSuggestion(){},
    syncReviewViews(){}, refreshAgentLog(){} };
  ctx.root = ctx;
  ctx.panelStack = {currentItem: ctx.discussionView, pushed: null, poppedTo: null,
    push(item) { this.pushed = item; this.currentItem = item; },
    pop(target) { this.poppedTo = target; this.currentItem = target; }};
  ctx.act = (path, body, done) => ctx.calls.push({path, body, done});
  ctx.Discussion = loadDiscussion();
  Object.defineProperty(ctx, 'discussionOpen', { get() { return ctx.panelStack.currentItem === ctx.discussionView; } });
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

class Model {
  rows = []; writes = []; inserts = []; removes = []; moves = [];
  get count() { return this.rows.length; }
  get(i) { return this.rows[i]; }
  insert(i, r) { this.rows.splice(i, 0, {...r}); this.inserts.push(i); }
  remove(i) { this.rows.splice(i, 1); this.removes.push(i); }
  move(a, b, n) { this.rows.splice(b, 0, ...this.rows.splice(a, n)); this.moves.push([a, b, n]); }
  setProperty(i, k, v) { this.rows[i][k] = v; this.writes.push([i, k, v]); }
}

test('chatMessages: user/assistant transcript keeps order and fields', () => {
  const d = loadDiscussion();
  const entries = [{role: 'user', text: 'hi', unix: 1}, {role: 'assistant', text: 'hello', unix: 2}];
  const rows = plain(d.chatMessages(entries, false, '', ''));
  assert.deepEqual(rows, [
    {key: 'entry/["user",1,"hi"]/0', kind: 'user', outgoing: true, author: 'You', status: '', text: 'hi'},
    {key: 'entry/["assistant",2,"hello"]/0', kind: 'assistant', outgoing: false, author: 'Forge', status: '', text: 'hello'},
  ]);
});

test('chatMessages: unknown roles and null entries are skipped; non-array entries are empty', () => {
  const d = loadDiscussion();
  const entries = [null, {role: 'system', text: 'x', unix: 1}, {role: 'user', text: 'hi', unix: 2}, undefined];
  const rows = plain(d.chatMessages(entries, false, '', ''));
  assert.deepEqual(rows, [{key: 'entry/["user",2,"hi"]/0', kind: 'user', outgoing: true, author: 'You', status: '', text: 'hi'}]);
  assert.deepEqual(plain(d.chatMessages(null, false, '', '')), []);
  assert.deepEqual(plain(d.chatMessages(undefined, false, '', '')), []);
  assert.deepEqual(plain(d.chatMessages('nope', false, '', '')), []);
});

test('chatMessages: null/undefined text is coerced to an empty string', () => {
  const d = loadDiscussion();
  const rows = plain(d.chatMessages([{role: 'user', text: null, unix: 1}], false, '', ''));
  assert.equal(rows[0].text, '');
});

test('chatMessages: keys stay stable when the oldest entry is dropped from the rolling window', () => {
  const d = loadDiscussion();
  const full = [{role: 'user', text: 'a', unix: 1}, {role: 'assistant', text: 'b', unix: 2},
    {role: 'user', text: 'c', unix: 3}, {role: 'assistant', text: 'd', unix: 4}];
  const before = plain(d.chatMessages(full, false, '', '')).map(r => r.key);
  const rolled = full.slice(1);
  const after = plain(d.chatMessages(rolled, false, '', '')).map(r => r.key);
  assert.deepEqual(after, before.slice(1));
});

test('chatMessages: duplicate identical messages get distinct keys', () => {
  const d = loadDiscussion();
  const entries = [{role: 'user', text: 'same', unix: 1}, {role: 'user', text: 'same', unix: 1}];
  const rows = plain(d.chatMessages(entries, false, '', ''));
  assert.equal(rows[0].key, 'entry/["user",1,"same"]/0');
  assert.equal(rows[1].key, 'entry/["user",1,"same"]/1');
  assert.notEqual(rows[0].key, rows[1].key);
});

test('chatMessages: pending with a sent message appends a Sending… row and the reply row', () => {
  const d = loadDiscussion();
  const rows = plain(d.chatMessages([], true, 'hello there', ''));
  assert.deepEqual(rows, [
    {key: 'pending/user', kind: 'user', outgoing: true, author: 'You', status: 'Sending…', text: 'hello there'},
    {key: 'pending/reply', kind: 'pending', outgoing: false, author: 'Forge', status: '', text: 'Forge is replying…'},
  ]);
});

test('chatMessages: pending with a blank sent message gives only the reply row', () => {
  const d = loadDiscussion();
  for (const sentMessage of ['', '   ', null, undefined]) {
    const rows = plain(d.chatMessages([], true, sentMessage, ''));
    assert.deepEqual(rows, [{key: 'pending/reply', kind: 'pending', outgoing: false, author: 'Forge', status: '', text: 'Forge is replying…'}]);
  }
});

test('chatMessages: error rows appear only when not pending and the error is not blank', () => {
  const d = loadDiscussion();
  assert.deepEqual(plain(d.chatMessages([], true, '', 'boom')), [
    {key: 'pending/reply', kind: 'pending', outgoing: false, author: 'Forge', status: '', text: 'Forge is replying…'},
  ]);
  for (const error of ['', '   ', null, undefined]) assert.deepEqual(plain(d.chatMessages([], false, '', error)), []);
  assert.deepEqual(plain(d.chatMessages([], false, '', 'boom')), [
    {key: 'error', kind: 'error', outgoing: false, author: 'Forge', status: 'Reply failed', text: 'boom'},
  ]);
});

test('reconcileChat: an identical second call makes zero writes', () => {
  const d = loadDiscussion();
  const model = new Model();
  const rows = plain(d.chatMessages([{role: 'user', text: 'a', unix: 1}, {role: 'assistant', text: 'b', unix: 2}], false, '', ''));
  d.reconcileChat(model, rows);
  assert.equal(model.count, 2);
  model.writes = []; model.inserts = []; model.removes = []; model.moves = [];
  d.reconcileChat(model, plain(rows));
  assert.deepEqual(model.writes, []);
  assert.deepEqual(model.inserts, []);
  assert.deepEqual(model.removes, []);
  assert.deepEqual(model.moves, []);
});

test('reconcileChat: a pending row is replaced by transcript rows, then entries are cleared', () => {
  const d = loadDiscussion();
  const model = new Model();
  d.reconcileChat(model, plain(d.chatMessages([], true, 'hello', '')));
  assert.deepEqual(model.rows.map(r => r.key), ['pending/user', 'pending/reply']);
  const delegate = model.rows[0];
  d.reconcileChat(model, plain(d.chatMessages([{role: 'user', text: 'hello', unix: 5}, {role: 'assistant', text: 'hi', unix: 6}], false, '', '')));
  assert.deepEqual(model.rows.map(r => r.key), ['entry/["user",5,"hello"]/0', 'entry/["assistant",6,"hi"]/0']);
  assert.ok(!model.rows.includes(delegate));
  d.reconcileChat(model, []);
  assert.equal(model.count, 0);
});

test('DiscussionView renders the chat instead of the old label/value transcript', () => {
  assert.match(discussionViewQml, /DiscussionChat \{/);
  assert.doesNotMatch(discussionViewQml, /DetailFields/);
  assert.doesNotMatch(discussionViewQml, /"You"\s*:\s*"Forge"/);
});

test('DiscussionView keeps the chat, input and send button in top-to-bottom order', () => {
  const chatIndex = discussionViewQml.indexOf('DiscussionChat {');
  const inputIndex = discussionViewQml.indexOf('id: messageInput');
  const sendIndex = discussionViewQml.indexOf('id: sendButton');
  assert.ok(chatIndex >= 0 && inputIndex >= 0 && sendIndex >= 0);
  assert.ok(chatIndex < inputIndex, 'chat comes before the message input');
  assert.ok(inputIndex < sendIndex, 'message input comes before the send button');
});

test('DiscussionView keeps existing input shortcuts and clipboard wiring', () => {
  assert.match(discussionViewQml, /event\.key === Qt\.Key_Return \|\| event\.key === Qt\.Key_Enter\)\s*\n\s*&& event\.modifiers === Qt\.NoModifier/);
  assert.match(discussionViewQml, /Keys\.onEscapePressed: event => \{\s*\n\s*view\.leaveRequested\(\)/);
  assert.match(discussionViewQml, /Qt\.Key_F1/);
  assert.match(discussionViewQml, /Quickshell\.clipboardText/);
});

test('Panel.qml passes the sent message into DiscussionView', () => {
  assert.match(qml, /sentMessage: root\.discussionSent/);
});

test('DiscussionChat.qml and DiscussionMessage.qml stay independent of Quickshell and Panel scope', () => {
  for (const source of [discussionChatQml, discussionMessageQml]) {
    assert.doesNotMatch(source, /import Quickshell/);
    assert.doesNotMatch(source, /qs\.Commons/);
    assert.doesNotMatch(source, /\broot\./);
  }
});

test('project switch closes the chat page and resets discussion state and the input', () => {
  assert.deepEqual(defaults, {discussionPending: false, discussionRequest: -1,
    discussionSent: '', discussionError: ''});
  const ctx = fixture();
  ctx.discussionPending = true;
  ctx.discussionRequest = 3;
  ctx.discussionSent = 'x';
  ctx.discussionError = 'err';
  ctx.panelStack.currentItem = ctx.discussionView;
  assert.equal(ctx.discussionOpen, true);
  ctx.discussionView.input.text = 'typed but unsent';
  ctx.engineState = {project: '/b', discussion: [], discussion_activity: null};
  ctx.engineStateChanged();
  assert.deepEqual(state(ctx), defaults);
  assert.equal(ctx.panelStack.poppedTo, ctx.panelPage);
  assert.equal(ctx.discussionOpen, false);
  assert.equal(ctx.discussionView.input.text, '');
  assert.equal(ctx.lastProject, '/b');
});

test('openDiscussion pushes the chat page once and focuses the composer', () => {
  const ctx = fixture();
  ctx.panelStack.currentItem = ctx.panelPage;
  let focused = 0;
  ctx.discussionView.input.forceActiveFocus = () => focused++;
  ctx.keyHandler.pendingKey = 'g';
  ctx.openDiscussion();
  assert.equal(ctx.panelStack.pushed, ctx.discussionView);
  assert.equal(ctx.discussionOpen, true);
  assert.equal(ctx.keyHandler.pendingKey, '');
  assert.equal(focused, 1);
  ctx.panelStack.pushed = null;
  ctx.openDiscussion();
  assert.equal(ctx.panelStack.pushed, null, 'an open page is never pushed twice');
  assert.equal(focused, 2);
});

test('closeDiscussion pops to the panel page and hands focus back', () => {
  const ctx = fixture();
  let focused = 0;
  ctx.keyHandler.forceActiveFocus = () => focused++;
  ctx.keyHandler.pendingKey = 'g';
  ctx.closeDiscussion();
  assert.equal(ctx.panelStack.poppedTo, ctx.panelPage);
  assert.equal(ctx.keyHandler.pendingKey, '');
  assert.equal(focused, 1);
  ctx.panelStack.poppedTo = null;
  ctx.closeDiscussion();
  assert.equal(ctx.panelStack.poppedTo, null, 'a closed page is never popped again');
  assert.equal(focused, 2);
});

test('DiscussionView is a stack page: a close signal, no expansion and no root geometry', () => {
  assert.match(discussionViewQml, /signal closeRequested/);
  assert.doesNotMatch(discussionViewQml, /expanded/);
  assert.doesNotMatch(discussionViewQml, /expansionRequested/);
  assert.doesNotMatch(discussionViewQml, /property bool open\b/);
  // StackView assigns visible imperatively; a binding here would break push/pop.
  const root = discussionViewQml.slice(discussionViewQml.indexOf('Rectangle {'),
    discussionViewQml.indexOf('Item {'));
  assert.doesNotMatch(root, /\bvisible:/);
  assert.doesNotMatch(root, /\b(?:width|height|anchors)\s*[.:]/);
  assert.match(discussionViewQml, /objectName: "discussionBackButton"/);
});

test('Panel derives discussionOpen from the stack and navigates through the page instance', () => {
  assert.match(qml, /readonly property bool discussionOpen: panelStack\.currentItem === discussionView/);
  assert.match(qml, /function openDiscussion\(\) \{\s*\n\s*if \(!discussionOpen\) panelStack\.push\(discussionView, StackView\.Immediate\)/);
  assert.match(qml, /function closeDiscussion\(\) \{\s*\n\s*if \(discussionOpen\) panelStack\.pop\(panelPage, StackView\.Immediate\)/);
  assert.match(qml, /onCloseRequested: root\.closeDiscussion\(\)/);
  assert.doesNotMatch(qml, /discussionExpanded/);

  const pageIndex = qml.indexOf('id: panelPage');
  const viewIndex = qml.indexOf('DiscussionView {');
  const catalogueIndex = qml.indexOf('CatalogueEditor {');
  assert.ok(pageIndex >= 0 && viewIndex >= 0 && catalogueIndex >= 0);
  assert.ok(pageIndex < viewIndex, 'the chat page is declared after the panel page');
  assert.ok(viewIndex < catalogueIndex, 'the chat page is declared before the overlays');
  // The instance is pushed, never a Component or a URL, so it survives a pop.
  assert.match(qml, /DiscussionView \{\s*\n\s*id: discussionView/);
});

test('the panel column keeps a discussion entry point that opens the chat page', () => {
  const columnIndex = qml.indexOf('id: panelColumn');
  const buttonIndex = qml.indexOf('id: discussionButton');
  const hintIndex = qml.indexOf('id: keyboardHint');
  assert.ok(columnIndex < buttonIndex && buttonIndex < hintIndex,
    'the entry point sits inside the panel column');
  assert.match(qml, /id: discussionButton[\s\S]*?onClicked: root\.openDiscussion\(\)/);
});

test('the t shortcut opens the chat page instead of revealing an inline section', () => {
  const branch = qml.slice(qml.indexOf('event.key === Qt.Key_T'));
  const body = branch.slice(0, branch.indexOf('event.accepted = true'));
  assert.match(body, /root\.openDiscussion\(\)/);
  assert.doesNotMatch(body, /panelScroll\.reveal/);
  assert.doesNotMatch(qml, /panelScroll\.reveal\(discussionView\)/);
});

test('the discussion modal branch routes keys like the other full views', () => {
  const chooserIndex = qml.indexOf('} else if (root.chooserOpen) {');
  const discussionIndex = qml.indexOf('} else if (root.discussionOpen) {');
  const questionIndex = qml.indexOf('} else if (question) {');
  assert.ok(chooserIndex >= 0 && discussionIndex >= 0 && questionIndex >= 0);
  assert.ok(chooserIndex < discussionIndex, 'the discussion branch comes after chooser');
  assert.ok(discussionIndex < questionIndex, 'the discussion branch comes before the question branch');

  const branch = qml.slice(discussionIndex, questionIndex);
  assert.match(branch, /^\} else if \(root\.discussionOpen\) \{\s*\n\s*event\.accepted = true\s*\n/,
    'every event is accepted unconditionally');
  assert.match(branch, /if \(question\) \{\s*\n\s*root\.helpOpen = true/);
  assert.match(branch,
    /event\.key === Qt\.Key_Escape[\s\S]*?event\.key === Qt\.Key_Q && event\.modifiers === Qt\.NoModifier[\s\S]*?\) \{\s*\n\s*root\.closeDiscussion\(\)/);
  assert.match(branch,
    /event\.key === Qt\.Key_I && event\.modifiers === Qt\.NoModifier\) \{\s*\n\s*discussionView\.input\.forceActiveFocus\(\)/);
  assert.doesNotMatch(branch, /root\.discussionOpen\s*=[^=]/,
    'the branch never assigns to the read-only discussionOpen');
});

test('the keyboard help overlay documents the discussion chat', () => {
  const headingIndex = qml.indexOf('{ key: "", description: "Discussion chat" }');
  const keyboardHelpIndex = qml.indexOf('{ key: "", description: "Keyboard help" }');
  assert.ok(headingIndex >= 0 && keyboardHelpIndex >= 0 && headingIndex < keyboardHelpIndex);
  const section = qml.slice(headingIndex, keyboardHelpIndex);
  assert.match(section, /\{ key: "i", description: "Edit the message \(insert mode\)" \}/);
  assert.match(section, /\{ key: "Enter \/ Shift\+Enter", description: "Send the message \/ insert a newline" \}/);
  assert.match(section, /\{ key: "q \/ Escape", description: "Close the chat \(Escape leaves the message field first\)" \}/);
  assert.match(qml, /\{ key: "t", description: "Open the discussion chat" \}/);
});
