import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { effectScope, ref, watch, onScopeDispose, nextTick } from 'vue';
import { webcrypto } from 'node:crypto';
function load(path, globals, exports) {
  const source = fs.readFileSync(new URL(path, import.meta.url), 'utf8').replace(/^import .*;\r?\n/gm, '').replace(/export /g, '');
  const js = ts.transpile(source, { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.None });
  return vm.runInNewContext(`${js}\n;({${exports.join(',')}})`, { ...globals, setTimeout, clearTimeout, crypto: webcrypto });
}
const locale = ref('zh-CN');
const pending = [];
const cancelled = [];
const calls = [];
let defer = false;
let responseGeneration = 7;
const { useTranslation } = load('../src/composables/useTranslation.ts', {
  ref, watch, onScopeDispose, useLocaleRef: () => locale, useTranslationError: () => e => e.message,
  translationCancel: async id => { cancelled.push(id); return true; },
  translationConfig: async () => ({ target_language: 'auto', source_language: 'en', generation: 7 }),
  translationTranslate: async (id, texts, target, source, expectedGeneration) => {
    calls.push({ id, texts, target, source, expectedGeneration });
    if (defer) return new Promise(resolve => pending.push(() => resolve({ texts: texts.map(t => `translated:${t}`), generation: responseGeneration })));
    return { texts: texts.map(t => `translated:${t}`), generation: responseGeneration };
  },
}, ['useTranslation']);
const scope = effectScope();
const state = scope.run(() => useTranslation());
const input = Array.from({ length: 300 }, (_, i) => `item-${i}`);
await state.run(input);
assert.deepEqual(calls.map(c => c.texts.length), [128,128,44]);
assert.equal(state.results.value.length, 300);
assert.equal(state.results.value[299], 'translated:item-299');
assert.equal(calls[0].source, 'en');
assert.equal(calls[0].target, 'zh-CN');
assert.ok(calls.every(c => c.expectedGeneration === 7));
responseGeneration = 8;
await state.run(['configuration changed']);
assert.equal(state.translated.value, false);
assert.equal(state.error.value, 'translation.configuration_changed');
responseGeneration = 7;
state.reset();
assert.equal(state.results.value.length, 0);
assert.equal(state.translated.value, false);
defer = true;
let run = state.run(['old page']);
await new Promise(resolve => setImmediate(resolve));
state.reset(); pending.shift()(); await run;
assert.equal(state.results.value.length, 0);
assert.equal(state.translated.value, false);
assert.equal(cancelled.length, 1);
run = state.run(['old locale']);
await new Promise(resolve => setImmediate(resolve));
locale.value = 'en-US'; await nextTick(); pending.shift()(); await run;
assert.equal(state.translated.value, false);
assert.equal(cancelled.length, 2);
run = state.run(['unmounted']);
await new Promise(resolve => setImmediate(resolve));
scope.stop(); pending.shift()(); await run;
assert.equal(state.translated.value, false);
assert.equal(cancelled.length, 3);
// A minimal DOM adapter exercises the real text walker and serializer boundary.
// Browser parsing/sanitization is independently owned by renderReadme + DOMPurify.
const escape = s => s.replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('>','&gt;');
const nodes = [
  { textContent: 'Hello https://example.org/a {{name}} ${value} {id} %token% world', parentElement: { closest: () => false } },
  { textContent: 'const untouched = 1', parentElement: { closest: () => true } },
];
let document;
class DOMParser {
  parseFromString() {
    let index = 0;
    document = { body: { get innerHTML() { return `<p>${escape(nodes[0].textContent)}</p><code>${escape(nodes[1].textContent)}</code><img src="https://image.example/a.png">`; } }, createTreeWalker: () => ({ nextNode: () => nodes[index++] ?? null }) };
    return document;
  }
}
const { translationDocument } = load('../src/services/translation.ts', { DOMParser, NodeFilter: { SHOW_TEXT: 4 } }, ['translationDocument']);
const plan = translationDocument('sanitized document');
assert.equal(plan.texts.length, 2);
assert.ok(!plan.texts.join('').includes('https://'));
assert.ok(!plan.texts.join('').includes('const untouched'));
const html = plan.render(plan.texts.map(() => '<script>unsafe</script>'));
assert.ok(html.includes('&lt;script&gt;unsafe&lt;/script&gt;'));
assert.ok(!html.includes('<script>'));
assert.ok(html.includes('https://example.org/a {{name}} ${value} {id} %token%'));
assert.ok(html.includes('<code>const untouched = 1</code>'));
assert.ok(html.includes('src="https://image.example/a.png"'));
assert.throws(() => plan.render([]), /segment count mismatch/);
const store = load('../src/modules/content-download/listStore.ts', { ref }, ['lastValidated','listNeedsValidation','resetList']);
store.lastValidated.value = 1000;
assert.equal(store.listNeedsValidation(1000 + 12*60*60*1000 - 1), false);
assert.equal(store.listNeedsValidation(1000 + 12*60*60*1000), true);
assert.equal(store.lastValidated.value, 1000); // retained module state across view consumers
store.resetList();
assert.equal(store.lastValidated.value, 0);
console.log('Translation logic: batching, restore, cancellation, locale/disposal generation, protected text and escaped rendering passed.');
