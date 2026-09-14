"""Run current Panel stage-4 subtrees in QtTest, isolated from shell/network state.
The extracted bindings, controls, collections and scroll handlers are unchanged.
Only the host, theme, clipboard and API actions are fixture services.
"""
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

repo = Path(__file__).resolve().parents[1]
panel = (repo / 'quickshell/Panel.qml').read_text()
architecture_view = (repo / 'quickshell/ArchitectureReviewView.qml').read_text()
catalogue_editor = (repo / 'quickshell/CatalogueEditor.qml').read_text()
reports_view = (repo / 'quickshell/ReportsView.qml').read_text()

def between(start, end):
    at = panel.index(start)
    return panel[at:panel.index(end, at)]

def block(source, marker, kind):
    at = source.index(marker)
    start = source.rfind(kind + ' {', 0, at)
    # QML strings and comments may contain braces. Track them while extracting.
    i = source.index('{', start)
    depth = 0
    quote = None
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
        elif c in '\"\'`':
            quote = c
        elif c == '{':
            depth += 1
        elif c == '}':
            depth -= 1
            if depth == 0:
                return source[start:i+1]
        i += 1
    raise ValueError(marker)

helpers = between('  function revealDetail(', '  function reviewScope(')
helpers += between('  function canMoveQueueGoal(', '  readonly property string phase:')
helpers += between('  property var reportIdentityCache:', '  property var reviewViews:')
helpers += between('  component CadenceButton:', '\n}')
helpers += between('  readonly property var planReview:', '  property bool chooserOpen:')
fragments = {
    'PLAN_REVIEW': block(architecture_view, 'id: planReviewSection', 'Column'),
    'CADENCE': 'CadenceButton { objectName: "architectCadence"; role: "architect" }\nCadenceButton { objectName: "reviewerCadence"; role: "reviewer" }',
    'LOCAL_ERROR': block(panel, 'objectName: "localErrorDetail"', 'PanelDetail'),
    'ARCHITECTURE': block(architecture_view, 'objectName: "architectureDetails"', 'ArchitectureDetails'),
    'PROVIDERS': block(panel, 'objectName: "providerDetails"', 'PanelFields'),
    'OPTIONS': block(catalogue_editor, 'objectName: "catalogueOptions"', 'ViewFields'),
    'SOURCES': block(catalogue_editor, 'objectName: "catalogueSources"', 'ViewFields'),
    'METADATA': block(catalogue_editor, 'objectName: "catalogueMetadata"', 'ViewFields'),
    'CHAT': block(panel, 'id: chatList', 'Rectangle'),
    'REPORT': block(reports_view, 'id: reportList', 'ListView'),
    'QUEUE': block(panel, 'id: queueSection', 'Rectangle'),
}
fragments['PLAN_REVIEW'] = re.sub(r'\bview\.', 'root.', fragments['PLAN_REVIEW'])
fragments['PLAN_REVIEW'] = fragments['PLAN_REVIEW'].replace('root.planReviewExpansionRequested(expanded)', 'root.planReviewExpanded = expanded')
fragments['PLAN_REVIEW'] = fragments['PLAN_REVIEW'].replace('root.planReviewLoadRequested()', 'root.loadPlanReviewRequests()')
fragments['PLAN_REVIEW'] = fragments['PLAN_REVIEW'].replace('root.leaveRequested()', 'keyHandler.forceActiveFocus()')
fragments['PLAN_REVIEW'] = fragments['PLAN_REVIEW'].replace('root.detailRevealed(control)', 'root.revealDetail(control)')
fragments['PLAN_REVIEW'] = fragments['PLAN_REVIEW'].replace('root.detailInspected(', 'root.inspectDetail(')
fragments['PLAN_REVIEW'] = fragments['PLAN_REVIEW'].replace('root.fontSize11', 'root.fs(11)').replace('root.fontSize12', 'root.fs(12)')
fragments['ARCHITECTURE'] = re.sub(r'\bview\.', 'root.', fragments['ARCHITECTURE'])
fragments['ARCHITECTURE'] = fragments['ARCHITECTURE'].replace('root.leaveRequested()', 'keyHandler.forceActiveFocus()')
fragments['ARCHITECTURE'] = fragments['ARCHITECTURE'].replace('root.detailRevealed(control)', 'root.revealDetail(control)')
fragments['ARCHITECTURE'] = fragments['ARCHITECTURE'].replace('root.detailInspected(', 'root.inspectDetail(')
fragments['ARCHITECTURE'] = fragments['ARCHITECTURE'].replace('root.detailScope',
    'JSON.stringify([root.lastProject, root.projectViewRevision, (root.plan || {}).plan_id || ""])')
fragments['ARCHITECTURE'] = fragments['ARCHITECTURE'].replace('root.fontSize12', 'root.fs(12)')
for name in ('OPTIONS', 'SOURCES', 'METADATA'):
    fragments[name] = fragments[name].replace('ViewFields {', 'PanelFields {')
    fragments[name] = re.sub(r'\bview\.', 'root.', fragments[name])
    fragments[name] = fragments[name].replace('root.details', 'root.catalogueDetails')
report = fragments['REPORT']
report = '\n'.join(line for line in report.splitlines() if not line.startswith('  required property '))
for name in ('reportsVisible', 'reportIndex', 'projectViewRevision', 'selectedReportKey', 'expandedReportKey',
             'now', 'detailScope', 'foreground', 'mutedForeground', 'background', 'surface', 'accent', 'urgent',
             'fontFamily', 'fontSize10', 'fontSize11'):
    report = report.replace('reportList.' + name, 'root.' + name)
report = report.replace('reportList.reportSelected(reportRow.key)', 'root.selectedReportKey = reportRow.key')
report = report.replace('reportList.reportExpansionRequested(reportRow.expanded ? "" : reportRow.key)',
                        'root.expandedReportKey = reportRow.expanded ? "" : reportRow.key')
report = report.replace('reportList.leaveRequested()', 'keyHandler.forceActiveFocus()')
report = report.replace('reportList.detailRevealed(control)', 'root.revealDetail(control)')
report = report.replace('reportList.detailInspected(fields)', 'root.inspectDetail(fields)')
report = report.replace('  id: reportList\n', '  id: reportList\n  width: root.width\n  height: 260\n')
report = report.replace('  visible: reportsVisible', '  visible: root.reportsVisible')
report = report.replace('root.now', 'root.agentNow')
report = report.replace('root.detailScope', 'JSON.stringify([root.lastProject, root.projectViewRevision, root.plan.plan_id || ""])')
report = report.replace('root.fontSize10', 'root.fs(10)').replace('root.fontSize11', 'root.fs(11)')
report = report.replace('  onReportIndexChanged: sync()\n  onProjectViewRevisionChanged: {\n    reportEntries.clear()\n    readingKey = ""\n    sync()\n  }',
    '  Connections {\n    target: root\n    function onReportIndexChanged() { reportList.sync() }\n'
    '    function onProjectViewRevisionChanged() { reportEntries.clear(); reportList.readingKey = ""; reportList.sync() }\n  }')
fragments['REPORT'] = report
fixture = (repo / 'tests/panel_details_fixture.qml.in').read_text().replace('// PANEL_HELPERS', helpers)
for name, fragment in fragments.items():
    fixture = fixture.replace('// PANEL_' + name, fragment)
fixture = fixture.replace('Style.space(', 'style.space(').replace('Quickshell.clipboardText', 'clipboard.clipboardText')
with tempfile.TemporaryDirectory(prefix='forge-panel-details-') as directory:
    path = Path(directory)
    (path / 'components').mkdir()
    for name in ('ArchitectureDetails.qml', 'CompactDetail.qml', 'DetailFields.qml', 'DetailText.js', 'PanelDetails.js', 'PlanReview.js',
                 'ModelRouting.js', 'ReportFormat.js', 'UsageFormat.js', 'CataloguePresentation.js'):
        shutil.copyfile(repo / 'quickshell' / name, path / 'components' / name)
    button = (repo / 'quickshell' / 'PanelViewButton.qml').read_text()
    button = button.replace('import qs.Commons\n', '')
    button = button.replace('Style.space(18)', '18').replace('Style.space(10)', '10')
    (path / 'components' / 'PanelViewButton.qml').write_text(button)
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
