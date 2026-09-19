// Business tests for the panel parts of milestone M3 (feature to plans)
// scenarios S27 and S28: the Features tab offers "Plan milestone" for a
// planned milestone that covers scenarios, enables it only for a feature whose
// status is `scenarios approved`, calls POST /api/features/plan, shows a
// refusal reason inline and switches to the Plan tab on success.
//
// M3 is not implemented yet: the helpers below do not exist in Features.js and
// FeaturesController.qml does not call the endpoint, so these tests are
// expected to fail on their assertions until the panel stage of the M3 plan.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

// Values produced inside vm.runInNewContext() carry that realm's prototypes,
// so assert/strict deep equality rejects them against literals written here.
const plain = value => JSON.parse(JSON.stringify(value));

const controller = readFileSync(new URL('../quickshell/FeaturesController.qml', import.meta.url), 'utf8');
const featuresSource = readFileSync(new URL('../quickshell/Features.js', import.meta.url), 'utf8');

const features = (() => {
  const module = {};
  vm.runInNewContext(featuresSource.replace(/^\.pragma library\s*$/m, ''), module);
  return module;
})();

const helper = name => {
  assert.equal(typeof features[name], 'function', `Features.js must define ${name}(); it does not exist yet`);
  return features[name];
};

const milestone = (overrides = {}) => ({
  id: 'M1', title: 'First', status: 'planned', scenario_ids: ['S1', 'S2'], business_tests: [], plan_status: '',
  ...overrides,
});
const spec = (specStatus = 'scenarios approved') => ({
  slug: 'demo', status: 'valid', valid: true, spec_status: specStatus, progress: 'planned',
  milestones: [milestone()],
});

test('S27 panel: a planned milestone with covered scenarios shows an enabled Plan milestone action', () => {
  const action = plain(helper('milestonePlanAction')(spec('scenarios approved'), milestone()));
  assert.equal(action.visible, true);
  assert.equal(action.enabled, true);
  assert.equal(action.reason, '');
});

test('S27 panel: the action is hidden for implemented milestones and milestones without covered scenarios', () => {
  const planAction = helper('milestonePlanAction');
  assert.equal(plain(planAction(spec(), milestone({ status: 'implemented' }))).visible, false);
  assert.equal(plain(planAction(spec(), milestone({ scenario_ids: [] }))).visible, false);
  assert.equal(plain(planAction(spec(), milestone({ scenario_ids: undefined }))).visible, false);
});

test('S27 panel: the milestone plan status is planning, planned, completed or empty', () => {
  const status = helper('milestonePlanStatus');
  for (const value of ['planning', 'planned', 'completed'])
    assert.equal(status(milestone({ plan_status: value })), value);
  assert.equal(status(milestone({ plan_status: '' })), '');
  assert.equal(status(milestone({ plan_status: undefined })), '');
  assert.equal(status(null), '');
});

test('S27 panel: the request body carries the project, feature slug and milestone ID', () => {
  assert.deepEqual(plain(helper('featurePlanRequest')('/p/project', 'demo', 'M1')),
    { project: '/p/project', slug: 'demo', milestone: 'M1' });
});

test('S27 panel: FeaturesController calls POST /api/features/plan and switches to the Plan tab on success', () => {
  const start = controller.indexOf('/api/features/plan');
  assert.ok(start >= 0, 'FeaturesController.qml must call POST /api/features/plan; it does not yet');
  assert.match(controller.slice(Math.max(0, start - 20), start), /api\("POST",\s*"$/,
    'the plan endpoint must be called with POST');
  const end = controller.indexOf('}, true)', start);
  assert.ok(end > start, 'the plan call must be followed by its callback');
  const call = controller.slice(start, end);
  const success = call.indexOf('status === 200');
  const switchTab = call.indexOf('host.currentTab = "plan"');
  assert.ok(success >= 0, 'the plan callback must branch on status === 200');
  assert.ok(switchTab > success, 'a 200 response must set host.currentTab = "plan"');
});

test('S28 panel: the action is visible but disabled, with a reason, until the feature is scenarios approved', () => {
  const planAction = helper('milestonePlanAction');
  for (const status of ['draft', 'spec approved']) {
    const action = plain(planAction(spec(status), milestone()));
    assert.equal(action.visible, true, `visible while ${status}`);
    assert.equal(action.enabled, false, `disabled while ${status}`);
    assert.match(action.reason, /scenarios approved/, `the reason names the missing status while ${status}`);
  }
  const invalid = plain(planAction({ ...spec('scenarios approved'), status: 'invalid', valid: false }, milestone()));
  assert.equal(invalid.enabled, false, 'an invalid feature cannot be planned');
  assert.notEqual(invalid.reason, '');
});

test('S28 panel: a refusal shows the engine reason inline and does not switch tabs', () => {
  const start = controller.indexOf('/api/features/plan');
  assert.ok(start >= 0, 'FeaturesController.qml must call POST /api/features/plan; it does not yet');
  const end = controller.indexOf('}, true)', start);
  const call = controller.slice(start, end > start ? end : undefined);
  const otherwise = call.indexOf('else');
  assert.ok(otherwise >= 0, 'the plan callback must handle a non-200 response');
  const refusal = call.slice(otherwise);
  assert.match(refusal, /root\.featuresError\s*=\s*Features\.errorMessage\(resp\)/,
    'a refusal must show the engine reason through featuresError');
  assert.equal(refusal.includes('host.currentTab = "plan"'), false, 'a refusal must stay on the Features tab');
});
