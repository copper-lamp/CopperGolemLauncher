import test from 'node:test';
import assert from 'node:assert/strict';
import { OnboardingSession, parseProgress, parseReleaseState, planBriefs, acknowledgeBriefs } from '../src/services/onboardingState.ts';
function store(initial = null) {
  let value = initial;
  return { fail: false, async read() { if (this.fail) throw Error('read failed'); return value; }, async write(_key, next) { if (this.fail) throw Error('disk full'); value = structuredClone(next); } };
}
test('every cursor and completed flag survive process recreation', async () => {
  const db = store(); let session = new OnboardingSession(db); await session.load();
  for (const step of ['theme', 'storage', 'game', 'content', 'launch', 'modules']) {
    await session.advance(); session = new OnboardingSession(db); await session.load(); assert.equal(session.progress.step, step); assert.equal(session.progress.completed, false);
  }
  await session.advance(); session = new OnboardingSession(db); await session.load(); assert.equal(session.progress.completed, true);
});
test('retired levi step resumes at storage instead of corrupting progress', async () => {
  const db = store({ schema: 1, step: 'levi', completed: false, selectedModules: [], installedModules: [] });
  const session = new OnboardingSession(db); await session.load(); assert.equal(session.progress.step, 'storage');
  await session.advance(); assert.equal(session.progress.step, 'game');
});
test('failed persistence keeps cursor and permits successful retry', async () => {
  const db = store(); const session = new OnboardingSession(db); await session.load(); db.fail = true;
  await assert.rejects(session.advance(), /disk full/); assert.equal(session.progress.step, 'features'); db.fail = false;
  await session.advance(); assert.equal(session.progress.step, 'theme');
});
test('failed read and malformed state are explicit; never silently complete', async () => {
  const db = store(); db.fail = true; await assert.rejects(new OnboardingSession(db).load(), /read failed/);
  assert.throws(() => parseProgress({ completed: true }), /Invalid/); assert.throws(() => parseReleaseState({ schema: 1 }), /Invalid/);
});
test('selection and successful installs resume without reinstalling successful modules', async () => {
  const db = store(); const first = new OnboardingSession(db); await first.load();
  await first.save({ selectedModules: ['a', 'b'], installedModules: ['a'] }); const next = new OnboardingSession(db); await next.load();
  assert.deepEqual(next.progress.selectedModules.filter(id => !next.progress.installedModules.includes(id)), ['b']);
});
const catalog = Array.from({ length: 10 }, (_, i) => ({ id: i + 1, version: `0.0.${i + 1}` }));
test('first install suppresses all historical briefs', () => { const plan = planBriefs('0.0.10', null, catalog); assert.deepEqual(plan.ids, []); assert.equal(plan.state.watermark, 10); });
test('jumping versions chooses latest five; older skipped items never return', () => {
  const plan = planBriefs('0.0.10', { schema: 1, highestVersion: '0.0.1', watermark: 1 }, catalog);
  assert.deepEqual(plan.ids, [10, 9, 8, 7, 6]); const ack = acknowledgeBriefs(plan.state);
  assert.deepEqual(planBriefs('0.0.10', ack, catalog).ids, []); assert.deepEqual(planBriefs('0.0.5', ack, catalog).ids, []);
});
test('pending display survives crashes; acknowledgement failure can retry without dropping batch', async () => {
  const plan = planBriefs('0.0.10', { schema: 1, highestVersion: '0.0.1', watermark: 1 }, catalog);
  const db = store(plan.state); db.fail = true; await assert.rejects(db.write('', acknowledgeBriefs(plan.state)));
  db.fail = false; const restored = parseReleaseState(await db.read()); assert.deepEqual(planBriefs('0.0.10', restored, catalog).ids, plan.ids);
});
test('downgrades do not consume pending newer version and language is not part of identity', () => {
  const plan = planBriefs('0.0.10', { schema: 1, highestVersion: '0.0.1', watermark: 1 }, catalog);
  assert.deepEqual(planBriefs('0.0.9', plan.state, catalog).ids, []);
  assert.deepEqual(planBriefs('0.0.10', plan.state, catalog).ids, plan.ids);
});
test('catalog rejects excess or duplicate entries', () => {
  assert.throws(() => planBriefs('0.0.10', null, [...catalog, { id: 11, version: '0.0.11' }]));
  assert.throws(() => planBriefs('0.0.10', null, [catalog[0], catalog[0]]));
});
