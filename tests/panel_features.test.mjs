import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const qml = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
const featuresJsSource = readFileSync(new URL('../quickshell/Features.js', import.meta.url), 'utf8');

const plain = value => JSON.parse(JSON.stringify(value));

function loadFeatures() {
  const module = {};
  vm.runInNewContext(featuresJsSource, module);
  return module;
}

// Extracts a top-level `function <name>(...) { ... }` from Panel.qml by
// brace-matching, so a test fails with a clear assertion message (not a
// crash, and not silently matching the wrong snippet) when the function
// does not exist yet.
function extractFunction(source, name) {
  const marker = `function ${name}(`;
  const start = source.indexOf(marker);
  assert.ok(start >= 0, `Panel.qml must define function ${name}(); it does not exist yet`);
  const braceStart = source.indexOf('{', start);
  assert.ok(braceStart >= 0, `Panel.qml's ${name} has no function body`);
  let depth = 0;
  let end = -1;
  for (let i = braceStart; i < source.length; i++) {
    if (source[i] === '{') depth++;
    else if (source[i] === '}') {
      depth--;
      if (depth === 0) { end = i + 1; break; }
    }
  }
  assert.ok(end >= 0, `Panel.qml's ${name} function body is never closed`);
  return source.slice(start, end);
}

function fixture() {
  const ctx = {
    lastProject: '/p',
    projectViewRevision: 1,
    featuresOpen: false,
    featureList: [],
    calls: [],
  };
  ctx.Features = loadFeatures();
  ctx.api = (method, path, body, done) => ctx.calls.push({ method, path, body, done });
  ctx.root = ctx;
  return ctx;
}

test('S7: panel lists discovered features with their validation status - featureRows shape', () => {
  // S7: The panel lists discovered features with their status
  const Features = loadFeatures();
  const response = {
    project: '/p',
    features: [
      { slug: 'alpha', title: 'Alpha', path: '/p/docs/features/alpha', status: 'valid', reasons: [] },
      { slug: 'beta', title: 'Beta', path: '/p/docs/features/beta', status: 'invalid',
        reasons: ['missing required file: scenarios.md'] },
    ],
  };
  const rows = plain(Features.featureRows(response));
  assert.deepEqual(rows, [
    { slug: 'alpha', title: 'Alpha', path: '/p/docs/features/alpha', status: 'valid',
      statusLabel: 'valid', valid: true, reasons: [] },
    { slug: 'beta', title: 'Beta', path: '/p/docs/features/beta', status: 'invalid',
      statusLabel: 'invalid: missing required file: scenarios.md', valid: false,
      reasons: ['missing required file: scenarios.md'] },
  ]);
  assert.deepEqual(plain(Features.featureRows({ features: [] })), []);
  assert.deepEqual(plain(Features.featureRows(null)), []);
  assert.deepEqual(plain(Features.featureRows(undefined)), []);
});

test('S7: openFeatures fetches /api/features for the current project and shows the list', () => {
  // S7: The panel lists discovered features with their status
  const src = extractFunction(qml, 'openFeatures');
  const ctx = fixture();
  vm.runInNewContext(src, ctx);

  ctx.openFeatures();
  assert.equal(ctx.calls.length, 1, 'openFeatures must call api() exactly once');
  assert.equal(ctx.calls[0].method, 'GET');
  assert.equal(ctx.calls[0].path, '/api/features?project=' + encodeURIComponent(ctx.lastProject));

  const response = {
    project: '/p',
    features: [{ slug: 'alpha', title: 'Alpha', path: '/p/docs/features/alpha', status: 'valid', reasons: [] }],
  };
  ctx.calls[0].done(response, 200);
  assert.equal(ctx.featuresOpen, true);
  assert.deepEqual(plain(ctx.featureList), plain(ctx.Features.featureRows(response)));

  // A response that arrives after the project view moved on must be ignored.
  ctx.featuresOpen = false;
  ctx.featureList = [];
  ctx.openFeatures();
  const staleRequest = ctx.calls[1];
  ctx.projectViewRevision++;
  staleRequest.done(response, 200);
  assert.equal(ctx.featuresOpen, false, 'a stale response must not reopen the feature list');
  assert.deepEqual(ctx.featureList, [], 'a stale response must not populate the feature list');
});

test('S8: open in nvim launches omarchy-launch-editor on the feature folder - editorCommand', () => {
  // S8: Opening a feature folder in nvim from the panel
  const Features = loadFeatures();
  assert.deepEqual(plain(Features.editorCommand({ path: '/p/docs/features/x' })),
    ['omarchy-launch-editor', '/p/docs/features/x']);
});

test('S8: openFeatureInEditor runs the editor command via Quickshell.execDetached', () => {
  // S8: Opening a feature folder in nvim from the panel
  const src = extractFunction(qml, 'openFeatureInEditor');
  const ctx = fixture();
  let executed = null;
  ctx.Quickshell = { execDetached(args) { executed = args; } };
  vm.runInNewContext(src, ctx);

  const feature = { slug: 'x', path: '/p/docs/features/x' };
  ctx.openFeatureInEditor(feature);
  assert.deepEqual(executed, ctx.Features.editorCommand(feature));
  assert.deepEqual(plain(executed), ['omarchy-launch-editor', '/p/docs/features/x']);
});
