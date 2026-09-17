import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

// Business tests for milestone M2 (spec phase) panel scenarios S10-S19.
// M2 is not implemented yet: every helper and Panel.qml function referenced
// here does not exist. Tests fail with a clear assertion message naming the
// missing function or helper, following the pattern in panel_features.test.mjs.

const qml = readFileSync(new URL('../quickshell/Panel.qml', import.meta.url), 'utf8');
const featuresJsSource = readFileSync(new URL('../quickshell/Features.js', import.meta.url), 'utf8');

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
    featuresError: '',
    calls: [],
  };
  ctx.Features = loadFeatures();
  ctx.api = (method, path, body, done) => ctx.calls.push({ method, path, body, done });
  ctx.root = ctx;
  return ctx;
}

function expectHelper(Features, name, signature) {
  assert.equal(typeof Features[name], 'function', `Features.js must define ${name}(${signature})`);
}

test('S10: createFeature POSTs /api/features/create with project, slug and title', () => {
  // S10: A new feature is created from the template
  const src = extractFunction(qml, 'createFeature');
  const ctx = fixture();
  vm.runInNewContext(src, ctx);

  ctx.createFeature('new-feature', 'New Feature');
  assert.ok(ctx.calls.length >= 1, 'createFeature must call api()');
  const create = ctx.calls[0];
  assert.equal(create.method, 'POST');
  assert.equal(create.path, '/api/features/create');
  assert.deepEqual(create.body, { project: ctx.lastProject, slug: 'new-feature', title: 'New Feature' });
});

test('S10: createFeature refreshes the feature list on success', () => {
  // S10: A new feature is created from the template
  const src = extractFunction(qml, 'createFeature');
  const ctx = fixture();
  vm.runInNewContext(src, ctx);

  ctx.createFeature('new-feature', 'New Feature');
  const create = ctx.calls[0];
  create.done({ ok: true, slug: 'new-feature' }, 200);

  const refreshed = ctx.calls.length > 1
    && ctx.calls[1].method === 'GET'
    && ctx.calls[1].path.startsWith('/api/features');
  assert.ok(refreshed, 'createFeature must refresh the feature list (an openFeatures-style GET) on success');
});

test('S10: createFeature sets featuresError from resp.error on failure', () => {
  // S10: A new feature is created from the template
  const src = extractFunction(qml, 'createFeature');
  const ctx = fixture();
  vm.runInNewContext(src, ctx);

  ctx.createFeature('bad slug', 'Bad');
  const create = ctx.calls[0];
  create.done({ error: 'invalid slug' }, 400);
  assert.equal(ctx.featuresError, 'invalid slug');
});

test('S10: Features.specStatusLabel falls back to draft', () => {
  // S10: A new feature is created from the template
  const Features = loadFeatures();
  expectHelper(Features, 'specStatusLabel', 'feature');
  assert.equal(Features.specStatusLabel({ spec_status: 'spec approved' }), 'spec approved');
  assert.equal(Features.specStatusLabel({}), 'draft');
  assert.equal(Features.specStatusLabel({ spec_status: null }), 'draft');
});

test('S10: Features.validateNewFeature validates slug and title', () => {
  // S10: A new feature is created from the template
  const Features = loadFeatures();
  expectHelper(Features, 'validateNewFeature', 'slug, title');
  assert.equal(Features.validateNewFeature('valid-slug', 'A Title'), '');
  assert.notEqual(Features.validateNewFeature('Not Valid', 'A Title'), '');
  assert.notEqual(Features.validateNewFeature('valid-slug', ''), '');
  assert.notEqual(Features.validateNewFeature('valid-slug', '   '), '');
});

test('S11: sendFeatureChat POSTs /api/features/chat with project, slug and message', () => {
  // S11: The co-authoring agent edits only its feature folder
  const src = extractFunction(qml, 'sendFeatureChat');
  const ctx = fixture();
  vm.runInNewContext(src, ctx);

  ctx.sendFeatureChat({ slug: 'draft-feature' }, 'please add a section');
  assert.equal(ctx.calls.length, 1, 'sendFeatureChat must call api() exactly once');
  assert.equal(ctx.calls[0].method, 'POST');
  assert.equal(ctx.calls[0].path, '/api/features/chat');
  assert.deepEqual(ctx.calls[0].body, { project: ctx.lastProject, slug: 'draft-feature', message: 'please add a section' });
});

test('S13: requestFeatureReview POSTs /api/features/review with project and slug', () => {
  // S13: The architect reviews the spec
  const src = extractFunction(qml, 'requestFeatureReview');
  const ctx = fixture();
  vm.runInNewContext(src, ctx);

  ctx.requestFeatureReview({ slug: 'reviewed-feature' });
  assert.equal(ctx.calls.length, 1, 'requestFeatureReview must call api() exactly once');
  assert.equal(ctx.calls[0].method, 'POST');
  assert.equal(ctx.calls[0].path, '/api/features/review');
  assert.deepEqual(ctx.calls[0].body, { project: ctx.lastProject, slug: 'reviewed-feature' });
});

test('S13: Features.reviewSummary summarizes the latest review', () => {
  // S13: The architect reviews the spec
  const Features = loadFeatures();
  expectHelper(Features, 'reviewSummary', 'latest_review');
  assert.equal(Features.reviewSummary(null), null);
  const approved = Features.reviewSummary({ approved: true, summary: 'Looks fine.', issues: [], questions: ['why?'] });
  assert.equal(approved.label, 'approved');
  assert.equal(approved.summary, 'Looks fine.');
  assert.deepEqual(approved.issues, []);
  assert.deepEqual(approved.questions, ['why?']);
  const rejected = Features.reviewSummary({ approved: false, summary: 'Needs work.', issues: ['x'], questions: [] });
  assert.equal(rejected.label, 'changes requested');
});

test('S14: a refused review surfaces resp.error in featuresError', () => {
  // S14: An invalid feature cannot be sent to spec review
  const src = extractFunction(qml, 'requestFeatureReview');
  const ctx = fixture();
  vm.runInNewContext(src, ctx);

  ctx.requestFeatureReview({ slug: 'invalid-feature' });
  assert.equal(ctx.calls.length, 1);
  ctx.calls[0].done(
    { error: 'feature is invalid: missing required file: scenarios.md', reasons: ['missing required file: scenarios.md'] },
    409,
  );
  assert.equal(ctx.featuresError, 'feature is invalid: missing required file: scenarios.md');
});

test('S15/S16: approveFeatureSpec POSTs /api/features/approve_spec with project and slug', () => {
  // S15: Approving the spec commits the feature folder
  const src = extractFunction(qml, 'approveFeatureSpec');
  const ctx = fixture();
  vm.runInNewContext(src, ctx);

  ctx.approveFeatureSpec({ slug: 'approve-feature' });
  assert.equal(ctx.calls.length, 1, 'approveFeatureSpec must call api() exactly once');
  assert.equal(ctx.calls[0].method, 'POST');
  assert.equal(ctx.calls[0].path, '/api/features/approve_spec');
  assert.deepEqual(ctx.calls[0].body, { project: ctx.lastProject, slug: 'approve-feature' });
});

test('S15/S16: Features.canApproveSpec requires a current approving review on a draft', () => {
  // S16: The spec cannot be approved without an approving review of the current content
  const Features = loadFeatures();
  expectHelper(Features, 'canApproveSpec', 'feature');
  const base = { valid: true, spec_status: 'draft', review_current: true, latest_review: { approved: true } };
  assert.equal(Features.canApproveSpec(base), true);
  assert.equal(Features.canApproveSpec({ ...base, valid: false }), false);
  assert.equal(Features.canApproveSpec({ ...base, review_current: false }), false);
  assert.equal(Features.canApproveSpec({ ...base, latest_review: { approved: false } }), false);
  assert.equal(Features.canApproveSpec({ ...base, latest_review: null }), false);
  assert.equal(Features.canApproveSpec({ ...base, spec_status: 'spec approved' }), false);
});

test('S17: approveFeatureScenarios POSTs /api/features/approve_scenarios with project and slug', () => {
  // S17: Approving scenarios records their IDs
  const src = extractFunction(qml, 'approveFeatureScenarios');
  const ctx = fixture();
  vm.runInNewContext(src, ctx);

  ctx.approveFeatureScenarios({ slug: 'scenario-approval' });
  assert.equal(ctx.calls.length, 1, 'approveFeatureScenarios must call api() exactly once');
  assert.equal(ctx.calls[0].method, 'POST');
  assert.equal(ctx.calls[0].path, '/api/features/approve_scenarios');
  assert.deepEqual(ctx.calls[0].body, { project: ctx.lastProject, slug: 'scenario-approval' });
});

test('S17: Features.canApproveScenarios requires an approved spec', () => {
  // S17: Approving scenarios records their IDs
  const Features = loadFeatures();
  expectHelper(Features, 'canApproveScenarios', 'feature');
  assert.equal(Features.canApproveScenarios({ spec_status: 'spec approved' }), true);
  assert.equal(Features.canApproveScenarios({ spec_status: 'draft' }), false);
  assert.equal(Features.canApproveScenarios({ spec_status: 'scenarios approved' }), false);
});

test('S18: specStatusLabel shows draft once a reviewed feature changes', () => {
  // S18: Changing an approved spec reopens it
  const Features = loadFeatures();
  expectHelper(Features, 'specStatusLabel', 'feature');
  assert.equal(Features.specStatusLabel({ spec_status: 'draft', review_current: false }), 'draft');
});
