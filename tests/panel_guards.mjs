// Evaluates Panel.qml's shared action guards (root.guards) against a test fixture.
// Buttons and keyboard shortcuts read `root.guards.<id>`; the guards are computed by
// PanelActions.guards() from the `guardFlags` binding in Panel.qml, so fixtures that
// used to evaluate a button's enabled expression evaluate that same binding here.
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const actionsSource = readFileSync(new URL('../quickshell/PanelActions.js', import.meta.url), 'utf8')
  .replace(/^\s*\.pragma\s+library\s*$/m, '');

export function guardFlagsExpression(qml) {
  const match = qml.match(/readonly property var guardFlags: (\(\{[\s\S]*?\}\))\n/);
  if (!match) throw new Error('Panel.qml must define readonly property var guardFlags');
  return match[1];
}

// Returns ctx => guards, evaluating the guardFlags binding with ctx as the root scope.
export function panelGuards(qml) {
  const flags = guardFlagsExpression(qml);
  const actions = {};
  vm.runInNewContext(actionsSource, actions);
  return ctx => actions.guards(vm.runInNewContext(flags, ctx));
}

// Defines ctx.guards (and ctx.root.guards when root is ctx) as a live getter.
export function installGuards(qml, ctx) {
  const guards = panelGuards(qml);
  Object.defineProperty(ctx, 'guards', { get() { return guards(ctx); }, configurable: true });
  return ctx;
}
