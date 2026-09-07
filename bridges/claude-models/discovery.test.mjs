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
