import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const context={};
vm.runInNewContext(readFileSync(new URL('../quickshell/UsageFormat.js',import.meta.url),'utf8'),context);
test('Fable balance is separate from general quota and includes reset',()=>{
  const text=context.quotaSummary({status:'available',windows:[
    {name:'Claude · 5h',used_percent:57},
    {name:'Fable · weekly',used_percent:100,resets_at:'2099-09-13T02:00:00Z'}]});
  assert.match(text,/Claude · 5h: 43% remaining/);
  assert.match(text,/Fable · weekly: 0% remaining · reset/);
});
test('unknown, stale and reset readings cannot imply a fresh balance',()=>{
  assert.match(context.quotaSummary(null),/checking/);
  assert.match(context.quotaSummary({status:'unavailable',error:'offline'}),/unavailable · offline/);
  assert.match(context.quotaSummary({status:'stale',error:'offline',windows:[{name:'Fable',used_percent:null}]}),/remaining unknown[\s\S]*Previous reading · offline/);
  assert.match(context.quotaSummary({status:'available',windows:[{name:'Fable',used_percent:100,resets_unix:1}]}),/awaiting refresh/);
});
test('report durations reuse non-negative integer normalization',()=>{
  assert.equal(context.reportDuration(125.9),'2m 5s');
  for (const value of [-1, NaN, Infinity, '125']) assert.equal(context.reportDuration(value),'—');
});
test('the Overview quota line keeps one entry per window and never implies a fresh balance',()=>{
  assert.equal(context.quotaLine({status:'available',windows:[{name:'Claude · 5h',used_percent:8},
    {name:'Fable · weekly',used_percent:100}]}),'Claude · 5h 92% left · Fable · weekly 0% left');
  assert.match(context.quotaLine(null),/checking/);
  assert.match(context.quotaLine({status:'unavailable',error:'offline'}),/unavailable · offline/);
  assert.equal(context.quotaLine({status:'stale',windows:[{name:'Fable',used_percent:null}]}),'Fable unknown · previous reading');
  assert.equal(context.quotaLine({status:'available',windows:[{name:'Fable',used_percent:1,resets_unix:1}]}),'Fable awaiting refresh');
});
test('the Overview summary shows run progress while busy and the last run while idle',()=>{
  const stages=[{status:'committed'},{status:'committed'},{status:'in_progress'},{status:'pending'},{status:'pending'}];
  const plan={status:'approved',stages,usage:{claude:{total_tokens:1200000}}};
  const running={phase:'running',run_started_unix:1000};
  assert.equal(context.overviewSummary(running,plan,true,1251),'2/5 stages committed · run 4m 11s · claude 1.2M tok');
  assert.equal(context.overviewSummary({phase:'planning',run_started_unix:0},null,true,1251),'');
  assert.equal(context.overviewSummary({phase:'idle'},{...plan,status:'done',usage:null,
    stages:stages.map(s=>({status:'committed'}))},false,0),'Last run · done · 5/5 stages committed');
  assert.equal(context.overviewSummary({phase:'awaiting_approval'},{status:'draft',stages:[{status:'pending'}]},false,0),
    'Plan · draft · 0/1 stages committed');
  assert.equal(context.overviewSummary({phase:'idle'},null,false,0),'');
});
