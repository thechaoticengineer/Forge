import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const qml = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
const context = {};
vm.runInNewContext(qml.slice(qml.indexOf('  function reviewGateText('), qml.indexOf('  function reviewRoundLabel(')), context);
test('documentation architect is explicitly not required, never approved', () => {
  const text = context.reviewGateText({review_policy:{scope:'ordinary_documentation',rationale:'Prose only'},review_gate:{status:'approved',roles:{architect:'not_required',reviewer:'approved'}}});
  assert.match(text,/Architect: review not required/); assert.match(text,/Independent: approved/);
  assert.doesNotMatch(text,/Architect: approved/); assert.match(text,/Prose only/);
});
test('current gate is independent of historical approval and partial outcomes', () => {
  const stage = {last_verdict:{approved:true,issues:[],notes:[]}, review_gate:{status:'error',roles:{reviewer:'approved',architect:'pending'}}};
  assert.match(context.reviewGateText(stage),/gate: error/);
  assert.match(context.reviewGateText(stage),/Architect: pending/);
  assert.equal(context.stageReviews(stage)[0].verdict,stage.last_verdict);
  assert.equal(context.reviewDecision(stage.last_verdict).clean,true);
  assert.match(context.reviewGateText({}),/gate: pending/);
});
test('historical legacy note rendering remains readable', () => {
  assert.equal(context.reviewDecision({approved:true,notes:['Old note']}).label,'approved with optional notes');
  assert.match(context.reviewDecision({approved:false,notes:['Requested edit']}).label,/changes requested/);
});
