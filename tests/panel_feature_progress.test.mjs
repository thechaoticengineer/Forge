// Milestone progress in the Features tab: the per-slug index carries the
// engine's progress fields, implemented features are hidden by default and
// partly implemented ones are labelled N/M implemented. featureRows keeps its
// shape (pinned by the feature-specs S7 business test).
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const plain = value => JSON.parse(JSON.stringify(value));
const load = name => {
  const module = {};
  const source = readFileSync(new URL(`../quickshell/${name}`, import.meta.url), 'utf8');
  vm.runInNewContext(source.replace(/^\.pragma library\s*$/m, ''), module);
  return module;
};
const features = load('Features.js');
const navigation = load('PanelNavigation.js');

const response = {features: [
  {slug: 'done', title: 'Done', path: '/p/done', status: 'valid', reasons: [], progress: 'implemented',
    milestones: [{id: 'M1', title: 'One', status: 'implemented'}]},
  {slug: 'partial', title: 'Partial', path: '/p/partial', status: 'valid', reasons: [], progress: 'in progress',
    milestones: [{id: 'M1', title: 'One', status: 'implemented'}, {id: 'M2', title: 'Two', status: 'planned'},
      {id: 'M3', title: 'Three', status: 'planned'}]},
  {slug: 'legacy', title: 'Legacy', path: '/p/legacy', status: 'valid', reasons: []},
]};

test('the spec index carries progress and milestones, defaulting to planned', () => {
  const specs = plain(features.featureSpecsBySlug(response));
  assert.deepEqual(Object.values(specs).map(s => [s.slug, s.progress, s.milestones.length]),
    [['done', 'implemented', 1], ['partial', 'in progress', 3], ['legacy', 'planned', 0]]);
  assert.equal('progress' in plain(features.featureRows(response))[0], false);
});

test('implemented rows are hidden unless shown, and counted', () => {
  const rows = features.featureRows(response);
  const specs = features.featureSpecsBySlug(response);
  assert.deepEqual(plain(features.visibleRows(rows, specs, false)).map(r => r.slug), ['partial', 'legacy']);
  assert.deepEqual(plain(features.visibleRows(rows, specs, true)).map(r => r.slug), ['done', 'partial', 'legacy']);
  assert.deepEqual(plain(features.visibleRows(rows, {}, false)).map(r => r.slug), ['done', 'partial', 'legacy']);
  assert.equal(features.implementedCount(rows, specs), 1);
  assert.deepEqual(plain(features.visibleRows(null, specs, false)), []);
});

test('progress label shows implemented, N/M implemented, or nothing', () => {
  const specs = features.featureSpecsBySlug(response);
  assert.deepEqual(['done', 'partial', 'legacy'].map(slug => features.progressLabel(specs[slug])),
    ['implemented', '1/3 implemented', '']);
  assert.equal(features.progressLabel(null), '');
});

test('keyboard help and the Features hint name the i toggle', () => {
  assert.ok(plain(navigation.helpRows()).some(r => r.key === 'i' && r.description === 'Show / hide implemented features'));
  assert.match(navigation.hintText('features', false), /i implemented/);
});
