import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const qml = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
const head = qml.match(/readonly property var queueHead: (.+)/)[1];
const ready = qml.match(/readonly property bool hasQueuedGoals: (.+)/)[1];
const start = qml.indexOf('  function canMoveQueueGoal(');
const end = qml.indexOf('  readonly property string phase:', start);

test('queue start stays disabled behind an unfinished goal, including after reload', () => {
  for (const status of ['blocked', 'failed', 'running', 'planning', 'awaiting_approval', 'unknown']) {
    const queue = [{status}, {status: 'queued'}];
    const context = {queue};
    context.queueHead = vm.runInNewContext(head, context);
    assert.equal(vm.runInNewContext(ready, context), false, status);
  }
  for (const queue of [[], [{status:'done'}], [{status:'done'}, {status:'queued'}], [{status:'queued'}]]) {
    const context = {queue};
    context.queueHead = vm.runInNewContext(head, context);
    assert.equal(vm.runInNewContext(ready, context), queue.some(item => item.status === 'queued'));
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
