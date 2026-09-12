import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const qml = readFileSync(new URL('../quickshell/Panel.qml',import.meta.url),'utf8');
const ctx = {engineState:{settings:{reviewer:'codex',automatic_routing:true}}};
vm.runInNewContext(qml.slice(qml.indexOf('  function reviewerLabel('),qml.indexOf('  function cycleTool(')),ctx);
ctx.act = (url,patch) => {assert.equal(url,'/api/settings');Object.assign(ctx.engineState.settings,patch);};
test('automatic reviewer is labelled honestly; explicit provider survives automatic routing', () => {
  assert.equal(ctx.reviewerLabel(),'reviewer: auto (other provider)');
  ctx.cycleReviewer();
  assert.equal(ctx.reviewerLabel(),'reviewer: codex');
  assert.equal(ctx.engineState.settings.reviewer_provider_mode,'configured');
  assert.equal(ctx.engineState.settings.automatic_routing,true);
  ctx.cycleReviewer();
  assert.equal(ctx.reviewerLabel(),'reviewer: claude');
  ctx.cycleReviewer();
  assert.equal(ctx.reviewerLabel(),'reviewer: auto (other provider)');
});
