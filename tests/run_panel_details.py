"""Run current Panel stage-4 subtrees in QtTest, isolated from shell/network state.
The extracted bindings, controls, collections and scroll handlers are unchanged.
Only the host, theme, clipboard and API actions are fixture services.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

repo = Path(__file__).resolve().parents[1]
panel = (repo / 'quickshell/Panel.qml').read_text()

def between(start, end):
    at = panel.index(start)
    return panel[at:panel.index(end, at)]

def block(marker, kind):
    at = panel.index(marker)
    start = panel.rfind(kind + ' {', 0, at)
    # QML strings and comments may contain braces. Track them while extracting.
    i = panel.index('{', start)
    depth = 0
    quote = None
    while i < len(panel):
        c = panel[i]
        if quote:
            if c == '\\':
                i += 2
                continue
            if c == quote:
                quote = None
        elif panel.startswith('//', i):
            i = panel.index('\n', i)
            continue
        elif c in '\"\'`':
            quote = c
        elif c == '{':
            depth += 1
        elif c == '}':
            depth -= 1
            if depth == 0:
                return panel[start:i+1]
        i += 1
    raise ValueError(marker)

helpers = between('  function revealDetail(', '  function reviewScope(')
helpers += between('  function stageModelText(', '  function changeModelConstraint(')
helpers += between('  function architectUsageText(', '  function reportLifecycleText(')
helpers += between('  function catalogueProviderText(', '  function openCatalogue(')
helpers += between('  function nonNegativeInt(', '  property var reviewViews:')
helpers += between('  function reportTime(', '  function chooserRows(')
helpers += between('  component PanelButton:', '\n}')
fragments = {
    'LOCAL_ERROR': block('objectName: "localErrorDetail"', 'PanelDetail'),
    'ARCHITECTURE': block('objectName: "architectureDetails"', 'PanelFields'),
    'PROVIDERS': block('objectName: "providerDetails"', 'PanelFields'),
    'OPTIONS': block('objectName: "catalogueOptions"', 'PanelFields'),
    'SOURCES': block('objectName: "catalogueSources"', 'PanelFields'),
    'METADATA': block('objectName: "catalogueMetadata"', 'PanelFields'),
    'CHAT': block('id: chatList', 'Rectangle'),
    'REPORT': block('id: reportList', 'ListView'),
    'QUEUE': block('id: queueSection', 'Rectangle'),
}
fixture = (repo / 'tests/panel_details_fixture.qml.in').read_text().replace('// PANEL_HELPERS', helpers)
for name, fragment in fragments.items():
    fixture = fixture.replace('// PANEL_' + name, fragment)
fixture = fixture.replace('Style.space(', 'style.space(').replace('Quickshell.clipboardText', 'clipboard.clipboardText')
with tempfile.TemporaryDirectory(prefix='forge-panel-details-') as directory:
    path = Path(directory)
    (path / 'components').mkdir()
    for name in ('CompactDetail.qml', 'DetailFields.qml', 'DetailText.js', 'PanelDetails.js'):
        shutil.copyfile(repo / 'quickshell' / name, path / 'components' / name)
    (path / 'tst_panel.qml').write_text(fixture)
    runtime = path / 'runtime'
    runtime.mkdir(mode=0o700)
    env = dict(os.environ, QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software',
               QT_QPA_PLATFORMTHEME='', QT_QUICK_CONTROLS_STYLE='Basic',
               GSETTINGS_BACKEND='memory', XDG_RUNTIME_DIR=str(runtime))
    result = subprocess.run(['/usr/lib/qt6/bin/qmltestrunner', '-input', str(path)],
                            env=env, capture_output=True, text=True, timeout=60)
    output = result.stdout + result.stderr
    print(output, end='')
    # Warnings in an extracted production subtree are errors, not runtime evidence.
    raise SystemExit(result.returncode or ('QWARN' in output) or ('FAIL!' in output))
