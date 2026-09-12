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
  assert.match(qml, /text: root.stageModelStatus\(stageRow.modelData\)/);
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
test('routing status shows bounded retry counts, trigger evidence and both decision reasons', () => {
  const routed = structuredClone(stage);
  routed.reassessment = {status:'blocked',count:2,operational_retries:1,
    limits:{max_reassessments:3,max_operational_retries:2},error:'No adequate eligible model',
    history:[{kind:'repeated_reasoning_failure',evidence:['[reviewer] same failing test'],planner_reason:'Stronger capability needed',architect_reason:'Preserve the invariant'}]};
  const text = ctx.stageModelText(routed,true);
  for (const expected of ['blocked','reassessments 2/3','operational retries 1/2','repeated_reasoning_failure','same failing test','Stronger capability needed','Preserve the invariant','No adequate eligible model']) assert.ok(text.includes(expected));
});

test('stage status stays live while full rationale and optional diagnostics use the snapshot', () => {
  const multiline = structuredClone(stage);
  multiline.model_agreement.planner_reason = '\n  planner first\r\nplanner tail  ';
  multiline.model_agreement.architect_reason = '\rarchitect first\narchitect tail\t';
  const status = ctx.stageModelStatus(multiline);
  for (const value of ['codex/configured-model', 'high', 'unverified', 'strong (configured)']) assert.ok(status.includes(value));
  assert.ok(!status.includes('planner tail'));
  const fields = ctx.stageModelDetails(multiline, false);
  assert.equal(fields.find(f => f.label === 'Planner').text, multiline.model_agreement.planner_reason);
  assert.equal(fields.find(f => f.label === 'Architect').text, multiline.model_agreement.architect_reason);
  multiline.model_block = 'blocked\nfull error';
  assert.equal(ctx.stageModelErrors(multiline), multiline.model_block);
  assert.match(qml, /rationale: stageModelRationale\(stage\), diagnostics: stageModelDiagnostics\(stage\)/);
  assert.match(qml, /model: stageRow.prose \? stageRow.prose.rationale : \[\]/);
  assert.match(qml, /model: root.stageRoutingExpanded && stageRow.prose \? stageRow.prose.diagnostics : \[\]/);
  assert.match(qml, /text: root.stageModelErrors\(stageRow.modelData\)\s+textFormat: Text.PlainText\s+color: root.urgent\s+wrapMode: Text.Wrap/);
});

test('pending model agreements still expose routing outcomes and execution identity', () => {
  const pending={reassessment:{status:'blocked',count:2},model_invocations:[{status:'failed',effective:{provider:'claude',model:'review-model'}}]};
  const text=ctx.stageModelStatus(pending);
  for (const value of ['pending','Routing: blocked','reassessments 2/3','Execution: claude/review-model','failed']) assert.ok(text.includes(value),value);
});

test('model partitions preserve exact ordered fields, pending evidence and the last four routing decisions', () => {
  const routed = structuredClone(stage);
  Object.assign(routed.model_agreement.policy_inputs, {
    constraint: {provider:'codex',model:'configured-model'},
    relative_cost_preference: 0,
    pricing: {input:1.25,output:5},
  });
  routed.model_agreement.planner_reason = '\n  Planner 界🙂\r\ntail  ';
  routed.model_agreement.architect_reason = '\rArchitect\nend\t';
  routed.reassessment = {
    status:'pending',count:2,operational_retries:1,error:'routing error\r\nfull',
    pending:{kind:'pending-trigger',evidence:{message:'pending evidence',attempt:2}},
    history: [
      {kind:'omitted',planner_reason:'Too old',architect_reason:'Also too old'},
      {kind:'first',planner_reason:'First planner',architect_reason:'First architect'},
      {kind:'second',planner_reason:'Second planner',architect_reason:'Second architect'},
      {kind:'third',planner_reason:'Third planner',architect_reason:'Third architect'},
      {kind:'fourth',planner_reason:'Fourth planner',architect_reason:'Fourth architect',evidence:'History must not override pending'},
    ],
  };
  routed.model_invocations = [
    {status:'failed',requested:{provider:'old',model:'old-model'}},
    {status:'succeeded',effective:{provider:'codex',model:'configured-model',native_effort:'high'},verification_state:'verified'},
  ];
  routed.model_block = 'model block\nfull';
  const reasons = [
    {label:'Planner',text:'\n  Planner 界🙂\r\ntail  '},
    {label:'Architect',text:'\rArchitect\nend\t'},
    {label:'Trigger evidence',text:'{"message":"pending evidence","attempt":2}'},
  ];
  const history = [
    {label:'first · Planner 1',text:'First planner'},
    {label:'first · Architect 1',text:'First architect'},
    {label:'second · Planner 2',text:'Second planner'},
    {label:'second · Architect 2',text:'Second architect'},
    {label:'third · Planner 3',text:'Third planner'},
    {label:'third · Architect 3',text:'Third architect'},
    {label:'fourth · Planner 4',text:'Fourth planner'},
    {label:'fourth · Architect 4',text:'Fourth architect'},
  ];
  const diagnostics = [
    {label:'Risk',text:'critical · complexity: complex'},
    {label:'Constraint',text:'{"provider":"codex","model":"configured-model"}'},
    {label:'Cost',text:'configured relative preference 0 (not a price)'},
    {label:'Routing price',text:'API list rate (not CLI spend): {"input":1.25,"output":5}'},
    {label:'Agreement',text:'agreed-1 · policy: stage-routing-1'},
    {label:'Latest invocation',text:'{"status":"succeeded","effective":{"provider":"codex","model":"configured-model","native_effort":"high"},"verification_state":"verified"}'},
  ];
  const detailed = [...reasons, ...history, ...diagnostics];
  // Clone VM results into this realm before strict comparison with literal expectations.
  const compactFields = structuredClone(ctx.stageModelDetails(routed, false));
  const detailedFields = structuredClone(ctx.stageModelDetails(routed, true));
  assert.deepEqual(compactFields.map(f => f.label), ['Planner','Architect','Trigger evidence']);
  assert.deepEqual(detailedFields.map(f => f.label), [
    'Planner','Architect','Trigger evidence',
    'first · Planner 1','first · Architect 1','second · Planner 2','second · Architect 2',
    'third · Planner 3','third · Architect 3','fourth · Planner 4','fourth · Architect 4',
    'Risk','Constraint','Cost','Routing price','Agreement','Latest invocation',
  ]);
  assert.deepEqual(compactFields, reasons);
  assert.deepEqual(detailedFields, detailed);
  assert.deepEqual(structuredClone(ctx.stageModelReasons(routed)), reasons);
  assert.deepEqual(structuredClone(ctx.stageModelRoutingHistory(routed)), history);
  assert.deepEqual(structuredClone(ctx.stageModelDiagnostics(routed)), diagnostics);
  assert.deepEqual(structuredClone(ctx.stageModelRationale(routed)), [...reasons, ...history]);
  assert.deepEqual(ctx.stageModelReasons(routed), ctx.stageModelDetails(routed, false));
  assert.deepEqual(ctx.stageModelRationale(routed).concat(ctx.stageModelDiagnostics(routed)), ctx.stageModelDetails(routed, true));
  for (const detail of [undefined, null, 0, '']) assert.deepEqual(ctx.stageModelDetails(routed, detail), ctx.stageModelDetails(routed, false));
  for (const detail of [1, 'yes']) assert.deepEqual(ctx.stageModelDetails(routed, detail), ctx.stageModelDetails(routed, true));

  const status = 'Agreed · codex/configured-model · high · unverified · strong (configured)'
    + '\nExecution: codex/configured-model · high · verified'
    + '\nRouting: pending · reassessments 2/3 · operational retries 1/2\nTrigger: pending-trigger';
  const errors = 'routing error\r\nfull\nmodel block\nfull';
  assert.equal(ctx.stageModelStatus(routed), status);
  assert.equal(ctx.stageModelErrors(routed), errors);
  for (const detail of [false, true]) {
    const expected = detail ? detailed : reasons;
    assert.equal(ctx.stageModelText(routed, detail), status + '\n'
      + expected.map(f => f.label + ': ' + f.text).join('\n') + '\n' + errors);
  }
});

test('model partitions preserve missing-value suppression and trigger fallback strings', () => {
  const diagnostics = [
    {label:'Risk',text:'pending · complexity: pending'},
    {label:'Constraint',text:'{}'},
    {label:'Cost',text:'unknown / no comparable billing data used'},
    {label:'Routing price',text:'unavailable / no comparable rate used'},
    {label:'Agreement',text:'pending · policy: pending'},
  ];
  for (const missing of [undefined, null, '']) {
    const sparse = {model_agreement:{planner_reason:missing,architect_reason:missing,
      policy_inputs:{relative_cost_preference:missing === '' ? null : missing}},
      reassessment:{history:[{kind:'empty',planner_reason:missing,architect_reason:missing,evidence:''}]}};
    assert.deepEqual(structuredClone(ctx.stageModelDetails(sparse, false)), []);
    assert.deepEqual(structuredClone(ctx.stageModelRoutingHistory(sparse)), []);
    assert.deepEqual(structuredClone(ctx.stageModelDetails(sparse, true)), diagnostics);
  }
  assert.deepEqual(structuredClone(ctx.stageModelDetails({}, false)), []);
  assert.deepEqual(structuredClone(ctx.stageModelDetails({}, true)), diagnostics);
  assert.equal(ctx.stageModelText({}, false), 'Model agreement pending — reconcile before approval\n');
  for (const [trigger, expected] of [
    [{evidence:'\r\n exact evidence\t '}, '\r\n exact evidence\t '],
    [{evidence:['first','second']}, '["first","second"]'],
    [{evidence:null,error:'failure'}, '"failure"'],
    [{evidence:false,error:{reason:'failure'}}, '{"reason":"failure"}'],
    [{evidence:0}, '""'],
    [{}, '""'],
    [{evidence:'',error:'must not replace empty string'}, null],
  ]) {
    for (const routing of [{pending:trigger}, {history:[{kind:'last',...trigger}]}]) {
      const fields = expected === null ? [] : [{label:'Trigger evidence',text:expected}];
      assert.deepEqual(structuredClone(ctx.stageModelReasons({reassessment:routing})), fields);
    }
  }
  const sparseHistory = {model_agreement:{planner_reason:0,architect_reason:false},reassessment:{history:[
    {kind:'empty',planner_reason:null},
    {kind:'partial',planner_reason:'',architect_reason:'Kept at position 2'},
    {kind:'values',planner_reason:0,architect_reason:false},
  ]}};
  assert.deepEqual(structuredClone(ctx.stageModelReasons(sparseHistory)), [
    {label:'Planner',text:'0'},{label:'Architect',text:'false'},{label:'Trigger evidence',text:'""'},
  ]);
  assert.deepEqual(structuredClone(ctx.stageModelRoutingHistory(sparseHistory)), [
    {label:'partial · Architect 2',text:'Kept at position 2'},
    {label:'values · Planner 3',text:'0'},{label:'values · Architect 3',text:'false'},
  ]);
});

test('tier drafts defer model identity and show the actual binding only after launch', () => {
  const draft = {model_agreement:{version:2, valid:true, effective:null,
    policy_inputs:{tier:'standard'}, validated_proposal:{tier:'standard'},
    planner_reason:'Ordinary implementation', architect_reason:'Bounded failure impact'}};
  const text = ctx.stageModelStatus(draft);
  assert.match(text, /Tier: standard/);
  assert.match(text, /model selected at implementation start/);
  assert.doesNotMatch(text, /undefined|codex|claude|unverified/);
  draft.model_selection = {effective:{provider:'codex',model:'terra-test',native_effort:'provider_default'}};
  assert.match(ctx.stageModelStatus(draft), /Selected: codex\/terra-test/);
  draft.model_selection.effective = {provider:'claude',model:'sonnet-test',native_effort:'provider_default'};
  assert.match(ctx.stageModelStatus(draft), /Selected: claude\/sonnet-test/);
});
