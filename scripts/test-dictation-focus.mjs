import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { test } from 'node:test';
import vm from 'node:vm';
import ts from 'typescript';

// Execute the actual production callback with a virtual clock. No React DOM,
// microphone, external API, or foreground application is touched by this test.
const source = process.env.FOCUS_BASELINE
  ? execFileSync('git', ['show', `${process.env.FOCUS_BASELINE}:src/App.tsx`], { encoding: 'utf8' })
  : readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8');
const ast = ts.createSourceFile('App.tsx', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
let callback;
function visit(node) {
  if (ts.isVariableDeclaration(node) && node.name.getText(ast) === 'handleStop') {
    callback = node.initializer.arguments[0].getText(ast);
  }
  ts.forEachChild(node, visit);
}
visit(ast);
assert.ok(callback, 'Toolbar stop callback must exist');

test('toolbar stop cannot restore focus while final transcription is still pending', async () => {
  const events = [];
  const commands = [];
  const timers = new Map();
  let timerId = 0;
  const context = {
    emit: async (...args) => events.push(args),
    invoke: async (...args) => commands.push(args),
    stopFallbackTimerRef: { current: null },
    window: {
      setTimeout: (fn) => { timers.set(++timerId, fn); return timerId; },
      clearTimeout: (id) => timers.delete(id),
    },
  };
  const js = ts.transpile(`(${callback})`, { target: ts.ScriptTarget.ES2022 });
  const stop = vm.runInNewContext(js, context);
  await stop();
  // Simulate a slow transcription: all toolbar-side timers fire before the
  // main window's transcription/injection promise has completed.
  for (const fn of timers.values()) await fn();
  assert.deepEqual(events, [['hotkey-pressed', 'toolbar']]);
  assert.deepEqual(commands, [], 'Only the completed dictation may restore main');
});
