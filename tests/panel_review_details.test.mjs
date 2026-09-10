import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const read = n => readFileSync(new URL('../quickshell/' + n, import.meta.url), 'utf8');
const review = {};
vm.runInNewContext(read('ReviewView.js'), review);
const full = (id, extra = {}) => ({id, round:1, role:'architect', approved:false,
  summary:'\n identical first line\r\n' + '界🙂'.repeat(5000) + '\nsummary END  \t',
  issues:['\nissue 1\n' + 'x'.repeat(6000) + ' END  ', 'second issue\r\nlast'],
  notes:['legacy note\nlast  '], checks:['check one\nlast  ', 'check two'], ...extra});
const preview = v => ({round:v.round, role:v.role, approved:v.approved, truncated:true,
  summary:v.summary.slice(0,240), issues:[v.issues[0].slice(0,100)]});
const stage = records => ({id:3, reviews:records.slice(-8).map(preview), review_count:records.length,
  reviews_truncated:true, review_snapshot:'history-1'});
const plan = {plan_id:'plan-a', revision:5, architecture:{checkpoint:'cp-1'}};
const scope = (s, p = plan, visit = 1, project = '/a') => review.scope(project,visit,p,s);
const page = (s, items, cursor, extra = {}) => ({project:s.project, plan_id:s.plan || null,
  revision:s.revision, stage_id:s.stage, checkpoint:s.checkpoint || null, snapshot:s.snapshot,
  count:s.count, items, next_cursor:cursor + items.length < s.count ? cursor + items.length : null, ...extra});

test('large identical previews, shared role rounds and legacy positions retrieve exact complete records', () => {
  const records = Array.from({length:13}, (_,i) => full('review-'+i, {role:i%2?'reviewer':'architect',
    issues:['request '+i+'\n'+'y'.repeat(5000),'second '+i]}));
  delete records[11].id; delete records[12].id;
  const s = scope(stage(records)), view = review.create(s);
  assert.equal(view.rows[0].position,5);
  assert.equal(new Set(view.rows.map(r=>r.key)).size,8);
  for (let i = 5; i < 13; i++) {
    const req = review.begin(view,i,i+1);
    const url = review.path(view, req);
    for (const param of ['project=%2Fa','plan_id=plan-a','checkpoint=cp-1','stage_id=3','cursor='+i,'limit=1']) assert.ok(url.includes(param),url);
    assert.ok(!url.includes('revision='));
    assert.equal(review.finish(view,req,s,page(s,[records[i]],i),200),'complete');
    const row = view.rows.find(r=>r.position===i);
    assert.equal(row.complete,true);
    assert.deepEqual(row.verdict,records[i]);
    assert.ok(row.verdict.summary.endsWith('summary END  \t'));
  }
  const req = review.begin(view,0,5);
  // The server can stop early at its byte budget: continue with next_cursor.
  assert.equal(review.finish(view,req,s,page(s,records.slice(0,2),0),200),'more');
  assert.equal(view.retry.cursor,2);
  const next = review.begin(view,view.retry.cursor,view.retry.end);
  assert.equal(review.finish(view,next,s,page(s,records.slice(2,5),2),200),'complete');
  assert.equal(view.older,0);
  assert.deepEqual(Array.from(view.rows,r=>r.verdict),records);
});

test('durable identity mismatches reject entire pages without attaching full content', () => {
  const s = scope({id:3,reviews:[full('a')],review_snapshot:'history-1'}), view = review.create(s);
  const req = review.begin(view,0,1);
  assert.equal(review.finish(view,req,s,page(s,[full('b')],0),200),'failed');
  assert.equal(view.rows[0].verdict.id,'a');
  assert.match(view.error,/identity/);
  assert.notEqual(review.identity({identity:{plan_id:'p', revision:1, stage_id:3, attempt_id:'a', role:'architect', round:1}}),
    review.identity({identity:{plan_id:'p', revision:1, stage_id:3, attempt_id:'a', role:'reviewer', round:1}}));
});

test('all scoped stale successes and failures are inert, including leaving and revisiting', () => {
  const st = stage([full('a')]), s = scope(st);
  for (const changed of [scope(st,plan,2),scope(st,plan,1,'/b'), scope(st,{...plan,revision:6}),
    scope(st,{...plan,plan_id:'b'}),scope(st,{...plan,architecture:{checkpoint:'cp-2'}}),scope({...st,review_snapshot:'new'})]) {
    for (const status of [200,500]) {
      const view=review.create(s), req=review.begin(view,0,1), rows=view.rows;
      assert.equal(review.finish(view,req,changed,page(s,[full('a')],0),status),'stale');
      assert.equal(view.rows,rows); assert.equal(view.error,''); assert.equal(view.pending,req);
    }
  }
});

test('checkpoint-free publication changes with equal revision/count/previews cannot combine pages', () => {
  const records=Array.from({length:10},(_,i)=>full('r'+i));
  const st=stage(records), s=scope(st,{plan_id:'plan-a',revision:5}), view=review.create(s);
  let req=review.begin(view,0,2);
  assert.equal(review.finish(view,req,s,page(s,records.slice(0,1),0),200),'more');
  req=review.begin(view,1,2);
  const before=view.rows;
  assert.equal(review.finish(view,req,s,page(s,[full('replaced')],1,{snapshot:'history-2'}),200),'changed');
  assert.equal(view.rows,before);
  const changed=scope({...st,review_snapshot:'history-2'},{plan_id:'plan-a',revision:5});
  assert.notEqual(changed.key,s.key);
  const restarted=review.create(changed);
  assert.equal(restarted.rows[0].position,2);
  assert.ok(restarted.rows.every(r=>!r.complete));
  const noToken=scope({...st,review_snapshot:''},{revision:5});
  const unsafe=review.create(noToken); req=review.begin(unsafe,2,3);
  assert.equal(review.finish(unsafe,req,noToken,page(noToken,[records[2]],2),200),'changed');
});

test('legacy last_verdict remains complete and uses historical fallback round only while idle', () => {
  const verdict = full(undefined);
  const idle=review.create(scope({id:3,last_verdict:{...verdict,round:undefined}, rounds:4,status:'committed'}));
  assert.equal(idle.rows[0].complete,true); assert.equal(idle.rows[0].round,4);
  const active=review.create(scope({id:3,last_verdict:{...verdict,round:undefined},rounds:5,status:'in_progress'}));
  assert.equal(active.rows[0].round,null);
});

test('errors keep preview incomplete, retry works, invalid pages never offer full copy sources', () => {
  const records=[full('a')], s=scope(stage(records)), view=review.create(s);
  let req=review.begin(view,0,1);
  assert.equal(review.finish(view,req,s,{error:'offline'},500),'failed');
  assert.equal(view.rows[0].complete,false); assert.equal(view.retry.cursor,0);
  req=review.begin(view,0,1);
  assert.equal(review.finish(view,req,s,page(s,[preview(records[0])],0),200),'failed');
  req=review.begin(view,0,1);
  assert.equal(review.finish(view,req,s,page(s,records,0),200),'complete');
  assert.equal(view.rows[0].verdict.summary,records[0].summary);
});

const panel = read('Panel.qml');
function panelContext(st = stage([full('a')]), p = plan) {
  const ctx = {ReviewView:review,lastProject:'/a',projectViewRevision:1,plan:{...p,stages:[st]},
    reviewViews:{},reviewViewVersion:0,stageReviewBlocks:{},expandedStageId:-1,editingPlan:false,calls:[],refreshes:0,refresh(){this.refreshes++}};
  ctx.root=ctx;
  ctx.api=(method,path,body,done,scoped)=>ctx.calls.push({method,path,done,scoped});
  vm.runInNewContext(panel.slice(panel.indexOf('  function reviewScope('),panel.indexOf('  component StageProseField:')),ctx);
  return ctx;
}
test('stage prose identity survives publications while review identity remains snapshot scoped', () => {
  const ctx=panelContext(), st=ctx.plan.stages[0];
  const key=ctx.stageDetailScope(st), reviewKey=ctx.reviewScope(st).key;
  ctx.plan={...ctx.plan,architecture:{checkpoint:'cp-2'}};
  assert.equal(ctx.stageDetailScope(st),key);
  assert.notEqual(ctx.reviewScope(st).key,reviewKey);
  const publishedReviewKey=ctx.reviewScope(st).key;
  const appended={...st,review_count:st.review_count+1,review_snapshot:'history-2',reviews:[...st.reviews,preview(full('b'))]};
  ctx.plan.stages=[appended];
  assert.equal(ctx.stageDetailScope(appended),key);
  assert.notEqual(ctx.reviewScope(appended).key,publishedReviewKey);
  for (const mutate of [c=>c.lastProject='/b',c=>c.projectViewRevision++,
    c=>c.plan.plan_id='other',c=>c.plan.revision++]) {
    const changed=panelContext(); mutate(changed);
    assert.notEqual(changed.stageDetailScope(st),key);
  }
  assert.notEqual(ctx.stageDetailScope({...st,id:4}),key);
  const card=panel.slice(panel.indexOf('id: stageRow'),panel.indexOf('id: stageEditor'));
  assert.match(card,/detailScope: root.stageDetailScope\(modelData\)/);
  assert.ok(!card.includes("reviewDetailScope"));
  assert.equal((card.match(/detailKey: stageRow.detailScope/g)||[]).length,0);
  assert.equal((card.match(/detailKey: stageRow.reviewDetailScope/g)||[]).length,0);
});
test('actual Panel handlers load lazily, cache completed text, restart changed snapshots and discard stale callbacks', () => {
  const ctx=panelContext(), st=ctx.plan.stages[0];
  assert.equal(ctx.calls.length,0);
  ctx.loadStageReviews(3,0,1);
  assert.equal(ctx.calls[0].scoped,true);
  assert.ok(ctx.reviewView(st).pending);
  ctx.calls[0].done(page(scope(st),[full('a')],0),200);
  assert.equal(ctx.reviewView(st).rows[0].complete,true);
  ctx.syncReviewViews();
  assert.equal(ctx.reviewView(st).rows[0].verdict.summary,full('a').summary);
  ctx.loadStageReviews(3,0,1); const old=ctx.calls[1];
  ctx.plan={...ctx.plan,architecture:{checkpoint:'new'}};ctx.syncReviewViews();
  old.done({error:'stale failure'},500);
  assert.equal(ctx.reviewView(st).error,'');
  old.done(page(scope(st),[full('wrong')],0),200);
  assert.equal(ctx.reviewView(st).rows[0].complete,false);
  const legacy=panelContext(st,{plan_id:'plan-a',revision:5});
  legacy.loadStageReviews(3,0,1);
  legacy.calls[0].done(page(scope(st,{plan_id:'plan-a',revision:5}),[full('changed')],0,{snapshot:'changed'}),200);
  assert.equal(legacy.refreshes,1);
  assert.equal(legacy.reviewView(st).rows[0].complete,false);
});

test('stage field wiring preserves each original, full prose, review previews, locks, and header-only parent expansion', () => {
  const ctx={};
  vm.runInNewContext(panel.slice(panel.indexOf('  function reviewGateText('),panel.indexOf('  function reviewRoundLabel(')),ctx);
  const v=full('a'), fields=ctx.reviewFields(v);
  assert.deepEqual(Array.from(fields,f=>f.text),[...v.issues,...v.notes,...v.checks]);
  const card=panel.slice(panel.indexOf('id: stageRow'),panel.indexOf('id: stageEditor'));
  assert.ok(!card.includes('maximumLineCount: 3'));
  assert.match(card,/label: reviewRound.modelData.complete \? "Summary"/);
  assert.ok(!card.includes("StageDetail"));
  assert.ok(!card.includes("textComplete:"));
  assert.ok(!card.includes("detailKey:"));
  assert.match(card,/model: reviewRound.modelData.complete \? root.reviewFields/);
  assert.match(card,/editable: root.editingPlan && modelData.status !== "committed"/);
  assert.equal((card.match(/objectName: "stageToggle"/g)||[]).length,1);
  assert.equal((card.match(/objectName: "stageRoutingToggle"/g)||[]).length,1);
  assert.ok(!card.includes('TapHandler'));
  assert.match(card,/StageProseField \{/);
  assert.match(card,/active: stageRow.expanded && !stageRow.editable && stageRow.prose !== null/);
  for (const name of ['commit','instructions','acceptance']) assert.match(card,new RegExp('originalText: stageRow.prose \\? stageRow.prose.'+name));
});

test('a byte-limited older page followed by failure keeps its unfilled gap reachable', () => {
  const records=Array.from({length:20},(_,i)=>full('r'+i)),s=scope(stage(records)),view=review.create(s);
  let req=review.begin(view,4,12);
  assert.equal(review.finish(view,req,s,page(s,records.slice(4,5),4),200),'more');
  assert.equal(view.older,12,'records 5..11 are still missing');
  req=review.begin(view,5,12);
  assert.equal(review.finish(view,req,s,{error:'offline'},500),'failed');
  assert.equal(view.older,12);
  // The older action starts the same window again, so it cannot skip the gap.
  req=review.begin(view,view.older-8,view.older);
  assert.equal(review.finish(view,req,s,page(s,records.slice(4,12),4),200),'complete');
  assert.equal(view.older,4);
  assert.deepEqual(Array.from(view.rows,r=>r.position),Array.from({length:16},(_,i)=>i+4));
});


test('automatic completion gates current state and claims pending before notifications', () => {
  const records=Array.from({length:10},(_,i)=>full('r'+i));
  const ctx=panelContext(stage(records)),st=ctx.plan.stages[0];
  ctx.ensureStageReviewsLoaded(st);assert.equal(ctx.calls.length,0);
  ctx.expandedStageId=3;ctx.editingPlan=true;
  ctx.ensureStageReviewsLoaded(st);assert.equal(ctx.calls.length,0);
  ctx.editingPlan=false;ctx.lastProject='';
  ctx.ensureStageReviewsLoaded(st);assert.equal(ctx.calls.length,0);
  ctx.lastProject='/a';
  // A scheduling callback resolves the current publication, not old previews.
  ctx.plan.stages=[{...st,reviews:[records[9]],review_count:10,review_snapshot:'complete-publication'}];
  ctx.ensureStageReviewsLoaded(st);assert.equal(ctx.calls.length,0);
  ctx.plan.stages=[st];
  let version=ctx.reviewViewVersion;
  Object.defineProperty(ctx,'reviewViewVersion',{get(){return version},set(v){
    version=v;ctx.ensureStageReviewsLoaded(st);
  }});
  ctx.ensureStageReviewsLoaded(st);ctx.ensureStageReviewsLoaded(st);
  assert.equal(ctx.calls.length,1);assert.match(ctx.calls[0].path,/cursor=2&limit=8/);
  ctx.expandedStageId=-1;
  ctx.calls[0].done(page(scope(st),records.slice(2,3),2),200);
  assert.equal(ctx.calls.length,2,'more is permitted after collapse');
  assert.match(ctx.calls[1].path,/cursor=3&limit=7/);
  ctx.expandedStageId=3;ctx.ensureStageReviewsLoaded(st);
  assert.equal(ctx.calls.length,2,'pending continuation owns the remaining range');
  ctx.calls[1].done(page(scope(st),records.slice(3),3),200);
  ctx.ensureStageReviewsLoaded(st);assert.equal(ctx.calls.length,2);
});

test('failures from every load origin survive publications and explicit Retry recovers current ranges', () => {
  const records=Array.from({length:10},(_,i)=>full('r'+i));
  const ctx=panelContext(stage(records));ctx.expandedStageId=3;
  let st=ctx.plan.stages[0];const key=ctx.stageDetailScope(st);
  const publish=()=>{
    st={...st,review_snapshot:st.review_snapshot+'-next'};
    ctx.plan={...ctx.plan,architecture:{checkpoint:ctx.plan.architecture.checkpoint+'-next'},stages:[st]};
    ctx.syncReviewViews();ctx.ensureStageReviewsLoaded(st);
  };
  ctx.ensureStageReviewsLoaded(st);
  ctx.calls[0].done(page(ctx.reviewScope(st),records.slice(2,3),2),200);
  assert.equal(ctx.calls.length,2);
  ctx.calls[1].done({error:'continuation offline'},500);
  assert.equal(ctx.stageReviewBlocks[key],'continuation offline');
  publish();publish();assert.equal(ctx.calls.length,2);
  assert.equal(ctx.reviewView(st).error,'');assert.equal(ctx.reviewView(st).retry,null);
  ctx.retryStageReviews(st);assert.equal(ctx.calls.length,3);
  assert.match(ctx.calls[2].path,/cursor=2&limit=8/);
  ctx.calls[2].done({error:'retry offline'},500);
  publish();assert.equal(ctx.calls.length,3);assert.equal(ctx.stageReviewBlocks[key],'retry offline');
  ctx.retryStageReviews(st);ctx.calls[3].done(page(ctx.reviewScope(st),records.slice(2),2),200);
  assert.equal(ctx.stageReviewBlocks[key],undefined);
  ctx.loadStageReviews(3,0,2);
  ctx.calls[4].done(page(ctx.reviewScope(st),records.slice(0,1),0),200);
  ctx.calls[5].done({error:'older gap offline'},500);
  assert.equal(ctx.reviewView(st).older,2);
  assert.equal(ctx.stageReviewBlocks[key],'older gap offline');
  ctx.retryStageReviews(st);
  assert.match(ctx.calls[6].path,/cursor=1&limit=1/,'Retry uses actual remaining older gap');
  ctx.calls[6].done({error:'older retry offline'},500);
  publish();assert.equal(ctx.calls.length,7);
  ctx.retryStageReviews(st);ctx.calls[7].done(page(ctx.reviewScope(st),records.slice(2),2),200);
  assert.equal(ctx.stageReviewBlocks[key],undefined);
});

test('changed observation precedes synchronous refresh and obsolete callbacks cannot alter a newer block', () => {
  const ctx=panelContext();ctx.expandedStageId=3;
  const st=ctx.plan.stages[0],key=ctx.stageDetailScope(st);
  ctx.refresh=function(){
    this.refreshes++;
    this.plan={...this.plan,architecture:{checkpoint:'published'},stages:[{...st,review_snapshot:'published'}]};
    this.syncReviewViews();this.ensureStageReviewsLoaded(st);
  };
  ctx.ensureStageReviewsLoaded(st);const obsolete=ctx.calls[0];
  obsolete.done(page(ctx.reviewScope(st),[full('a')],0,{snapshot:'wrong'}),200);
  assert.equal(ctx.refreshes,1);assert.equal(ctx.calls.length,1);
  assert.equal(ctx.reviewView(ctx.plan.stages[0]).error,'');
  assert.match(ctx.stageReviewBlocks[key],/cannot be verified/);
  ctx.retryStageReviews(st);ctx.calls[1].done({error:'new failure'},500);
  obsolete.done(page(scope(st),[full('a')],0),200);
  obsolete.done({error:'obsolete failure'},500);
  assert.equal(ctx.stageReviewBlocks[key],'new failure');
  ctx.ensureStageReviewsLoaded(st);assert.equal(ctx.calls.length,2);
  const oldScope=ctx.stageDetailScope(st);
  ctx.plan={...ctx.plan,revision:6};ctx.syncReviewViews();
  ctx.ensureStageReviewsLoaded(st);assert.equal(ctx.calls.length,3,'a new stage scope is unblocked');
  const newScope=ctx.stageDetailScope(st);
  ctx.calls[2].done({error:'new revision failure'},500);
  assert.equal(ctx.stageReviewBlocks[oldScope],undefined);
  ctx.calls[1].done(page(scope(st),[full('a')],0),200);
  assert.equal(ctx.stageReviewBlocks[newScope],'new revision failure');
});
