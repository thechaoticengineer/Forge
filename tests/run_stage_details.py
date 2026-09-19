"""Run the actual read-only stage details offscreen, without Omarchy imports.
Only theme/clipboard/viewport/stack services are substituted; Panel helpers, request
handlers, bindings, delegates, controls and the stage detail page come from the
current source.

Panel-redesign M2 moved the inline expanded stage out of PlanEditorView.qml onto
its own page, StageDetailPage.qml, which Panel.qml pushes onto panelStack. The
fixture therefore runs the Plan stage card (PlanEditorView's delegate, whose
header toggle now opens the page), the unmodified StageDetailPage.qml with
Panel.qml's own StageDetailPage instance wiring, Panel.qml's openStageDetail /
closeStageDetail, its stage detail key branch and its Enter / o / Space shortcut.
"Expanded" means the page shows that stage (expandedStageId), "collapsed" that it
is closed.
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

def editor_between(start, end):
    offset = plan_editor.index(start)
    return plan_editor[offset:plan_editor.index(end, offset)]

def instance(source, type_name):
    """The brace-matched `Type { ... }` block, skipping strings and comments."""
    start = source.index(type_name + ' {')
    i, depth, quote = source.index('{', start), 0, None
    while i < len(source):
        c = source[i]
        if quote:
            if c == '\\':
                i += 2
                continue
            if c == quote:
                quote = None
        elif source.startswith('//', i):
            i = source.index('\n', i)
            continue
        elif c in '"\'':
            quote = c
        elif c == '{':
            depth += 1
        elif c == '}':
            depth -= 1
            if depth == 0:
                return source[start:i + 1]
        i += 1
    raise ValueError(type_name)

helpers = between('  function revealDetail(', '  function reviewScope(')
helpers += between('  function reviewScope(', '  function stageActivity(')
helpers += between('  function stageActivity(', '  function reviewerLabel(')
helpers += between('  function openStageDetail(', '  // A tab click or a g jump')
state = between('  property var reviewViews:', '  function revealDetail(')
state += between('  property int expandedStageId:', '  property int selectedStageIndex:')
state += between('  readonly property var displayedStages:', '  readonly property bool editValid:')
delegate = editor_between('              readonly property var modelData: view.displayedStages[index]',
                          '              width: stageList.width')
model = editor_between('            model: view.displayedStages.length', '            delegate: Rectangle {')
content = editor_between('              Column {\n                id: stageContent', '              Loader {\n                id: stageEditor')
shortcut = between('              } else if (event.key === Qt.Key_Return', '                event.accepted = true\n              }\n            }')
shortcut = shortcut.replace('              } else if', '              if', 1) + '                event.accepted = true\n              }\n'
shortcut = shortcut.replace('planEditor.stageList', 'stageList')
# The keyHandler's stage detail branch comes before the shortcut, as in Panel.qml.
detail_keys = between('          } else if (root.stageDetailOpen', '          } else if (features.featurePageOpen')
detail_keys = detail_keys.replace('          } else if', '          if', 1)
keys = detail_keys + '          } else {\n' + shortcut + '          }\n'
# Panel.qml's own page instance. The StackView sizes and shows it in the panel; the
# fixture lays it out under the Plan card and shows it while a stage is open.
page = instance(panel, 'StageDetailPage')
assert '        visible: false\n' in page
page = page.replace('        visible: false\n', '        visible: root.expandedStageId !== -1\n        anchors.fill: parent\n', 1)

def card_scope(fragment):
    fragment = re.sub(r'\bview\.', 'root.', fragment)
    fragment = fragment.replace('root.detailRevealed(', 'panelScroll.reveal(')
    fragment = fragment.replace('root.leaveRequested()', 'keyHandler.forceActiveFocus()')
    # PlanView forwards the editor's stageOpened(index) to Panel.qml's openStageDetail.
    fragment = fragment.replace('root.stageOpened(stageRow.index)', 'root.openStageDetail(stageRow.index)')
    return fragment

fixture = (repo / 'tests/stage_details_fixture.qml.in').read_text()
fixture = fixture.replace('import QtTest', 'import QtTest\nimport "components/PanelDetails.js" as PanelDetails'
                          '\nimport "components/ReviewPresentation.js" as ReviewPresentation'
                          '\nimport "components/PanelNavigation.js" as PanelNavigation'
                          '\nimport "components/StagePresentation.js" as StagePresentation')
fixture = fixture.replace('// PANEL_HELPERS', helpers).replace('// PANEL_STAGE_STATE', state)
fixture = fixture.replace('// PANEL_STAGE_CONTENT', card_scope(content))
fixture = fixture.replace('// PANEL_STAGE_DELEGATE', card_scope(delegate))
fixture = fixture.replace('// PANEL_STAGE_MODEL', card_scope(model)).replace('// PANEL_STAGE_KEYS', keys)
fixture = fixture.replace('// PANEL_STAGE_DETAIL_PAGE', page)
fixture = fixture.replace('Style.space(', 'style.space(').replace('Quickshell.clipboardText', 'clipboard.clipboardText')
with tempfile.TemporaryDirectory(prefix='forge-stage-details-') as directory:
    path = Path(directory)
    (path / 'components').mkdir()
    for name in ('StageDetailPage.qml', 'StageProse.qml', 'PanelViewButton.qml', 'CompactDetail.qml',
                 'DetailFields.qml', 'DetailText.js', 'PanelDetails.js', 'ReviewView.js', 'ReviewPresentation.js',
                 'ModelRouting.js', 'UsageFormat.js', 'PlanEdit.js', 'PanelNavigation.js', 'StagePresentation.js'):
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
