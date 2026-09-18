import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';

const read = name => readFileSync(new URL('../quickshell/' + name, import.meta.url), 'utf8');
const panel = read('Panel.qml');
// Tab views host the focused views that moved out of Panel.qml (ActivityView hosts
// AgentOutputView, ArchitectureView hosts ArchitectureReviewView, PlanView hosts PlanEditorView and
// PlanStageList; the stage detail page moved out of PlanEditorView's inline expansion).
const composition = panel + read('AgentOutputView.qml') + read('ActivityView.qml') + read('ArchitectureView.qml')
  + read('PlanView.qml') + read('SettingsView.qml');
const views = [
  'PlanEditorView.qml',
  'PlanView.qml',
  'PlanStageList.qml',
  'StageDetailPage.qml',
  'SettingsView.qml',
  'AgentOutputView.qml',
  'ArchitectureReviewView.qml',
  'ActivityView.qml',
  'ArchitectureView.qml',
  'QueueView.qml',
  'ReportsView.qml',
  'ProjectChooser.qml',
  'CatalogueEditor.qml',
  'DiffView.qml',
  'DiscussionView.qml',
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
  // The stage prose fields moved with the inline expansion to the stage detail page (M2).
  const plan = read('StageDetailPage.qml');
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

test('root and extracted views share one button implementation', () => {
  const button = read('PanelViewButton.qml');
  // The Overview buttons moved from Panel.qml's PanelButton into OverviewView.qml.
  const overview = read('OverviewView.qml');
  assert.match(overview, /component OverviewButton: PanelViewButton/);
  assert.doesNotMatch(panel + overview, /component \w*Button: Rectangle/);
  assert.match(button, /property bool enabled: true/);
  assert.match(button, /enabled: button\.enabled/);
  // qs-free so the qs-free views load it under qmltestrunner; callers pass the shell padding.
  assert.doesNotMatch(button, /^\s*import\s+(Quickshell|qs\.)/m);
  assert.match(button, /property real horizontalPadding: 18/);
  assert.match(button, /property real verticalPadding: 10/);
  assert.match(overview, /component OverviewButton: PanelViewButton \{[\s\S]*?horizontalPadding: view\.horizontalPadding\n\s*verticalPadding: view\.verticalPadding/);
  assert.match(panel, /OverviewView \{[\s\S]*?horizontalPadding: Style\.space\(18\)\n\s*verticalPadding: Style\.space\(10\)/);
});

test('Panel hosts its content on a StackView page', () => {
  assert.match(panel, /StackView\s*\{/);
  assert.match(panel, /id:\s*panelStack/);
  assert.match(panel, /anchors\.fill:\s*parent\s*\n\s*initialItem:\s*panelPage/);
  assert.match(panel, /Item\s*\{\s*\n\s*id:\s*panelPage/);

  const stackIndex = panel.indexOf('StackView {');
  const pageIndex = panel.indexOf('id: panelPage');
  // The page content: the Overview view took the old panelScroll column's place.
  const scrollIndex = panel.indexOf('OverviewView {');
  const hintIndex = panel.indexOf('id: keyboardHint');
  const catalogueIndex = panel.indexOf('CatalogueEditor {');
  assert.ok(stackIndex >= 0 && pageIndex >= 0 && scrollIndex >= 0
    && hintIndex >= 0 && catalogueIndex >= 0);
  assert.ok(stackIndex < pageIndex, 'panelStack comes before panelPage');
  assert.ok(pageIndex < scrollIndex, 'panelPage comes before panelScroll');
  assert.ok(scrollIndex < hintIndex, 'panelScroll comes before keyboardHint');
  assert.ok(hintIndex < catalogueIndex, 'keyboardHint comes before the overlays');
});
