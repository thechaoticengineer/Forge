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

test('incompatible usage schemas fail instead of inventing an empty balance', async () => {
  const {normalizeUsage} = await import('./discovery.mjs');
  for (const response of [null, [], {}, {rate_limits_available:'yes'},
    {rate_limits_available:true,rate_limits:null},
    {rate_limits_available:true,rate_limits:[]},
    {rate_limits_available:true,rate_limits:{model_scoped:{}}}]) {
    assert.throws(() => normalizeUsage(response), /unsupported/);
  }
});

test('bridge probes compatible CLI upgrades without a version allowlist or generation', async () => {
  const {mkdtemp, mkdir, copyFile, writeFile, rm} = await import('node:fs/promises');
  const {tmpdir} = await import('node:os');
  const {join} = await import('node:path');
  const {execFile} = await import('node:child_process');
  const {promisify} = await import('node:util');
  const run = promisify(execFile);
  const env = {...process.env};
  delete env.NODE_TEST_CONTEXT;
  const root = await mkdtemp(join(tmpdir(), 'forge-bridge-protocol-'));
  try {
    for (const file of ['bridge.mjs','discovery.mjs'])
      await copyFile(new URL(file, import.meta.url),join(root,file));
    const sdk = join(root,'node_modules','@anthropic-ai','claude-agent-sdk');
    await mkdir(sdk,{recursive:true});
    await writeFile(join(sdk,'package.json'),JSON.stringify({type:'module',version:'0.3.261',exports:'./index.mjs'}));
    await writeFile(join(sdk,'index.mjs'), `
      import assert from 'node:assert/strict';
      export function query({prompt,options}) {
        assert.equal(options.pathToClaudeCodeExecutable,'claude');
        assert.equal(options.persistSession,false);
        assert.deepEqual(options.tools,[]);
        assert.deepEqual(options.mcpServers,{});
        assert.equal(options.settings.disableAllHooks,true);
        const input = prompt[Symbol.asyncIterator]().next();
        async function noPrompt() {
          assert.equal(await Promise.race([input.then(()=>'sent'),Promise.resolve('empty')]),'empty');
        }
        return {
          async supportedModels() { await noPrompt(); return [{value:'opus[1m]',resolvedModel:'claude-opus-5[1m]'}]; },
          async usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET(options) {
            await noPrompt(); assert.deepEqual(options,{skipBehaviors:true});
            return {rate_limits_available:true,rate_limits:{model_scoped:[{display_name:'Fable',utilization:100,resets_at:'2099-01-01T00:00:00Z'}],extra_usage:{is_enabled:false}}};
          },
          close() {}
        };
      }
    `);
    // Future version labels are fixtures, not claims that those releases exist.
    for (const version of ['2.1.263','2.1.265','2.1.999','3.0.0']) {
      const cli = `${version} (Claude Code)`;
      for (const protocol of ['--forge-discovery-v1','--forge-usage-v1']) {
        const {stdout,stderr} = await run(process.execPath,[join(root,'bridge.mjs'),protocol,'claude',cli],{timeout:5000,env});
        assert.ok(stdout, `Bridge returned no envelope: ${cli} ${protocol}: ${stderr}`);
        const result = JSON.parse(stdout);
        assert.equal(result.cli_version,cli);
        assert.equal(result.error,undefined);
        if (protocol === '--forge-usage-v1') {
          assert.equal(result.usage.windows[0].used_percent,100);
          assert.equal(result.usage.extra_usage_enabled,false);
        } else assert.equal(result.models[0].value,'opus[1m]');
      }
    }
    for (const args of [['--unknown','claude','3.0.0 (Claude Code)'],['--forge-usage-v1','other','3.0.0 (Claude Code)'],['--forge-usage-v1','claude','']]) {
      await assert.rejects(run(process.execPath,[join(root,'bridge.mjs'),...args],{timeout:5000,env}), error => {
        const result = JSON.parse(error.stdout);
        return result.error.code === -32601 && result.usage === undefined;
      });
    }
  } finally { await rm(root,{recursive:true,force:true}); }
});
