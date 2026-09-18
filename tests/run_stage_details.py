"""Run the actual read-only stage-card subtree offscreen, without Omarchy imports.
Only theme/clipboard/viewport services are substituted; Panel helpers, request
handlers, bindings, delegates and controls are extracted from the current source.
"""
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

repo = Path(__file__).resolve().parents[1]
panel = (repo / 'quickshell/Panel.qml').read_text()
plan_editor = (repo / 'quickshell/PlanEditorView.qml').read_text()

def between(start, end):
    offset = panel.index(start)
    return panel[offset:panel.index(end, offset)]

plan_view = (repo / 'quickshell/PlanView.qml').read_text()

helpers = between('  function revealDetail(', '  function reviewScope(')
helpers += between('  function reviewScope(', '  function stageActivity(')
# The review presentation helpers moved to ReviewPresentation.js; PlanView passes them
# to the editor, wrapping the preview reconciliation with its own review view and scope.
helpers += ''.join('  readonly property var %s: ReviewPresentation.%s\n' % pair
                   for pair in re.findall(r'^      (\w+): ReviewPresentation\.(\w+)$', plan_view, re.M))
reconcile = plan_view[plan_view.index('  function reconcileStageReviewPresentation('):plan_view.index('  function reveal(')]
helpers += re.sub(r'\bview\.', 'root.', reconcile)
helpers += between('  function stageActivity(', '  function chooseRow(')
def editor_between(start, end):
    offset = plan_editor.index(start)
    return plan_editor[offset:plan_editor.index(end, offset)]

stage_prose_component = editor_between('  component StageProseField:', '  component ViewButton:')
content = editor_between('              Column {\n                id: stageContent', '              Loader {\n                id: stageEditor')
state = between('  property var reviewViews:', '  function revealDetail(')
state += between('  property int expandedStageId:', '  property int selectedStageIndex:')
state += between('  readonly property var displayedStages:', '  readonly property bool editValid:')
delegate = editor_between('              readonly property var modelData: view.displayedStages[index]', '              readonly property var reviewView:')
model = editor_between('            model: view.displayedStages.length', '            delegate: Rectangle {')
shortcut = between('              } else if (event.key === Qt.Key_Return', '                event.accepted = true\n              }\n            }')
shortcut = shortcut.replace('              } else if', '              if', 1) + '                event.accepted = true\n              }\n'
properties = editor_between('              readonly property var reviewView:', '              width: stageList.width')
def fixture_scope(fragment):
    fragment = re.sub(r'\bview\.', 'root.', fragment)
    fragment = fragment.replace('root.detailRevealed(', 'panelScroll.reveal(')
    fragment = fragment.replace('root.leaveRequested()', 'keyHandler.forceActiveFocus()')
    fragment = fragment.replace('root.detailInspected(stageProse)', '{}')
    fragment = fragment.replace('root.expandedStageRequested(stageRow.expanded ? -1 : stageRow.modelData.id)',
                                'root.expandedStageId = stageRow.expanded ? -1 : stageRow.modelData.id')
    fragment = fragment.replace('root.stageRoutingExpandedRequested(!root.stageRoutingExpanded)',
                                'root.stageRoutingExpanded = !root.stageRoutingExpanded')
    return fragment

helpers += fixture_scope(stage_prose_component)
content = fixture_scope(content)
delegate = fixture_scope(delegate)
model = fixture_scope(model)
properties = fixture_scope(properties)
shortcut = shortcut.replace('planEditor.stageList', 'stageList')
fixture = (repo / 'tests/stage_details_fixture.qml.in').read_text()
fixture = fixture.replace('import QtTest', 'import QtTest\nimport "components/PanelDetails.js" as PanelDetails'
                          '\nimport "components/ReviewPresentation.js" as ReviewPresentation')
fixture = fixture.replace('// PANEL_HELPERS', helpers).replace('// PANEL_STAGE_CONTENT', content)
fixture = fixture.replace('// PANEL_STAGE_PROPERTIES', properties)
fixture = fixture.replace('// PANEL_STAGE_STATE', state).replace('// PANEL_STAGE_DELEGATE', delegate)
fixture = fixture.replace('// PANEL_STAGE_MODEL', model).replace('// PANEL_STAGE_SHORTCUT', shortcut)
fixture = fixture.replace('Style.space(', 'style.space(').replace('Quickshell.clipboardText', 'clipboard.clipboardText')
with tempfile.TemporaryDirectory(prefix='forge-stage-details-') as directory:
    path = Path(directory)
    (path / 'components').mkdir()
    for name in ('StageProse.qml', 'CompactDetail.qml', 'DetailFields.qml', 'DetailText.js', 'PanelDetails.js', 'ReviewView.js',
                 'ReviewPresentation.js',
                 'ModelRouting.js', 'UsageFormat.js', 'PlanEdit.js'):
        shutil.copyfile(repo / 'quickshell' / name, path / 'components' / name)
    (path / 'tst_stage.qml').write_text(fixture)
    runtime = path / 'runtime'
    runtime.mkdir(mode=0o700)
    env = dict(os.environ, QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software',
               QT_QPA_PLATFORMTHEME='', QT_QUICK_CONTROLS_STYLE='Basic',
               GSETTINGS_BACKEND='memory', XDG_RUNTIME_DIR=str(runtime))
    result = subprocess.run(['/usr/lib/qt6/bin/qmltestrunner', '-input', str(path)],
                            env=env, capture_output=True, text=True, timeout=120)
    print(result.stdout + result.stderr, end='')
    raise SystemExit(result.returncode)
