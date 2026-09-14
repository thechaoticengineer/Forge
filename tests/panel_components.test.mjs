import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';

const read = name => readFileSync(new URL('../quickshell/' + name, import.meta.url), 'utf8');
const panel = read('Panel.qml');
const composition = panel + read('AgentOutputView.qml');
const views = [
  'PlanEditorView.qml',
  'AgentOutputView.qml',
  'ArchitectureReviewView.qml',
  'ReportsView.qml',
  'ProjectChooser.qml',
  'CatalogueEditor.qml',
  'DiffView.qml',
];

test('Panel composes focused views while retaining state and API coordination', () => {
  for (const file of views) {
    const type = file.slice(0, -4);
    assert.ok(composition.includes(type + ' {'), type);
    const source = read(file);
    assert.match(source, /\/\/ Interface:/, file + ' documents its boundary');
    assert.doesNotMatch(source, /\broot\./, file + ' has no dynamic Panel scope');
    assert.doesNotMatch(source, /\b(?:api|callApi|sendRequest)\s*\(/,
      file + ' emits actions instead of coordinating HTTP');
  }
  for (const state of ['engineState', 'projectViewRevision', 'stateRequestSerial', 'logFeed'])
    assert.match(panel, new RegExp('property (?:var|int) ' + state + '\\b'));
  assert.match(panel, /function api\(/);
  assert.match(panel, /function refresh\(/);
});

test('moved views reuse shared prose and detail components', () => {
  const plan = read('PlanEditorView.qml');
  const architecture = read('ArchitectureReviewView.qml');
  const output = read('AgentOutputView.qml');
  const reports = read('ReportsView.qml');
  assert.match(plan, /component StageProseField: StageProse/);
  assert.match(architecture, /ArchitectureDetails \{/);
  assert.match(architecture, /DetailFields \{/);
  assert.match(architecture, /CompactDetail \{/);
  assert.match(output, /DetailList \{/);
  assert.match(output, /ReportsView \{/);
  assert.match(reports, /component ViewFields: DetailFields/);
});
