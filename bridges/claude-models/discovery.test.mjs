import test from 'node:test';
import assert from 'node:assert/strict';
import { discover } from './discovery.mjs';
for (const fail of [false, true]) {
  test(`initialization only; cleanup on ${fail ? 'failure' : 'success'}`, async () => {
    let closed = false, input;
    const rows = [{ value: 'alias', resolvedModel: 'wire-id', supportedEffortLevels: ['high'] }];
    const query = ({ prompt, options }) => {
      assert.equal(options.pathToClaudeCodeExecutable, 'claude');
      assert.equal(options.persistSession, false);
      assert.deepEqual(options.tools, []);
      input = prompt[Symbol.asyncIterator]().next();
      return {
        async supportedModels() {
          assert.equal(await Promise.race([input.then(() => 'sent'), Promise.resolve('empty')]), 'empty');
          if (fail) throw new Error('authentication failed');
          return rows;
        },
        close() { closed = true; },
      };
    };
    if (fail) await assert.rejects(discover(query, 'claude'), /authentication failed/);
    else assert.deepEqual(await discover(query, 'claude'), rows);
    assert.equal(closed, true);
    assert.deepEqual(await input, { value: undefined, done: true });
  });
}

for (const fail of [false, true]) {
  test(`usage control request never generates; cleanup on ${fail ? 'failure' : 'success'}`, async () => {
    let closed = false, input;
    const query = ({prompt}) => {
      input = prompt[Symbol.asyncIterator]().next();
      return {
        async usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET(options) {
          assert.deepEqual(options, {skipBehaviors:true});
          assert.equal(await Promise.race([input.then(() => 'sent'), Promise.resolve('empty')]), 'empty');
          if (fail) throw new Error('offline');
          return {rate_limits_available:true, session:{private:'never forward'}, rate_limits:{
            five_hour:{utilization:57,resets_at:null},
            model_scoped:[{display_name:'Fable',utilization:100,resets_at:'2026-09-13T02:00:00.035658+00:00'}],
            extra_usage:{is_enabled:false,used_credits:999}, private:'never forward'}};
        }, close() {closed = true;}
      };
    };
    if (fail) await assert.rejects(discover(query,'claude','usage'), /offline/);
    else {
      const result = await discover(query,'claude','usage');
      assert.equal(result.windows[1].used_percent,100);
      assert.equal(result.windows[1].resets_unix,1789264800);
      assert.equal(result.extra_usage_enabled,false);
      assert.doesNotMatch(JSON.stringify(result),/private|used_credits|session/);
    }
    assert.equal(closed,true);
    assert.deepEqual(await input,{value:undefined,done:true});
  });
}

test('missing and malformed provider numbers remain unknown', async () => {
  const {normalizeUsage} = await import('./discovery.mjs');
  assert.deepEqual(normalizeUsage({rate_limits_available:false,rate_limits:null}).windows,[]);
  const result = normalizeUsage({rate_limits_available:true,rate_limits:{model_scoped:[
    {display_name:'Fable',utilization:null,resets_at:null},
    {display_name:'Opus',utilization:101,resets_at:'invalid'},
    {display_name:'Sonnet',utilization:'0',resets_at:null}]}});
  assert.ok(result.windows.every(w => w.used_percent === null && w.resets_unix === null));
});
