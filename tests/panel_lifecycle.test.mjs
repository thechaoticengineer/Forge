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

const cadence = {};
vm.runInNewContext(panel.slice(panel.indexOf('  function reviewCadenceLabel('),panel.indexOf('  function planReviewStatusText(')),cadence);
test('cadence controls toggle only their role and post both normalized keys',()=>{
  for(const current of [{architect:'per_stage',reviewer:'per_plan'}, {architect:'per_plan',reviewer:'per_stage'},
    {architect:'unknown'}, undefined]) for(const role of ['architect','reviewer']) {
    cadence.engineState={settings:{review_cadence:current}};
    let sent;cadence.act=(path,body)=>{assert.equal(path,'/api/settings');sent=body};
    cadence.toggleReviewCadence(role);
    const expected={architect:current?.architect==='per_plan'?'per_plan':'per_stage',
      reviewer:current?.reviewer==='per_plan'?'per_plan':'per_stage'};
    expected[role]=expected[role]==='per_plan'?'per_stage':'per_plan';
    assert.deepEqual(JSON.parse(JSON.stringify(sent)),{review_cadence:expected});
    assert.deepEqual(cadence.engineState.settings.review_cadence,current,'does not mutate polled state');
  }
  for(const role of ['architect','reviewer']) {
    assert.equal(cadence.reviewCadenceLabel(null,role),'… (unavailable)');
    assert.equal(cadence.reviewCadenceLabel({settings:{}},role),'… (unavailable)');
    assert.equal(cadence.reviewCadenceLabel({settings:{review_cadence:{[role]:'per_plan'}}},role),'per plan');
    assert.equal(cadence.reviewCadenceLabel({settings:{review_cadence:{[role]:'per_stage'}}},role),'per stage');
    assert.equal((panel.match(new RegExp('CadenceButton \\{ role: "'+role+'" \\}','g'))||[]).length,1);
  }
  const button=panel.slice(panel.indexOf('  component CadenceButton:'),panel.indexOf('  component PanelButton:'));
  assert.match(button,/enabled: root.engineOnline/);
  assert.match(button,/activeFocusOnTab: enabled/);
  assert.match(button,/root.toggleReviewCadence\(role\)/);
  for(const key of ['Key_Space','Key_Return','Key_Enter','Key_Tab','Key_Backtab','Key_Escape']) assert.ok(button.includes(key));
  assert.match(button,/Key_Escape\) keyHandler.forceActiveFocus\(\)/);
});
