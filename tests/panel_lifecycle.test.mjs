import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const panel = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
const bar = readFileSync(new URL('../quickshell/BarWidget.qml', import.meta.url), 'utf8');
const ctx = {};
vm.runInNewContext(panel.slice(panel.indexOf('  function stageModelText('), panel.indexOf('  function catalogueProviderText(')), ctx);
vm.runInNewContext(bar.slice(bar.indexOf('  function lifecycleSummary('), bar.indexOf('  readonly property string statusGlyph:')), ctx);
const agreement = {id:'agreement', valid:true, effective:{provider:'codex',model:'resolved',native_effort:'high'},
  validated_proposal:{provider:'codex',model:'alias',native_effort:'high'},
  verification_state:'configured_unverified', planner_reason:'Configured adequacy', architect_reason:'Preserve interfaces',
  policy_inputs:{tier:'strong',tier_provenance:'configured',relative_cost_preference:1,pricing:null}};
test('proposed and effective identities, native effort, relative preference and unavailable prices stay distinct', () => {
  const text = ctx.stageModelText({model_agreement:agreement,model_invocations:[{effective:agreement.effective,status:'completed'}]},true);
  for (const value of ['Proposed: codex/alias · high','Execution: codex/resolved · high','configured relative preference 1 (not a price)','Routing price: unavailable / no comparable rate used']) assert.ok(text.includes(value),value);
});
test('archived reports retain reasons, separate review outcomes and role totals; legacy reports still render', () => {
  const report = {plan_id:'old-plan',revision:2,architecture:{summary:'Preserve greeting'},
    stage_outcomes:[{id:1,title:'Spelling',status:'committed',model_agreement:agreement,
      review_gate:{status:'approved',roles:{architect:'not_required',reviewer:'approved'}}}],
    role_usage:{reviewer:{claude:{total_tokens:15}}}};
  const text = ctx.reportLifecycleText(report);
  for (const value of ['old-plan','Preserve greeting','Configured adequacy','Preserve interfaces','Recorded aggregate: approved','architect: not required','reviewer: 15 tokens','history?plan_id=old-plan']) assert.ok(text.includes(value),value);
  assert.equal(ctx.reportLifecycleText({goal:'Legacy'}),'');
});
test('pending recovery and current gates take precedence over old approval in panel and bar', () => {
  const state = {current_stage:1,architect_activity:{status:'ready'},architecture:{context_status:'needs_recovery'},
    plan:{stages:[{id:1,model_agreement:agreement,review_gate:{status:'interrupted',roles:{architect:'pending',reviewer:'pending'}},last_verdict:{approved:true}}]}};
  assert.match(ctx.architectActivityText(state.architect_activity,state.architecture),/needs_recovery/);
  const text = ctx.lifecycleSummary(state);
  for (const value of ['needs recovery','configured_unverified','Current gate: interrupted','independent: pending','architect: pending']) assert.ok(text.includes(value),value);
  assert.ok(!text.includes('approved'));
  assert.match(ctx.lifecycleSummary({}),/inactive/);
});
