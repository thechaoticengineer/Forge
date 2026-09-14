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
