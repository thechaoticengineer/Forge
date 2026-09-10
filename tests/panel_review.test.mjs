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
  assert.doesNotMatch(text,/Architect: approved/); assert.doesNotMatch(text,/Prose only/);
  assert.match(qml, /originalText: stageRow.prose \? stageRow.prose.policyRationale/);
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

test('deferred roles retain their own outcome alongside a stage approval', () => {
  for (const role of ['architect', 'reviewer']) {
    const roles = {architect:'approved', reviewer:'approved', [role]:'deferred'};
    const text = context.reviewGateText({review_gate:{status:'approved',roles}});
    const label = role === 'architect' ? 'Architect' : 'Independent';
    assert.match(text,new RegExp(label+': deferred to the plan review'));
    assert.doesNotMatch(text,new RegExp(label+': (approved|pending|review not required)'));
  }
});
test('deferred gates distinguish waiting for commit from a committed deferred policy', () => {
  const stage = {review_policy:{scope:'code_or_contract'}, review_gate:{status:'deferred',
    roles:{architect:'deferred',reviewer:'deferred'}},last_verdict:{approved:true}};
  assert.match(context.reviewGateText(stage),/gate: deferred · awaiting commit under a deferred review policy/);
  const committed = context.reviewGateText({...stage,status:'committed',sha:'abc123'});
  assert.match(committed,/gate: deferred · committed under a deferred review policy/);
  for (const text of [committed,context.reviewGateText(stage)])
    assert.doesNotMatch(text,/approved|pending|review not required/);
});

const planHelpers = {};
vm.runInNewContext(qml.slice(qml.indexOf('  function loadPlanReviewRequests('),qml.indexOf('  property bool chooserOpen:')),planHelpers);
const detailText = {};
vm.runInNewContext(readFileSync(new URL('../quickshell/DetailText.js',import.meta.url),'utf8'),detailText);
const history = {};
vm.runInNewContext(readFileSync(new URL('../quickshell/PlanReview.js',import.meta.url),'utf8'),history);
function assertPreview(view, plan) {
  assert.equal(view.complete,false);
  assert.equal(view.text,plan.plan_review.gate.requests.map(r=>`[${r.role}] ${r.text}`).join('\n\n'));
  assert.equal(detailText.preview(view.text),'[architect] request 0');
}
function planFixture() {
  const identity = {plan_id:'plan',revision:2,scope:'plan',stage_id:null,attempt_id:'attempt',round:2,
    snapshot:{head:'head',tree:'tree',content:'content'},policy:{required_roles:['architect','reviewer']}};
  const originals = Array.from({length:12},(_,i)=>`request ${i}\r\n  界🙂 <b>plain</b> & é\n${'long finding '.repeat(100)}\nlast  \t`);
  const records = ['architect','reviewer'].map(role=>({identity:{...identity,role},role,
    approved:false,issues:originals,notes:[originals[0],'legacy note']}));
  const plan = {plan_id:'plan',revision:2,plan_review:{attempt_id:'attempt',status:'exhausted',rounds:2,budget:1,
    fix_sha:'abc123',required_roles:['architect','reviewer'],reviews:[{truncated:true,issues:[originals[0].slice(0,100)]}],
    gate:{identity,status:'exhausted',roles:{architect:'changes_requested',reviewer:'changes_requested'},
      requests_truncated:true,requests:originals.slice(0,8).map(text=>({role:'architect',text:text.slice(0,240)}))}}};
  const event = record => ({id:'event-'+record.role,plan_id:'plan',payload:{kind:'plan_review',reviews:[record]}});
  return {plan,records,originals,event};
}
test('plan review status, B+1 rounds, outcomes and fix commit are independent of stage gates',()=>{
  const {plan} = planFixture();
  const text = planHelpers.planReviewStatusText(plan.plan_review);
  for (const value of ['Plan review: exhausted','round 2 of 2','Current gate: exhausted',
    'Architect: changes_requested','Independent: changes_requested','Fix commit: abc123']) assert.ok(text.includes(value),value);
  assert.match(planHelpers.planReviewStatusText({status:'pending',rounds:0,budget:0}),/round 0 of 1/);
  assert.match(planHelpers.planReviewStatusText({}),/round — of —/);
  assert.equal(planHelpers.planReviewStatusText(null),'');
  assert.match(planHelpers.planReviewStatusText({gate:{roles:{architect:'not_required',reviewer:'deferred'}}}),
    /Architect: review not required · Independent: deferred to the plan review/);
  const section = qml.slice(qml.indexOf('        Column {\n          id: planReviewSection'),qml.indexOf('// ------------------------------------------------ plan Q&A'));
  assert.match(section,/root.planReviewStatusText\(root.planReview\)/);
  assert.match(section,/CompactDetail/);
  assert.match(section,/originalText: planReviewSection.view.text \|\| ""/);
  assert.match(section,/Shortened preview · full change requests have not been loaded/);
  assert.match(section,/textComplete: planReviewSection.view.complete === true/);
  assert.match(section,/onCopyRequested: original => Quickshell.clipboardText = original/);
  assert.ok(qml.indexOf('id: planReviewSection') < qml.indexOf('id: stageList'));
});
test('panel disclosure retrieves all large role-tagged requests through bounded history pages',()=>{
  const {plan,records,originals,event} = planFixture();
  const view = history.create('/project with space',plan);
  assertPreview(view,plan);
  const calls=[], stale = {...records[0],identity:{...records[0].identity,attempt_id:'old-attempt'}};
  const pages = [
    {plan_id:'plan',event_end:300,next_cursor:100,items:[event(stale)]},
    {plan_id:'plan',event_end:300,next_cursor:200,items:[event(records[1])]},
    {plan_id:'plan',event_end:300,next_cursor:null,items:[event(records[0])]}
  ];
  const ctx = {PlanReview:history,planReviewView:view,planReviewScope:history.scope('/project with space',1,plan),planReviewVersion:0,
    api(method,path,body,done,scoped){
      calls.push(path);assert.equal(method,'GET');assert.equal(body,null);assert.equal(scoped,true);
      assertPreview(view,plan); // Partial history keeps the bounded preview incomplete.
      done(pages.shift(),200);
    }};
  ctx.root=ctx;
  vm.runInNewContext(qml.slice(qml.indexOf('  function loadPlanReviewRequests('),qml.indexOf('  function reviewCadenceLabel(')),ctx);
  ctx.loadPlanReviewRequests();
  assert.equal(calls.length,3);assert.equal(view.complete,true);assert.equal(view.pending,false);
  calls.forEach((path,i)=>assert.equal(path,`/api/architecture/history?project=%2Fproject%20with%20space&plan_id=plan&cursor=${i*100}&limit=100`));
  const expected=['architect','reviewer'].flatMap(role=>[...originals,'legacy note'].map(text=>`[${role}] ${text}`)).join('\n\n');
  assert.equal(view.text,expected);
  assert.ok(view.text.length>24000,'fixture exceeds both text and collection preview bounds');
  ctx.loadPlanReviewRequests();assert.equal(calls.length,3,'completed disclosure does not poll history');
});
test('history does not substitute other attempts, rounds, roles, stage scopes or snapshots',()=>{
  const {plan,records,event}=planFixture(), view=history.create('/p',plan);
  const bad = [{attempt_id:'old'},{round:1},{stage_id:1},{scope:'stage'},{role:'fixer'},
    {snapshot:{head:'other'}},{revision:1},{plan_id:'other'},{policy:{required_roles:['architect']}}]
    .map(patch=>event({...records[0],identity:{...records[0].identity,...patch}}));
  assert.equal(history.begin(view),true);
  assert.equal(history.finish(view,{plan_id:'plan',event_end:100,next_cursor:null,items:bad},200),false);
  assertPreview(view,plan);assert.match(view.error,/not found/);
  assert.equal(history.begin(view),true);
  history.finish(view,{error:'offline'},503);assert.equal(view.pending,false);assert.equal(view.error,'offline');assertPreview(view,plan);
  const truncated=history.create('/p',{...plan,plan_review:{...plan.plan_review,gate:{...plan.plan_review.gate,identity_truncated:true}}});
  assert.equal(history.begin(truncated),false);assert.match(truncated.error,/no complete review identity/);
});
test('late history responses cannot cross project visits or gate changes',()=>{
  const {plan,records,event}=planFixture(), view=history.create('/p',plan);
  let reply;
  const ctx={PlanReview:history,planReviewView:view,planReviewScope:'old',planReviewVersion:0,api(m,p,b,done){reply=done}};
  ctx.root=ctx;
  vm.runInNewContext(qml.slice(qml.indexOf('  function loadPlanReviewRequests('),qml.indexOf('  function reviewCadenceLabel(')),ctx);
  ctx.loadPlanReviewRequests();ctx.planReviewScope='new';ctx.planReviewView=history.create('/other',plan);
  reply({plan_id:'plan',event_end:100,next_cursor:null,items:records.map(event)},200);
  assertPreview(ctx.planReviewView,plan);
  const key=history.scope('/p',1,plan);
  assert.equal(key,history.scope('/p',1,structuredClone(plan)));
  assert.notEqual(key,history.scope('/p',2,plan));
});
test('complete small gates need no history and manual retry restarts a failed bounded scan',()=>{
  const {plan,records,event}=planFixture();
  const small=history.create('/p',{...plan,plan_review:{gate:{requests:[{role:'reviewer',text:'full\r\ntext  '}]}}});
  assert.equal(small.text,'[reviewer] full\r\ntext  ');assert.equal(small.complete,true);assert.equal(history.begin(small),false);
  const view=history.create('/p',plan);
  assert.equal(history.begin(view),true);
  assert.equal(history.finish(view,{plan_id:'plan',event_end:200,next_cursor:100,items:[event(records[0])]},200),true);
  assert.equal(view.cursor,100);assert.equal(history.begin(view),true);
  history.finish(view,null,503);assertPreview(view,plan);
  assert.equal(history.begin(view),true);assert.equal(view.cursor,0);assert.equal(view.end,null);
  history.finish(view,{plan_id:'plan',event_end:200,next_cursor:null,items:records.map(event)},200);
  assert.equal(view.complete,true);
});
