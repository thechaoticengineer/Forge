import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const qml = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
const context = {};
vm.runInNewContext(qml.slice(qml.indexOf('  function architectActivityText('), qml.indexOf('  function catalogueProviderText(')), context);
test('legacy plans render without architect fields', () => {
  assert.match(context.architectActivityText(null, null), /legacy/);
  assert.equal(context.architectGuidanceText(null), '');
  assert.equal(context.architectUsageText({stages:[]}), '');
});
test('architect activity recovery and current guidance are visible', () => {
  assert.match(context.architectActivityText({status:'recovering',reason:'session expired'}, null), /recovering · session expired/);
  assert.match(context.architectActivityText({status:'failed',error:'invalid output'}, null), /failed · invalid output/);
  const cp = {context_status:'ready',session:{reference:'exact-id'},recovery:{reason:'provider changed'},
    guidance:{1:{valid:true,text:'Keep API stable'},2:{valid:false,text:'Check UI'}}};
  assert.match(context.architectActivityText(null,cp), /provider changed.*exact-id/);
  assert.equal(context.architectGuidanceText(cp),'Stage 1: Keep API stable\nStage 2 (needs refresh): Check UI');
});
test('decision rationale supersessions alternatives and role usage render', () => {
  const text = context.architectDecisionText({id:'d2',status:'accepted',summary:'Stable IDs',rationale:'Clients persist IDs',
    supersedes:'d1',alternatives:[{description:'Names',tradeoffs:'Rename breaks clients'}]});
  assert.match(text,/Why: Clients persist IDs/); assert.match(text,/Supersedes: d1/); assert.match(text,/Rename breaks clients/);
  assert.equal(context.architectUsageText({role_usage:{architect:{codex:{total_tokens:123}},planner:{claude:{total_tokens:50}}}}),
    'architect: 123 tokens · planner: 50 tokens');
});
