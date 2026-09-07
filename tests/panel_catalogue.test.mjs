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

test('metadata rows show provenance, freshness, labelled API rates and unknown pricing', () => {
  const priced = context.catalogueMetadataText({provider:'codex',model:'m',provenance:'official',verified_unix:1788220800,
    pricing:{label:'api_list_rate',input:1.25,output:10,currency:'USD',unit:'per_million_tokens',basis:'api_list_rate',as_of:'2026-09-01'},
    conflicts:[{field:'supports_reasoning'}]});
  assert.match(priced,/official/); assert.match(priced,/verified 2026-09-01/);
  assert.match(priced,/api_list_rate 1\.25\/10 USD per_million_tokens/);
  assert.match(priced,/basis api_list_rate/); assert.match(priced,/as of 2026-09-01/);
  assert.match(priced,/discovered native support wins/);
  assert.doesNotMatch(priced,/subscription/);
  const unknown = context.catalogueMetadataText({provider:'claude',model:'n',provenance:'official',
    verified_unix:1788220800,pricing:null,removed:true,conflicts:[]});
  assert.match(unknown,/pricing unknown/); assert.match(unknown,/removed \(retained for audit\)/);
  assert.doesNotMatch(unknown,/\$/);
});
test('metadata summary and sources surface retry/error state and quiet freshness', () => {
  assert.equal(context.catalogueMetadataSummaryText(null),'Official metadata pending');
  const quiet = context.catalogueMetadataSummaryText({records:2,unknown_pricing:1,negative:0,
    source_errors:0,refreshing:false,last_refresh_unix:1788220800,last_requests:0});
  assert.match(quiet,/2 records/); assert.match(quiet,/1 unknown pricing/); assert.match(quiet,/0 requests/);
  const bad = context.catalogueMetadataSummaryText({records:2,unknown_pricing:2,negative:1,
    source_errors:1,refreshing:false,last_refresh_unix:1788220800,last_requests:1,store_error:'metadata store is corrupt'});
  assert.match(bad,/1 source errors/); assert.match(bad,/metadata store is corrupt/);
  const source = context.catalogueMetadataSourceText({url:'https://developers.openai.com/codex/models.json',
    error:'http status 503',failures:2,next_attempt_unix:1788220800});
  assert.match(source,/error: http status 503/); assert.match(source,/failures 2/); assert.match(source,/next attempt 2026-09-01/);
  assert.match(context.catalogueMetadataSourceText({url:'u',checked_unix:1788220800}),/ok · checked 2026-09-01/);
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
