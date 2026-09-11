import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
const read = name => readFileSync(new URL('../quickshell/' + name, import.meta.url), 'utf8');
const text = {}, view = {};
vm.runInNewContext(read('DetailText.js'), text);
vm.runInNewContext(read('DetailView.js'), view);
const model = () => ({
  rows: [], changes: [], get count() { return this.rows.length; },
  get(i) { return this.rows[i]; },
  append(row) { this.rows.push(row); this.changes.push(['append', row.entryKey]); },
  setProperty(i, role, value) { this.rows[i][role] = value; this.changes.push([i, role]); },
  clear() { this.rows = []; this.changes.push(['clear']); }
});
const page = (entries = [], extra = {}) => ({ version: 1, project: '/a', session: 's1',
  entries, next_cursor: entries.length, more: false, ...extra });
const record = (id, value = '\n  original\r\nnext  \t', kind = 'message') => ({ id, text: value, kind });

test('preview handles LF, CRLF and CR without trimming the chosen line or modifying the source', () => {
  for (const ending of ['\n', '\r\n', '\r']) {
    const value = ending + ' \t' + ending + '  界🙂 <b>plain</b>  \t' + ending + 'last  ';
    assert.equal(text.preview(value), '  界🙂 <b>plain</b>  \t');
    assert.equal(text.copySource(value), value);
  }
  for (const value of ['', ' \n\r\n\r\t ', null, undefined]) assert.equal(text.preview(value), '');
  const long = 'x'.repeat(100000);
  assert.equal(text.preview(long), long, 'elision is a width decision in QML');
  assert.equal(text.copySource(' \n\t '), ' \n\t ');
});

test('layout guards bound one long line without narrowing what is readable or copyable', () => {
  const line = 'i'.repeat(200000);
  // The slice is a starting estimate only: it always renders a prefix, never
  // more than the limit, and never ends on a lone high surrogate.
  for (const width of [0, 40, 400, 680, 2000, 6000]) {
    for (const size of [8, 10, 14, 24]) {
      const limit = text.initialLimit(width, size);
      assert.ok(limit > width / (size * 0.6), `${width}px at ${size}px font`);
      const shown = text.renderable(line, limit);
      assert.ok(shown.length <= limit);
      assert.equal(line.startsWith(shown), true);
    }
  }
  assert.equal(text.initialLimit(-50, 0) >= 0, true, 'a degenerate width still yields a limit');
  assert.equal(text.renderable('short line', text.initialLimit(680, 10)), 'short line');
  assert.equal(text.renderable('a\u{1f642}'.repeat(200000), 41).endsWith('\ud83d'), false);
  // Growth must reach the whole line in a small number of layouts, so no
  // character count decides the preview for zero-width or combining text.
  const invisible = 'a' + '\u200b'.repeat(1000) + 'VISIBLE SUFFIX';
  let limit = text.initialLimit(400, 10), steps = 0;
  while (limit < invisible.length) {
    const grown = text.grownLimit(invisible, limit);
    assert.ok(grown > limit, 'growth always makes progress');
    limit = grown;
    assert.ok(++steps < 12, 'growth terminates quickly');
  }
  assert.equal(text.renderable(invisible, limit), invisible);
  assert.equal(text.grownLimit(invisible, limit), invisible.length, 'growth stops at the line');
  assert.equal(text.grownLimit(line, 0), 256, 'a zero limit still advances');
  // Word wrapping is kept for ordinary text and dropped for unbroken long lines.
  for (const value of ['', ' \n\t ', 'x'.repeat(1000), ('y'.repeat(900) + '\r\n').repeat(50), null])
    assert.equal(text.wrapsWords(value), true);
  for (const value of ['x'.repeat(1001), 'ok\n' + 'x'.repeat(4000) + '\nok', 'ok\r' + 'x'.repeat(4000)])
    assert.equal(text.wrapsWords(value), false);
  assert.equal(text.copySource(line).length, line.length, 'guards never change the source');
});

test('feed assembles complete pages, deduplicates identities, and polls the advancing terminal cursor', () => {
  const feed = view.newFeed(), rows = model();
  let request = view.begin(feed, '/a', 2, 'agent');
  let result = view.finish(feed, request, '/a', 2, 'agent', page([record('s1:0'), record('s1:1')], {next_cursor: 2, more:true}));
  assert.equal(result.reset, true);
  view.reconcile(rows, result.entries, '/a', feed.session);
  request = view.begin(feed, '/a', 2, 'agent');
  assert.equal(request.cursor, 2);
  assert.equal(request.session, 's1');
  result = view.finish(feed, request, '/a', 2, 'agent', page([record('s1:1'), record('s1:2', 'final\n\n')], {next_cursor: 3}));
  view.reconcile(rows, result.entries, '/a', feed.session);
  assert.equal(rows.count, 3);
  assert.equal(rows.get(0).originalText, '\n  original\r\nnext  \t');
  assert.equal(rows.get(2).originalText, 'final\n\n');
  // Transition to idle invalidates the old request, but still fetches final output.
  view.invalidate(feed);
  request = view.begin(feed, '/a', 2, '');
  assert.equal(request.cursor, 3);
  result = view.finish(feed, request, '/a', 2, '', page([], {next_cursor: 3}));
  assert.equal(result.reset, false);
  assert.equal(view.begin(feed, '/a', 2, '').cursor, 3);
});

test('request ownership guards project, revisit revision, invocation, feed session and generation', () => {
  for (const change of ['project', 'revision', 'invocation', 'session', 'generation']) {
    const feed = view.newFeed(), old = view.begin(feed, '/a', 1, 'agent');
    const project = change === 'project' ? '/b' : '/a';
    const revision = change === 'revision' ? 3 : 1;
    const invocation = change === 'invocation' ? 'new' : 'agent';
    if (change === 'session') feed.session = 'other';
    view.invalidate(feed);
    const current = view.begin(feed, project, revision, invocation);
    assert.equal(view.finish(feed, old, project, revision, invocation, page()), null, change);
    assert.equal(feed.pending, current, 'old response cannot clear new pending state');
    assert.equal(view.finish(feed, current, project, revision, invocation, page([], {project, session:'new'})).reset, true);
    assert.equal(feed.pending, null);
  }
  const feed = view.newFeed(), request = view.begin(feed, '/a', 1, 'agent');
  assert.equal(view.finish(feed, request, '/a', 1, 'agent', page([], {project:'/b'})), null);
  assert.equal(feed.cursor, 0);
});

test('duplicate-looking and rolling history preserves row objects and expansion; filtering only changes visibility', () => {
  const rows = model();
  const records = [record('source:0', 'same', 'error'), record('source:50', 'same', 'git')];
  records.forEach(r => { r.goal = 'goal'; r.t = '12:00:00'; });
  view.reconcile(rows, records, '/a', 'history');
  const first = rows.get(0), second = rows.get(1);
  view.expand(rows, first.entryKey, true);
  rows.changes = [];
  view.reconcile(rows, structuredClone(records), '/a', 'history');
  assert.deepEqual(rows.changes, [], 'identical polls never rebind original strings');
  view.reconcile(rows, [records[1], record('source:100', 'same')], '/a', 'history');
  assert.equal(rows.get(0), first);
  assert.equal(rows.get(1), second);
  assert.equal(first.expanded, true);
  assert.equal(second.expanded, false);
  view.filterRows(rows, 'git');
  assert.equal(first.shown, false);
  assert.equal(second.separator, 'goal');
  view.filterRows(rows, 'all');
  assert.equal(first.shown, true);
  assert.equal(first.expanded, true);
  assert.equal(first.separator, 'goal');
  assert.equal(second.separator, '');
  view.reconcile(rows, [record('replacement:0', 'same')], '/a', 'history');
  assert.equal(rows.get(3).expanded, false);
  assert.notEqual(view.key('/a', 's1', '0'), view.key('/b', 's1', '0'));
  assert.notEqual(view.key('/a', 's1', '0'), view.key('/a', 's2', '0'));
});

test('legacy re-fetches whole source and defers updates while selected content is expanded', () => {
  const feed = view.newFeed(), rows = model();
  const request = view.begin(feed, '/a', 1, '');
  view.finish(feed, request, '/a', 1, '', page([record('legacy:0', 'old\n', 'legacy')], {session:'legacy', legacy:true, next_cursor:1}));
  assert.equal(view.begin(feed, '/a', 1, '').cursor, 0);
  view.reconcile(rows, [record('legacy:0', 'old\n', 'legacy')], '/a', 'legacy');
  const row = rows.get(0);
  view.expand(rows, row.entryKey, true);
  view.reconcile(rows, [record('legacy:0', 'old\nnew\n', 'legacy')], '/a', 'legacy');
  assert.equal(row.originalText, 'old\n');
  view.expand(rows, row.entryKey, false);
  assert.equal(row.originalText, 'old\nnew\n');
  view.expand(rows, row.entryKey, true);
  view.reconcile(rows, [record('legacy:0', '', 'legacy')], '/a', 'legacy');
  assert.equal(row.originalText, 'old\nnew\n');
  view.expand(rows, row.entryKey, false);
  assert.equal(row.originalText, '', 'an empty update is still an exact original value');
});

test('Panel integration resets only scoped models and rejects stale callbacks before displaying errors', () => {
  const panel = read('Panel.qml');
  const ctx = { DetailView: view, window: {visible:true}, logFeed:view.newFeed(), lastProject:'/a',
    projectViewRevision:1, agentSession:'agent', encodeURIComponent,
    liveEntries:model(), liveOutput:{beginUpdate(){}, endUpdate(){}, resetView(){}},
    calls:[], Qt:{callLater(){}}, api(method,path,body,done,scoped) { this.calls.push({path,done,scoped}); } };
  ctx.root = ctx;
  // api is called as a global function in QML.
  ctx.api = (method,path,body,done,scoped) => ctx.calls.push({path,done,scoped});
  vm.runInNewContext(panel.slice(panel.indexOf('  function refreshAgentLog()'), panel.indexOf('  function act(')), ctx);
  ctx.refreshAgentLog();
  const old = ctx.calls[0];
  assert.match(old.path, /\/api\/agent_records\?cursor=0&project=%2Fa/);
  assert.equal(old.scoped, true);
  old.done(page([record('s1:0')]));
  view.expand(ctx.liveEntries, ctx.liveEntries.get(0).entryKey, true);
  ctx.refreshAgentLog();
  assert.match(ctx.calls[1].path, /cursor=1.*session=s1/);
  ctx.calls[1].done(page([record('s2:0', 'new session')], {reset:true, session:'s2', next_cursor:1}));
  assert.equal(ctx.liveEntries.count, 1);
  assert.equal(ctx.liveEntries.get(0).expanded, false);
  ctx.refreshAgentLog();
  view.invalidate(ctx.logFeed);
  ctx.agentSession = '';
  ctx.refreshAgentLog();
  ctx.calls[2].done({error:'stale failure'});
  assert.notEqual(ctx.logError, 'stale failure');
  assert.ok(ctx.logFeed.pending);
  ctx.calls[3].done(page([record('s2:1', 'terminal')], {session:'s2', next_cursor:2}));
  assert.equal(ctx.liveEntries.get(1).originalText, 'terminal');
  assert.match(panel, /onCopyRequested: original => Quickshell.clipboardText = original/);
});

test('leaving and revisiting a project resets both lists and strands the earlier visit responses', () => {
  const panel = read('Panel.qml');
  const slice = (from, to) => panel.slice(panel.indexOf(from), panel.indexOf(to));
  const view0 = { beginUpdate(){}, endUpdate(){}, resetView(){ this.reset = true; },
    positionViewAtBeginning(){}, followTail: true, readingY: 0 };
  const ctx = { DetailView: view, window: {visible:true}, encodeURIComponent,
    liveEntries: model(), historyEntries: model(),
    liveOutput: {...view0}, historyList: {...view0}, reportList: {...view0},
    chatList: {...view0}, goalFlick: {contentY: 0}, goalField: {text: 'draft'},
    goalEnhancePending: false, goalEnhanceRequest: -1, goalEnhanceSent: '',
    goalEnhanceReady: '', goalEnhanceUndo: '', goalEnhanceError: '',
    feedbackField: {text: ''}, questionField: {text: ''}, goalDrafts: {},
    cancelPlanEdit(){}, syncReviewViews(){}, calls: [], Qt: {callLater(){}},
    logFeed: view.newFeed(), logError: '', lastProject: '/a', projectViewRevision: 1,
    agentSession: 'agent', busy: false, historyFilter: 'errors', engineState: null };
  ctx.root = ctx;
  ctx.api = (method, path, body, done, scoped) => ctx.calls.push({path, done, scoped});
  vm.runInNewContext([
    slice('  function syncHistory()', '  onHistoryFilterChanged:'),
    slice('  function goalEnhancementAction(', '  function revisePlan('),
    slice('  function refreshAgentLog()', '  function act('),
    slice('  onEngineStateChanged: {', '  onBusyChanged: {')
      .replace('onEngineStateChanged: {', 'function engineStateChanged() {'),
  ].join('\n'), ctx);

  const entry = id => ({id, text: 'goal line\ndetail', kind: 'git', goal: 'g', t: '12:00:00'});
  ctx.engineState = {project: '/a', history: [entry('history:src:0')]};
  ctx.engineStateChanged();
  assert.equal(ctx.historyEntries.count, 1);
  ctx.refreshAgentLog();
  const stranded = ctx.calls[0];
  view.expand(ctx.historyEntries, ctx.historyEntries.get(0).entryKey, true);

  ctx.engineState = {project: '/b', history: [entry('history:other:0')]};
  ctx.engineStateChanged();
  assert.equal(ctx.projectViewRevision, 2);
  assert.equal(ctx.goalDrafts['/a'], 'draft', 'the previous goal draft is kept');
  assert.equal(ctx.historyFilter, 'all');
  assert.equal(ctx.liveOutput.reset && ctx.historyList.reset, true);
  assert.equal(ctx.historyEntries.count, 1);
  assert.equal(ctx.historyEntries.get(0).expanded, false, 'expansion never moves to another project');
  assert.equal(ctx.historyEntries.get(0).entryKey.includes('/b'), true);

  stranded.done(page([record('s1:0')], {project: '/a'}));
  assert.equal(ctx.liveEntries.count, 0, 'an earlier visit cannot append to the new project');
  assert.equal(ctx.logError, '');

  // Revisiting /a starts a new view: the old identities and cursor are gone.
  ctx.engineState = {project: '/a', history: [entry('history:src:0')]};
  ctx.engineStateChanged();
  assert.equal(ctx.projectViewRevision, 3);
  assert.equal(ctx.logFeed.cursor, 0);
  assert.equal(ctx.logFeed.session, '');
  ctx.refreshAgentLog();
  ctx.calls[1].done(page([record('s1:0')]));
  assert.equal(ctx.liveEntries.count, 1);
  // A same-project state poll only reconciles; it never resets a live reading view.
  ctx.liveOutput.reset = false;
  ctx.historyList.reset = false;
  ctx.engineState = {project: '/a', history: [entry('history:src:0'), entry('history:src:40')]};
  ctx.engineStateChanged();
  assert.equal(ctx.projectViewRevision, 3);
  assert.equal(ctx.historyEntries.count, 2);
  assert.equal(ctx.liveOutput.reset || ctx.historyList.reset, false);
  assert.equal(ctx.liveEntries.count, 1);
});
