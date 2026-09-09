import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const read = n => readFileSync(new URL('../quickshell/'+n, import.meta.url),'utf8');
const fields = {}, preview = {}, panel = read('Panel.qml'), helpers = {};
vm.runInNewContext(read('PanelDetails.js'), fields);
vm.runInNewContext(read('DetailText.js'), preview);
vm.runInNewContext(panel.slice(panel.indexOf('  function stageModelText('),panel.indexOf('  function changeModelConstraint(')), helpers);
vm.runInNewContext(panel.slice(panel.indexOf('  function architectUsageText('),panel.indexOf('  function reportLifecycleText(')), helpers);
vm.runInNewContext(panel.slice(panel.indexOf('  function nonNegativeInt('),panel.indexOf('  property var reportIdentityCache:')), helpers);
vm.runInNewContext(panel.slice(panel.indexOf('  function catalogueProviderText('),panel.indexOf('  function openCatalogue(')), helpers);
const values = ['\n \t\n  first LF  \nlast  \t','\r\n \r\n  first CRLF  \r\nlast\t','\r \rfirst CR\rlast  ',
  ' \r\n\t ','','\n界🙂 é <b>literal</b> & text\nlast  ','x'.repeat(5000)];
const decision = (text,i) => ({id:'d'+i,status:'accepted',summary:text,rationale:text,supersedes:'old'+i,
  alternatives:[{description:text,tradeoffs:text},{description:values[(i+1)%values.length],tradeoffs:text}]});
const architecture = {context_status:'needs_recovery',revision:5,error:values[0],recovery:{reason:values[1]},
  guidance:Object.fromEntries(values.map((text,i)=>[i,{text,valid:i%2===0}])),
  unresolved_risks:values.map((text,i)=>({id:'r'+i,text})),recent_decisions:values.map(decision)};
const cases = {
  architecture:()=>fields.architecture(architecture,{status:'blocked',error:values[2],reason:values[3]},values[4]),
  providers:()=>fields.providers(values.map((text,i)=>({provider:'p'+i,status:'unavailable',model_count:2,cached_stale:true,
    blocker:{message:text},error:{message:text},cache_error:text,description:text}))),
  options:()=>fields.options(values.map((text,i)=>({provider:'p',model:'m'+i,tier:'strong',availability:'unverified',
    effort:'high',policy_revision:5,eligible:false,error:text,description:text})),helpers.catalogueOptionText),
  metadata:()=>fields.metadata(values.map((text,i)=>({provider:'p',model:'m'+i,provenance:text,description:text,verified_unix:100,
    removed:true,conflicts:['discovered'],pricing:{label:text,input:1,output:2,currency:'USD',unit:'per million tokens',basis:'API',as_of:'2026'}})),helpers.catalogueStamp),
  sources:()=>fields.sources(values.map(text=>({url:text,error:text,failures:2,next_attempt_unix:100})),helpers.catalogueStamp),
  reports:()=>fields.report({plan_id:'archived',revision:5,project:'/project',architecture:{summary:values[0],...architecture},
    commits:values.map((text,i)=>({sha:'sha'+i,message:text})),stage_outcomes:values.map((text,i)=>({id:i,title:'Stage '+i,status:'committed',
      review_gate:{status:'error',roles:{architect:'pending',reviewer:'approved'}},review_policy:{rationale:text},
      model_agreement:{valid:true,effective:{provider:'codex',model:'exact',native_effort:'high'},availability:'unverified',planner_reason:text,architect_reason:text}})),
    usage:{codex:{total_tokens:10,calls:1,models:{exact:10}},claude:{total_tokens:12,calls:2}}},helpers)
};
for (const [surface, build] of Object.entries(cases)) test(surface+' retains original fields, collection entries and plain previews',()=>{
  const rows=build(), prose=rows.filter(r=>r.detail);
  for(const value of values) {
    assert.ok(prose.some(r=>r.text===value),surface+' lost '+JSON.stringify(value.slice(0,40)));
    assert.equal(preview.copySource(value),value);
    const expected=value.split(/\r\n|\r|\n/).find(line=>line.trim()) || '';
    assert.equal(preview.preview(value),expected);
  }
  assert.equal(new Set(rows.map(r=>r.key)).size,rows.length,'every entry has distinct identity');
});
test('recovery, errors, model availability and archived role outcomes remain independent of prose',()=>{
  const statuses=Object.values(cases).flatMap(build=>build().filter(r=>!r.detail).map(r=>r.status)).join('\n');
  for(const expected of ['needs_recovery','needs refresh','supersedes old','unavailable','cached/stale','unverified','ineligible',
    'removed','conflict','Source error','Plan archived','Project /project','Stage 0','committed','Recorded aggregate: error',
    'independent: approved','architect: pending','codex/exact']) assert.ok(statuses.includes(expected),expected);
  assert.ok(cases.architecture().some(r=>r.error && r.text===values[0]));
  const report=cases.reports();assert.ok(report.some(r=>r.key==='usage/codex'));assert.ok(report.some(r=>r.key==='usage/claude'));
});
class Model {
  rows=[];writes=[];
  get count(){return this.rows.length}
  get(i){return this.rows[i]}
  insert(i,r){this.rows.splice(i,0,{...r})}
  remove(i){this.rows.splice(i,1)}
  move(a,b,n){this.rows.splice(b,0,...this.rows.splice(a,n))}
  setProperty(i,k,v){this.writes.push([i,k,v]);this.rows[i][k]=v}
}
test('polling and reorder retain field objects, open originals and selection; collapse publishes changed text',()=>{
  const model=new Model(), records=cases.architecture();fields.reconcile(model,records);
  const selected=model.rows.find(r=>r.key==='risk/r0');fields.expand(model,selected.key,true);selected.selection='selected';
  model.writes=[];fields.reconcile(model,JSON.parse(JSON.stringify(records)));assert.equal(model.writes.length,0);
  const moved=records.slice().reverse().map(r=>r.key===selected.key ? {...r,text:'new\r\nbody'}:r);
  fields.reconcile(model,moved);assert.equal(model.rows.find(r=>r.key===selected.key),selected);
  assert.equal(selected.selection,'selected');assert.equal(selected.text,values[0]);assert.equal(selected.pendingText,'new\r\nbody');
  fields.expand(model,selected.key,false);assert.equal(selected.text,'new\r\nbody');
  fields.reconcile(model,moved.filter(r=>r.key!==selected.key));assert.ok(!model.rows.includes(selected));
});
test('report identities are cached once per update, distinct for duplicates and stable across prepend/reorder',()=>{
  const cache={nextId:0,byRecord:new Map()};
  const reports=[{unix:1,goal:'same'},{unix:1,goal:'same'},{unix:1,goal:'same',commits:[{sha:'a'}]}];
  const before=fields.indexReports(reports,cache).keys;
  assert.equal(new Set(before).size,3);
  const updated=fields.indexReports([{unix:2,goal:'new'},...JSON.parse(JSON.stringify(reports))],cache);
  assert.deepEqual(updated.keys.slice(1),before);
  const reordered=fields.indexReports([reports[2],reports[0],reports[1]],cache);
  assert.deepEqual(Array.from(reordered.keys),[before[2],before[0],before[1]]);
  assert.equal(cache.byRecord.size,2,'absent originals are released');
});
test('100-report navigation uses short cached keys without serializing full records',()=>{
  let serializations=0;
  const ctx={JSON:{...JSON,stringify(value){serializations++;return JSON.stringify(value)}}};
  vm.runInNewContext(read('PanelDetails.js'),ctx);
  const reports=Array.from({length:100},(_,unix)=>({unix,goal:values[6],commits:[{message:values[0]}]}));
  ctx.reportIndex=ctx.indexReports(reports,{nextId:0,byRecord:new Map()});
  assert.equal(serializations,100);
  vm.runInNewContext(panel.slice(panel.indexOf('  function reportKey('),panel.indexOf('  property var reviewViews:')),ctx);
  for(let n=0;n<1000;n++) {
    const index=n%100; ctx.selectedReportKey=ctx.reportKey(reports[index],index);
    assert.equal(ctx.selectedReportIndex(),index);
    assert.ok(ctx.selectedReportKey.length<20);
  }
  assert.equal(serializations,100,'navigation never traverses or serializes reports');
  assert.match(panel,/active: reportRow.expanded \|\| reportRow.detailsLoaded/);
});
test('remaining Panel surfaces use complete sources while inputs and structured diff retain editing/viewing behavior',()=>{
  for(const name of ['architectureDetails','providerDetails','catalogueOptions','catalogueMetadata','catalogueSources','chatDetails','reportGoal','reportDetails'])
    assert.ok(panel.includes('objectName: "'+name+'"'),name);
  assert.match(panel,/originalText: liveEntries.count > 0 \? liveEntries.get\(liveEntries.count - 1\).originalText/);
  assert.ok(!panel.includes('originalText: root.agentActive ? root.agent.last_line'));
  for(const id of ['goalField','catalogueEditor','questionField','feedbackField']) assert.ok(panel.includes('id: '+id));
  assert.match(panel,/model: root.diffText === "" \? \[\] : root.diffText.split\("\\n"\)/);
  const report=panel.slice(panel.indexOf('id: reportRow'),panel.indexOf('id: keyboardHint'));
  assert.ok(!report.includes('TapHandler'));assert.match(report,/objectName: "reportToggle"/);
});

test('an omitted inspected field survives polling until explicit collapse',()=>{
  const model=new Model();fields.reconcile(model,[fields.field('error','Error',values[0],true)]);
  const row=model.get(0);fields.expand(model,'error',true);row.selection='selected';
  fields.reconcile(model,[]);assert.equal(model.get(0),row);assert.equal(row.selection,'selected');assert.equal(row.retired,true);
  assert.equal(row.text,values[0]);fields.expand(model,'error',false);assert.equal(model.count,0);
});
