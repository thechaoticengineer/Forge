import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { installGuards } from './panel_guards.mjs';

const qml = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
const head = qml.match(/readonly property var queueHead: (.+)/)[1];
const ready = qml.match(/readonly property bool hasQueuedGoals: (.+)/)[1];
const start = qml.indexOf('  function canMoveQueueGoal(');
const end = qml.indexOf('  readonly property string phase:', start);
const button = qml.slice(qml.indexOf('id: startQueueButton'), qml.indexOf('id: queueList'));
const enabled = button.match(/enabled: (.+)/)[1];
// The button reads the shared guard; evaluate Panel.qml's guard binding for a root state.
const panelRoot = state => installGuards(qml, {phase: 'idle', revisePending: false, chatPending: false,
  plan: null, goalEnhancePending: false, goalField: {text: ''}, feedbackField: {text: ''},
  questionField: {text: ''}, ...state});

test('Start queue can resume the first paused goal, including after reload', () => {
  for (const status of ['blocked', 'failed', 'running', 'planning', 'awaiting_approval', 'unknown']) {
    const queue = [{status}, {status: 'queued'}];
    const context = {queue};
    context.queueHead = vm.runInNewContext(head, context);
    assert.equal(vm.runInNewContext(ready, context), status !== 'unknown', status);
  }
  for (const queue of [[], [{status:'done'}], [{status:'done'}, {status:'queued'}], [{status:'queued'}]]) {
    const context = {queue};
    context.queueHead = vm.runInNewContext(head, context);
    assert.equal(vm.runInNewContext(ready, context), queue.some(item => item.status === 'queued'));
  }
});

test('Start queue remains available for a stopped goal and cannot interrupt active work', () => {
  assert.match(button, /label: "Start queue"/);
  assert.match(button, /onClicked: root.act\("\/api\/queue\/start"\)/);
  for (const status of ['queued', 'blocked', 'failed', 'running', 'planning', 'awaiting_approval']) {
    const context = {queueHead: {status}};
    const state = {hasQueuedGoals: vm.runInNewContext(ready, context), engineOnline: true,
      editingPlan: false, busy: false, queueActive: false};
    assert.equal(vm.runInNewContext(enabled, {root: panelRoot(state)}), true, status);
    for (const key of ['editingPlan', 'busy', 'queueActive']) {
      assert.equal(vm.runInNewContext(enabled, {root: panelRoot({...state, [key]: true})}), false, key);
    }
    assert.equal(vm.runInNewContext(enabled, {root: panelRoot({...state, engineOnline: false})}), false);
  }
});

test('queue move controls preserve barriers while allowing pending goals to reorder', () => {
  const context = {};
  vm.runInNewContext(qml.slice(start, end), context);
  for (const status of ['blocked', 'failed', 'running', 'planning', 'awaiting_approval', 'done', 'queued']) {
    context.queue = [{status:'queued'}, {status}, {status:'queued'}];
    const expected = status === 'queued' || status === 'done';
    assert.equal(context.canMoveQueueGoal(0, 1), expected, status);
    assert.equal(context.canMoveQueueGoal(2, -1), expected, status);
    assert.equal(context.canMoveQueueGoal(0, -1), false);
    assert.equal(context.canMoveQueueGoal(2, 1), false);
  }
});
