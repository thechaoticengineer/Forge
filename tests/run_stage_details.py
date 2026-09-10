"""Run the actual read-only stage-card subtree offscreen, without Omarchy imports.
Only theme/clipboard/viewport services are substituted; Panel helpers, request
handlers, bindings, delegates and controls are extracted from the current source.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

repo = Path(__file__).resolve().parents[1]
panel = (repo / 'quickshell/Panel.qml').read_text()

def between(start, end):
    offset = panel.index(start)
    return panel[offset:panel.index(end, offset)]

helpers = between('  function revealDetail(', '  function reviewScope(')
helpers += between('  function stageModelText(', '  function changeModelConstraint(')
helpers += between('  function reviewScope(', '  function reviewGateText(')
helpers += between('  function reviewGateText(', '  function reportTime(')
helpers += between('  function nonNegativeInt(', '  function formatTokens(')
content = between('              Column {\n                id: stageContent', '              Loader {\n                id: stageEditor')
state = between('  property var reviewViews:', '  function revealDetail(')
state += between('  property int expandedStageId:', '  property int selectedStageIndex:')
state += between('  readonly property var displayedStages:', '  readonly property bool editValid:')
delegate = between('              readonly property var modelData: root.displayedStages[index]', '              readonly property var reviewView:')
model = between('            model: root.displayedStages.length', '            delegate: Rectangle {')
shortcut = between('              } else if (event.key === Qt.Key_Return', '                event.accepted = true\n              }\n            }')
shortcut = shortcut.replace('              } else if', '              if', 1) + '                event.accepted = true\n              }\n'
properties = between('              readonly property var reviewView:', '              width: stageList.width')
fixture = (repo / 'tests/stage_details_fixture.qml.in').read_text()
fixture = fixture.replace('import QtTest', 'import QtTest\nimport "components/PanelDetails.js" as PanelDetails')
fixture = fixture.replace('// PANEL_HELPERS', helpers).replace('// PANEL_STAGE_CONTENT', content)
fixture = fixture.replace('// PANEL_STAGE_PROPERTIES', properties)
fixture = fixture.replace('// PANEL_STAGE_STATE', state).replace('// PANEL_STAGE_DELEGATE', delegate)
fixture = fixture.replace('// PANEL_STAGE_MODEL', model).replace('// PANEL_STAGE_SHORTCUT', shortcut)
fixture = fixture.replace('Style.space(', 'style.space(').replace('Quickshell.clipboardText', 'clipboard.clipboardText')
with tempfile.TemporaryDirectory(prefix='forge-stage-details-') as directory:
    path = Path(directory)
    (path / 'components').mkdir()
    for name in ('StageProse.qml', 'CompactDetail.qml', 'DetailFields.qml', 'DetailText.js', 'PanelDetails.js', 'ReviewView.js'):
        shutil.copyfile(repo / 'quickshell' / name, path / 'components' / name)
    (path / 'tst_stage.qml').write_text(fixture)
    runtime = path / 'runtime'
    runtime.mkdir(mode=0o700)
    env = dict(os.environ, QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software',
               QT_QPA_PLATFORMTHEME='', QT_QUICK_CONTROLS_STYLE='Basic',
               GSETTINGS_BACKEND='memory', XDG_RUNTIME_DIR=str(runtime))
    result = subprocess.run(['/usr/lib/qt6/bin/qmltestrunner', '-input', str(path)],
                            env=env, capture_output=True, text=True, timeout=45)
    print(result.stdout + result.stderr, end='')
    raise SystemExit(result.returncode)
