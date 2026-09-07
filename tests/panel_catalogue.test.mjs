import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const qml = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
// Exercise the actual panel formatting helpers without a desktop or live engine.
const start = qml.indexOf('  function catalogueProviderText(');
const end = qml.indexOf('  function openCatalogue()', start);
const context = {};
vm.runInNewContext(qml.slice(start, end), context);
test('provider states show unavailable evidence and stale/cache errors', () => {
  for (const status of ['pending', 'discovered', 'cached_stale', 'unsupported_discovery', 'unavailable']) {
    assert.match(context.catalogueProviderText({provider:'claude',status,model_count:0}), new RegExp(status));
  }
  const text = context.catalogueProviderText({provider:'codex',status:'unavailable',model_count:2,
    cached_stale:true,blocker:{message:'authentication failed'},error:{message:'timeout'},cache_error:'corrupt cache'});
  assert.match(text,/cached\/stale/); assert.match(text,/authentication failed/); assert.match(text,/corrupt cache/);
  assert.doesNotMatch(text,/timeout/); // Stronger evidence stays visible.
});
test('configured options disclose unverified availability, revision and errors', () => {
  const text = context.catalogueOptionText({provider:'codex',model:'test-model',tier:'strong',availability:'configured_unverified',
    effort:'provider_default',policy_revision:'policy-2',error:'unsupported effort'});
  assert.match(text,/configured_unverified/); assert.match(text,/configured revision policy-2/); assert.match(text,/unsupported effort/);
  assert.doesNotMatch(text,/\$/);
});

test('model settings load full policy and submit JSON; invalid edits stay local', () => {
  const policy = {policy_revision:'2',entries:[{provider:'codex',model:'test-model',tier:'basic',relative_cost_preference:1}]};
  const calls = [];
  const root = {};
  const sandbox = {root, catalogueEditor:{text:'{'},
    api(method,path,body,done) { calls.push([method,path,body]); done({policy,options:[]},200); },
    act(path,body,done) { calls.push(['POST',path,body]); done({},200); },
  };
  const code = qml.slice(qml.indexOf('  function openCatalogue()'), qml.indexOf('  property bool helpOpen:'));
  vm.runInNewContext(code, sandbox);
  root.openCatalogue = sandbox.openCatalogue;
  sandbox.openCatalogue();
  assert.equal(root.catalogueOpen,true);
  assert.deepEqual(JSON.parse(root.catalogueDraft),policy);
  sandbox.saveCatalogue();
  assert.match(root.localError,/valid JSON/);
  assert.equal(calls.length,1);
  sandbox.catalogueEditor.text=JSON.stringify(policy);
  sandbox.saveCatalogue();
  assert.equal(calls[1][1],'/api/settings');
  assert.deepEqual(JSON.parse(JSON.stringify(calls[1][2])),{model_catalogue:policy});
});
