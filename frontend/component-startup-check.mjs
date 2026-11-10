// Isolated timing probe of the production initializer. IPC is controlled; no GUI timing is claimed.
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import ts from 'typescript';
const code = ts.transpileModule(readFileSync(new URL('./src/composables/initComponentUpdates.ts', import.meta.url), 'utf8'), { compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 } }).outputText;
for (const scenario of ['listen-pending', 'invoke-pending', 'ready']) {
  for (let sample = 0; sample < 3; sample++) {
    let subscriptions = 0, commands = 0, notices = 0, listener;
    const never = new Promise(() => {});
    const context = { exports: {}, require(id) {
      if (id === '@tauri-apps/api/event') return { listen: (_name, callback) => { subscriptions++; listener = callback; return scenario === 'listen-pending' ? never : Promise.resolve(() => {}); } };
      if (id === '@tauri-apps/api/core') return { invoke: (command) => { assert.equal(command, 'components_ui_ready'); commands++; return scenario === 'invoke-pending' ? never : Promise.resolve(); } };
      if (id === '../i18n') return { useI18n: () => ({ t: key => key }) };
      if (id === './useToast') return { showToast: () => { notices++; } };
      throw new Error(`unexpected dependency ${id}`);
    } };
    vm.runInNewContext(code, context);
    const start = performance.now();
    const initial = context.exports.initComponentUpdates();
    const callMs = performance.now() - start;
    void context.exports.initComponentUpdates();
    let settled = false;
    void initial.then(() => { settled = true; });
    await new Promise(resolve => setTimeout(resolve, 10));
    const heartbeatMs = performance.now() - start;
    assert.ok(callMs < 100, 'initializer must return without awaiting IPC');
    assert.ok(heartbeatMs < 250, 'pending IPC must leave event loop responsive');
    assert.equal(subscriptions, 1);
    assert.equal(commands, scenario === 'listen-pending' ? 0 : 1);
    assert.equal(settled, scenario === 'ready');
    if (scenario === 'ready') { listener(); listener(); assert.equal(notices, 1); }
    console.log(JSON.stringify({ scenario, sample, callMs: +callMs.toFixed(3), heartbeatMs: +heartbeatMs.toFixed(3), subscriptions, commands, settled }));
  }
}
