import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const edit = {};
vm.runInNewContext(readFileSync(new URL('../quickshell/PlanEdit.js', import.meta.url), 'utf8'), edit);

test('plan edit transformations preserve committed stages and submitted optional fields', () => {
  const original = {stages: [{id: 1, title: 'Committed', instructions: 'Keep', acceptance: 'Done', commit: 'a', status: 'committed'},
    {id: 2, title: 'Editable', instructions: 'Change', acceptance: 'Checked', commit: 'b', depends_on: [1],
      model_constraint: {provider: 'codex'}}]};
  const stages = edit.cloneStages(original);
  assert.notEqual(stages, original.stages);
  assert.equal(edit.editableNeighbor(stages, 1, -1), -1);
  assert.equal(edit.editableNeighbor(stages, 0, 1), 1);
  const moved = edit.moveStages(stages, 0, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(moved.map(s => s.id))), [2, 1]);
  assert.deepEqual(JSON.parse(JSON.stringify(edit.deleteStage(stages, 1).map(s => s.id))), [1]);
  assert.equal(edit.addStage(stages).at(-1).title, '');
  let constraint = edit.modelConstraint(stages[1], 'model', ' exact ');
  assert.deepEqual(JSON.parse(JSON.stringify(constraint)), {provider: 'codex', model: 'exact'});
  constraint = edit.modelConstraint({model_constraint: constraint}, 'provider', '');
  constraint = edit.modelConstraint({model_constraint: constraint}, 'model', '');
  assert.equal(constraint, null);
  assert.deepEqual(JSON.parse(JSON.stringify(edit.payload('Goal', stages))), {goal: 'Goal', stages: [
    {title: 'Committed', instructions: 'Keep', acceptance: 'Done', commit: 'a', model_constraint: null, id: 1},
    {title: 'Editable', instructions: 'Change', acceptance: 'Checked', commit: 'b', model_constraint: {provider: 'codex'}, depends_on: [1], id: 2},
  ]});
});
