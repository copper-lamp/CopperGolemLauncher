import test from 'node:test';
import assert from 'node:assert/strict';
import { acquireModal, activeModalId, guardModalFocus } from '../src/services/modalCoordinator.ts';

test('mandatory modal priority, nested handoff and idempotent release', () => {
  const ordinary = acquireModal('ordinary', 10);
  const guide = acquireModal('onboarding-test', 100);
  const update = acquireModal('update-test', 10);
  assert.equal(activeModalId.value, 'onboarding-test');
  assert.throws(() => acquireModal('onboarding-test'), /already registered/);
  guide(); guide();
  assert.equal(activeModalId.value, 'update-test');
  update(); assert.equal(activeModalId.value, 'ordinary');
  ordinary(); assert.equal(activeModalId.value, null);
});

test('focus and Escape belong only to active modal; Tab wraps and cleanup restores', async () => {
  const listeners = new Map();
  class Element {
    isConnected = true;
    children = [];
    focus() { document.activeElement = this; }
    contains(value) { return value === this || this.children.includes(value); }
    querySelectorAll() { return this.children; }
    getClientRects() { return [1]; }
    closest() { return null; }
    matches() { return false; }
  }
  const previous = new Element(), panel = new Element(), first = new Element(), last = new Element();
  panel.children = [first, last];
  globalThis.HTMLElement = Element;
  globalThis.document = {
    activeElement: previous,
    addEventListener(name, handler) { const items = listeners.get(name) ?? []; items.push(handler); listeners.set(name, items); },
    removeEventListener(name, handler) { listeners.set(name, (listeners.get(name) ?? []).filter(x => x !== handler)); },
  };
  const release = acquireModal('focus-test', 100);
  const cleanup = guardModalFocus('focus-test', () => panel);
  await Promise.resolve();
  assert.equal(document.activeElement, panel);
  let prevented = false, stopped = false;
  const key = (value, shiftKey = false) => {
    prevented = false; stopped = false;
    for (const handler of listeners.get('keydown')) handler({ key: value, shiftKey, preventDefault() { prevented = true; }, stopImmediatePropagation() { stopped = true; } });
  };
  key('Escape'); assert.equal(prevented, true); assert.equal(stopped, true);
  last.focus(); key('Tab'); assert.equal(document.activeElement, first);
  key('Tab', true); assert.equal(document.activeElement, last);
  const popup = new Element(), trigger = new Element();
  popup.dataset = { coSelectOwner: 'trusted-trigger' };
  popup.closest = () => popup;
  document.getElementById = id => id === 'trusted-trigger' ? trigger : null;
  panel.children.push(trigger);
  let popupPrevented = false;
  for (const handler of listeners.get('keydown')) handler({ key: 'Escape', target: popup, preventDefault() { popupPrevented = true; } });
  assert.equal(popupPrevented, false, 'owned select receives Escape');
  popup.dataset.coSelectOwner = 'foreign-trigger';
  let foreignPrevented = false;
  for (const handler of listeners.get('keydown')) handler({ key: 'Escape', target: popup, preventDefault() { foreignPrevented = true; }, stopImmediatePropagation() {} });
  assert.equal(foreignPrevented, true, 'unowned marker cannot bypass modal');
  const higher = acquireModal('higher-test', 200);
  key('Escape'); assert.equal(prevented, false);
  higher();
  release(); cleanup();
  assert.equal(document.activeElement, previous);
  assert.equal(listeners.get('keydown').length, 0);
  delete globalThis.document; delete globalThis.HTMLElement;
});
