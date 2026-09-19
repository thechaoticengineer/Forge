// Business tests for the panel logic of milestone M5 (panel viewer) scenarios
// S36 (feature page navigation, Markdown segments), S39 (scenario result
// view), S40 (design items), S41 (page header) and S42 (page actions).
//
// M5 is not implemented yet: the helpers below do not exist, so these tests
// fail on the assertion naming the missing helper until the panel stages of
// the M5 plan add them. Helpers may live in quickshell/Features.js or in a new
// quickshell/FeatureViewer.js; both are searched, Features.js first.
//
// Contract pinned here (later stages implement it exactly):
// - featurePageTabs() -> ["README", "Scenarios", "Decisions", "Milestones", "Design"]
// - markdownSegments(text) -> [{kind: "markdown", text} | {kind: "mermaid", text}]
// - scenarioResultView(scenario) -> {label, evidence, when, plan, outOfDate}
// - designItems(design) -> [{kind, label, image, note} | {kind: "mermaid", label, text}]
// - featurePageHeader(spec, state) -> {validation, reasons, specStatus, progress,
//   review, milestonePlans}; spec is a featureSpecsBySlug entry and state the
//   GET /api/features/state response.
// - featurePageActions(spec, activity) -> [{id, label, enabled, reason, milestone?}]
//   with ids review, approveSpec, approveScenarios and one plan per plannable milestone.
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import vm from 'node:vm';

// Values produced inside vm.runInNewContext() carry that realm's prototypes,
// so assert/strict deep equality rejects them against literals written here.
const plain = value => JSON.parse(JSON.stringify(value));

const quickshell = new URL('../quickshell/', import.meta.url);
const read = name => readFileSync(new URL(name, quickshell), 'utf8');

const load = name => {
  if (!existsSync(new URL(name, quickshell))) return {};
  const module = {};
  vm.runInNewContext(read(name).replace(/^\.pragma library\s*$/m, ''), module);
  return module;
};
const features = load('Features.js');
const viewer = load('FeatureViewer.js');

const helper = name => {
  const fn = typeof features[name] === 'function' ? features[name] : viewer[name];
  assert.equal(typeof fn, 'function',
    `Features.js (or FeatureViewer.js) must define ${name}(); it does not exist yet`);
  return fn;
};

const controller = read('FeaturesController.qml');
const featuresView = read('FeaturesView.qml');

// ---------------------------------------------------------------- fixtures

const milestone = (overrides = {}) => ({
  id: 'M1', title: 'First', status: 'planned', covers: ['S1', 'S2'], business_tests: [], plan: null, ...overrides,
});

const spec = (overrides = {}) => ({
  slug: 'demo', status: 'valid', valid: true, spec_status: 'scenarios approved', content_hash: 'h1',
  latest_review: { approved: true, summary: 'Consistent.', issues: [], questions: [] }, review_current: true,
  progress: 'in progress',
  milestones: [
    milestone({ id: 'M1', title: 'Done', status: 'implemented', covers: ['S1'] }),
    milestone({ id: 'M2', title: 'Planned', covers: ['S2', 'S3'], plan: { status: 'planning' } }),
    milestone({ id: 'M3', title: 'Uncovered', covers: [] }),
    milestone({ id: 'M4', title: 'Also planned', covers: ['S4'] }),
  ],
  ...overrides,
});

const state = (overrides = {}) => ({ slug: 'demo', valid: true, reasons: [], ...overrides });

// ------------------------------------------------------------- S36: tabs

test('S36: the feature page has the sub-tabs README, Scenarios, Decisions, Milestones and Design', () => {
  assert.deepEqual(plain(helper('featurePageTabs')()), ['README', 'Scenarios', 'Decisions', 'Milestones', 'Design']);
});

test('S36: Markdown without a Mermaid block is one Markdown segment with the text unchanged', () => {
  const text = '# Title\n\n- one\n- two\n\n```js\nconst x = 1\n```\n\n[link](https://example.com) and *emphasis*\n';
  assert.deepEqual(plain(helper('markdownSegments')(text)), [{ kind: 'markdown', text }]);
});

test('S36: a fenced mermaid block becomes a Mermaid segment and everything else stays Markdown', () => {
  const text = '# Flow\n\nIntro.\n\n```mermaid\ngraph TD\n  A --> B\n```\n\nAfter.\n\n```js\nconst x = 1\n```\n';
  const segments = plain(helper('markdownSegments')(text));
  assert.deepEqual(segments.map(s => s.kind), ['markdown', 'mermaid', 'markdown']);
  assert.equal(segments[1].text.trim(), 'graph TD\n  A --> B');
  assert.ok(segments[0].text.includes('# Flow') && segments[0].text.includes('Intro.'), 'text before the block stays Markdown');
  assert.ok(segments[2].text.includes('After.') && segments[2].text.includes('```js\nconst x = 1\n```'),
    'text after the block, including other code fences, stays Markdown');
  for (const s of segments) assert.equal(s.text.includes('```mermaid'), false, 'the mermaid fence is not part of a segment');
});

test('S36: several mermaid blocks keep their order and an empty text has no segments', () => {
  const markdownSegments = helper('markdownSegments');
  const text = '```mermaid\ngraph LR\n  A --> B\n```\nmiddle\n```mermaid\nsequenceDiagram\n  A->>B: hi\n```\n';
  const mermaid = plain(markdownSegments(text)).filter(s => s.kind === 'mermaid').map(s => s.text.trim());
  assert.deepEqual(mermaid, ['graph LR\n  A --> B', 'sequenceDiagram\n  A->>B: hi']);
  assert.deepEqual(plain(markdownSegments('')), []);
  assert.deepEqual(plain(markdownSegments(null)), []);
});

test('S36: Enter in the Features list opens the feature page while o keeps opening the editor', () => {
  const body = featuresView.slice(featuresView.indexOf('function handleKey('));
  assert.ok(body.length > 0, 'FeaturesView.qml must define handleKey()');
  const branches = body.split(/\belse if\b/);
  const enter = branches.find(branch => /Qt\.Key_Return/.test(branch));
  assert.ok(enter, 'handleKey must have an Enter/Return branch');
  assert.ok(/Qt\.Key_Enter/.test(enter), 'Return and the keypad Enter behave the same');
  assert.equal(/Qt\.Key_O\b/.test(enter), false, 'Enter must not share its branch with the editor key o');
  assert.equal(/activateSelection|openRequested/.test(enter), false, 'Enter must not open the editor');
  assert.ok(/[Pp]age/.test(enter), 'Enter must open the feature page');
  const editor = branches.find(branch => /Qt\.Key_O\b/.test(branch));
  assert.ok(editor, 'handleKey must keep an o branch');
  assert.ok(/activateSelection|openRequested/.test(editor), 'o keeps opening the editor');
});

test('S36: clicking a feature row selects it and opens the feature page', () => {
  const rowClick = /MouseArea\s*\{[^}]*?onClicked:\s*\{([^}]*)\}/.exec(featuresView.slice(featuresView.indexOf('delegate: Rectangle')));
  assert.ok(rowClick, 'the feature row must have a click handler');
  assert.ok(/featuresList\.currentIndex\s*=/.test(rowClick[1]), 'a click keeps selecting the row');
  assert.ok(/[Pp]age/.test(rowClick[1]), 'a click opens the feature page');
});

// ---------------------------------------------- S39: scenario result view

const result = (overrides = {}) => ({
  status: 'passed', evidence: 'the S1 test passes', role: 'reviewer', plan_id: 'plan-20260919-1',
  milestone: 'M1', unix: 1800000000, out_of_date: false, ...overrides,
});

test('S39: a scenario without a recorded result is labelled not recorded', () => {
  const view = plain(helper('scenarioResultView')({ id: 'S1', result: null }));
  assert.equal(view.label, 'not recorded');
  assert.equal(view.evidence, '');
  assert.equal(view.outOfDate, false);
  assert.equal(plain(helper('scenarioResultView')({ id: 'S1' })).label, 'not recorded', 'a missing result field is not recorded');
});

test('S39: a passed result shows its time and plan but no evidence', () => {
  const view = plain(helper('scenarioResultView')({ id: 'S1', result: result() }));
  assert.equal(view.label, 'passed');
  assert.equal(view.evidence, '', 'evidence is shown only for failed results');
  assert.equal(view.plan, 'plan-20260919-1');
  assert.equal(typeof view.when, 'string');
  assert.ok(view.when.includes('2027'), `the time of the result is shown, got ${JSON.stringify(view.when)}`);
  assert.equal(view.outOfDate, false);
});

test('S39: a failed result shows its evidence', () => {
  const view = plain(helper('scenarioResultView')({ id: 'S2', result: result({ status: 'failed', evidence: 'no executable test names S2' }) }));
  assert.equal(view.label, 'failed');
  assert.equal(view.evidence, 'no executable test names S2');
});

test('S39: a result whose scenario changed afterwards is marked possibly out of date', () => {
  const scenarioResultView = helper('scenarioResultView');
  assert.equal(plain(scenarioResultView({ id: 'S1', result: result({ out_of_date: true }) })).outOfDate, true);
  assert.equal(plain(scenarioResultView({ id: 'S1', result: result({ status: 'failed', out_of_date: true }) })).outOfDate, true);
  assert.equal(plain(scenarioResultView({ id: 'S1', result: result({ out_of_date: false }) })).outOfDate, false);
});

// ------------------------------------------------------- S40: design items

const design = () => [
  { path: 'design/main.pen', kind: 'pen', png: 'design/main.png' },
  { path: 'design/main.png', kind: 'png', absolute_path: '/p/docs/features/demo/design/main.png' },
  { path: 'design/lonely.pen', kind: 'pen', png: null },
  { path: 'design/flow.mmd', kind: 'mermaid', text: 'graph LR\n  A --> B\n' },
  { path: 'design/notes.txt', kind: 'other' },
];

test('S40: a .pen file is paired with its exported PNG and labelled with its .pen path', () => {
  const items = plain(helper('designItems')(design()));
  const pen = items.find(item => item.label === 'design/main.pen');
  assert.ok(pen, `the .pen file is listed by its path: ${JSON.stringify(items)}`);
  assert.equal(pen.kind, 'pen');
  assert.equal(pen.image, '/p/docs/features/demo/design/main.png');
  assert.equal(items.some(item => item.label === 'design/main.png'), false, 'the PNG is shown under its .pen file, not twice');
});

test('S40: a .pen file without a PNG is listed with a note that no export exists', () => {
  const items = plain(helper('designItems')(design()));
  const lonely = items.find(item => item.label === 'design/lonely.pen');
  assert.ok(lonely, `the .pen file without export is listed: ${JSON.stringify(items)}`);
  assert.equal(lonely.image, null);
  assert.equal(lonely.note, 'no export exists');
});

test('S40: a Mermaid file becomes a Mermaid item with its source, and other files are not listed', () => {
  const items = plain(helper('designItems')(design()));
  const flow = items.find(item => item.label === 'design/flow.mmd');
  assert.ok(flow, `the .mmd file is listed: ${JSON.stringify(items)}`);
  assert.equal(flow.kind, 'mermaid');
  assert.equal(flow.text, 'graph LR\n  A --> B\n');
  assert.deepEqual(items.map(item => item.label), ['design/main.pen', 'design/lonely.pen', 'design/flow.mmd']);
  assert.deepEqual(plain(helper('designItems')([])), []);
  assert.deepEqual(plain(helper('designItems')(null)), []);
});

// -------------------------------------------------- S41: page header

test('S41: the header of a valid feature shows validation, spec status, progress and milestone plans', () => {
  const header = plain(helper('featurePageHeader')(spec(), state()));
  assert.equal(header.validation, 'valid');
  assert.deepEqual(header.reasons, []);
  assert.equal(header.specStatus, 'scenarios approved');
  assert.equal(header.progress, '1/4 implemented');
  assert.deepEqual(header.milestonePlans, [{ id: 'M2', status: 'planning' }]);
});

test('S41: the header of an invalid feature carries its validation reasons', () => {
  const reasons = ['milestone M1 covers unknown scenario S99', 'missing required file: decisions.md'];
  const header = plain(helper('featurePageHeader')(
    spec({ status: 'invalid', valid: false, spec_status: 'draft' }), state({ valid: false, reasons })));
  assert.equal(header.validation, 'invalid');
  assert.deepEqual(header.reasons, reasons);
  assert.equal(header.specStatus, 'draft');
});

test('S41: the header shows the latest spec review verdict and whether it covers the current content', () => {
  const headerOf = helper('featurePageHeader');
  const approved = plain(headerOf(spec(), state()));
  assert.deepEqual(approved.review, { verdict: 'approved', current: true });
  const outdated = plain(headerOf(spec({
    latest_review: { approved: false, summary: 'Needs work.', issues: ['x'], questions: [] }, review_current: false,
  }), state()));
  assert.deepEqual(outdated.review, { verdict: 'changes requested', current: false });
  assert.equal(plain(headerOf(spec({ latest_review: null, review_current: false }), state())).review, null,
    'a feature without a review has no verdict');
});

test('S41: progress counts implemented milestones and lists every plan status', () => {
  const headerOf = helper('featurePageHeader');
  const all = spec({ progress: 'implemented', milestones: [
    milestone({ id: 'M1', status: 'implemented', plan: { status: 'completed' } }),
    milestone({ id: 'M2', status: 'implemented', plan: { status: 'completed' } }),
  ] });
  const done = plain(headerOf(all, state()));
  assert.equal(done.progress, '2/2 implemented');
  assert.deepEqual(done.milestonePlans, [{ id: 'M1', status: 'completed' }, { id: 'M2', status: 'completed' }]);
  const none = plain(headerOf(spec({ progress: 'planned', milestones: [milestone({ id: 'M1' }), milestone({ id: 'M2', plan: { status: 'failed' } })] }), state()));
  assert.equal(none.progress, '0/2 implemented');
  assert.deepEqual(none.milestonePlans, [{ id: 'M2', status: 'failed' }]);
});

// ------------------------------------------------- S42: page actions

const action = (actions, id, milestoneId) =>
  actions.find(a => a.id === id && (milestoneId === undefined || a.milestone === milestoneId));

const specs = () => [
  spec({ spec_status: 'draft' }),
  spec({ spec_status: 'spec approved' }),
  spec({ spec_status: 'scenarios approved' }),
  spec({ spec_status: 'draft', latest_review: { approved: false, summary: 'no' }, review_current: true }),
  spec({ spec_status: 'draft', review_current: false }),
  spec({ status: 'invalid', valid: false, spec_status: 'draft' }),
];

test('S42: the page offers Request review, Approve spec, Approve scenarios and one Plan milestone per plannable milestone', () => {
  const actions = plain(helper('featurePageActions')(spec(), null));
  assert.deepEqual(actions.map(a => a.id).filter(id => id !== 'plan'), ['review', 'approveSpec', 'approveScenarios']);
  assert.equal(action(actions, 'review').label, 'Request review');
  assert.equal(action(actions, 'approveSpec').label, 'Approve spec');
  assert.equal(action(actions, 'approveScenarios').label, 'Approve scenarios');
  const plans = actions.filter(a => a.id === 'plan');
  assert.deepEqual(plans.map(a => a.milestone), ['M2', 'M4'], 'only planned milestones that cover scenarios');
  for (const plan of plans) assert.ok(plan.label.startsWith('Plan milestone'), `label was ${plan.label}`);
});

test('S42: every action is enabled under the same conditions as in the Features tab', () => {
  const actionsOf = helper('featurePageActions');
  for (const current of specs()) {
    for (const activity of [null, { slug: 'demo', status: 'running' }]) {
      const actions = plain(actionsOf(current, activity));
      const label = `${current.spec_status}/${current.valid ? 'valid' : 'invalid'}/${activity ? 'busy' : 'idle'}`;
      assert.equal(action(actions, 'review').enabled, current.valid && !activity, `Request review ${label}`);
      assert.equal(action(actions, 'approveSpec').enabled, helper('canApproveSpec')(current), `Approve spec ${label}`);
      assert.equal(action(actions, 'approveScenarios').enabled, helper('canApproveScenarios')(current), `Approve scenarios ${label}`);
      for (const m of current.milestones) {
        const expected = plain(helper('milestonePlanAction')(current, m));
        const entry = action(actions, 'plan', m.id);
        assert.equal(!!entry, expected.visible, `Plan milestone ${m.id} is offered exactly when the Features tab shows it (${label})`);
        if (entry) {
          assert.equal(entry.enabled, expected.enabled, `Plan milestone ${m.id} ${label}`);
          assert.equal(entry.reason, expected.reason, `Plan milestone ${m.id} reason ${label}`);
        }
      }
    }
  }
});

test('S42: a disabled action explains why', () => {
  const actionsOf = helper('featurePageActions');
  const draft = plain(actionsOf(spec({ spec_status: 'draft', review_current: false }), null));
  for (const id of ['approveSpec', 'approveScenarios']) {
    assert.equal(action(draft, id).enabled, false, id);
    assert.notEqual(action(draft, id).reason, '', `${id} names why it is disabled`);
  }
  assert.match(action(draft, 'plan', 'M2').reason, /scenarios approved/);
  const busy = plain(actionsOf(spec(), { slug: 'demo', status: 'running' }));
  assert.equal(action(busy, 'review').enabled, false);
  assert.notEqual(action(busy, 'review').reason, '');
  const invalid = plain(actionsOf(spec({ status: 'invalid', valid: false }), null));
  assert.equal(action(invalid, 'review').enabled, false);
  assert.notEqual(action(invalid, 'review').reason, '');
  const ready = plain(actionsOf(spec({ spec_status: 'spec approved' }), null));
  assert.deepEqual([action(ready, 'approveScenarios').enabled, action(ready, 'approveScenarios').reason], [true, '']);
});

test('S42: the page actions go through the existing controller functions and endpoints', () => {
  const endpoints = {
    requestFeatureReview: '/api/features/review',
    approveFeatureSpec: '/api/features/approve_spec',
    approveFeatureScenarios: '/api/features/approve_scenarios',
    planMilestone: '/api/features/plan',
  };
  for (const [name, endpoint] of Object.entries(endpoints)) {
    const start = controller.indexOf(`function ${name}(`);
    assert.ok(start >= 0, `FeaturesController.qml must keep function ${name}()`);
    const next = controller.indexOf('\n  function ', start + 1);
    const body = controller.slice(start, next > start ? next : undefined);
    assert.ok(body.includes(`"${endpoint}"`), `${name}() must call ${endpoint}`);
  }
  // The page is wired to those functions where it is hosted, so no second
  // request path or enablement rule exists for it.
  const hosts = readdirSync(quickshell).filter(f => f.endsWith('.qml') && !['FeaturePage.qml', 'FeaturesController.qml'].includes(f))
    .map(f => ({ f, text: read(f) })).filter(({ text }) => /\bFeaturePage\s*\{/.test(text));
  assert.ok(hosts.length > 0, 'a FeaturePage must be hosted in the panel (none is yet)');
  for (const { f, text } of hosts) {
    const page = text.slice(text.search(/\bFeaturePage\s*\{/));
    for (const name of Object.keys(endpoints))
      assert.ok(page.includes(`${name}(`), `${f} must wire FeaturePage to FeaturesController.${name}()`);
  }
});

test('S42: the Features tab keeps its actions and the page offers chat and review details', () => {
  for (const name of ['featureChatButton', 'featureReviewButton', 'featureApproveSpecButton',
    'featureApproveScenariosButton', 'featureOpenButton', 'featurePlanMilestoneButton'])
    assert.ok(featuresView.includes(`"${name}"`), `FeaturesView.qml must keep ${name}`);
  assert.ok(existsSync(new URL('FeaturePage.qml', quickshell)), 'quickshell/FeaturePage.qml must exist; it does not yet');
  const page = existsSync(new URL('FeaturePage.qml', quickshell)) ? read('FeaturePage.qml') : '';
  for (const name of ['featurePageChat', 'featurePageReviewDetails', 'featurePageReview', 'featurePageApproveSpec',
    'featurePageApproveScenarios', 'featurePagePlan_'])
    assert.ok(page.includes(name), `FeaturePage.qml must define ${name}`);
});

test('S42: the panel stays below 1800 lines', () => {
  const lines = read('Panel.qml').split('\n').length;
  assert.ok(lines < 1800, `Panel.qml has ${lines} lines; it must remain below 1800`);
  // The page itself needs new panel wiring, so a page that is not hosted yet fails here.
  assert.ok(existsSync(new URL('FeaturePage.qml', quickshell)), 'the feature page is not implemented yet');
});
