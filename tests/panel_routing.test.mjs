import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const qml = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
const ctx = {};
vm.runInNewContext(qml.slice(qml.indexOf('  function stageModelText('), qml.indexOf('  function architectActivityText(')), ctx);
const stage = {model_agreement: {id:'agreed-1', valid:true, availability:'unverified',
  effective:{provider:'codex',model:'configured-model',native_effort:'high'},
  policy_inputs:{tier:'strong',tier_provenance:'configured',relative_cost_preference:2,policy:'stage-routing-1'},
  planner_reason:'Complex storage requires a strong tier.', architect_reason:'A failure breaks dependent interfaces.',
  validated_proposal:{risk:'critical',complexity:'complex'}}};
test('collapsed drafts show both reasons, native effort, unverified availability and configured tier provenance', () => {
  const text = ctx.stageModelText(stage, false);
  for (const part of ['codex/configured-model', 'high', 'unverified', 'strong (configured)', 'Planner: Complex storage', 'Architect: A failure']) assert.ok(text.includes(part));
  assert.match(qml, /text: root.stageModelText\(stageRow.modelData, stageRow.expanded\)/);
});
test('expanded details distinguish relative preferences from prices and handle unknowns', () => {
  assert.match(ctx.stageModelText(stage,true), /configured relative preference 2 \(not a price\)/);
  assert.match(ctx.stageModelText({},false), /pending/);
  const unknown = structuredClone(stage); unknown.model_agreement.policy_inputs.relative_cost_preference = null;
  unknown.model_agreement.valid = false;
  assert.match(ctx.stageModelText(unknown,true), /unknown \/ no comparable billing data/);
  assert.match(ctx.stageModelText(unknown,false), /Needs reconciliation/);
});
test('stage constraints can be narrowed or cleared and survive manual save', () => {
  ctx.editStages = [{model_constraint:{provider:'codex'}}];
  ctx.changeStageField = (index,key,value) => { ctx.editStages[index][key] = value; };
  ctx.changeModelConstraint(0,'model',' exact-id ');
  assert.equal(ctx.editStages[0].model_constraint.model,'exact-id');
  ctx.changeModelConstraint(0,'model',''); ctx.changeModelConstraint(0,'provider','');
  assert.equal(ctx.editStages[0].model_constraint,null);
  assert.match(qml,/content.model_constraint = stage.model_constraint \|\| null/);
  assert.match(qml,/automatic_routing:/);
});
