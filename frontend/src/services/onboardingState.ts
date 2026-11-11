export const ONBOARDING_STEPS = ['features', 'theme', 'levi', 'storage', 'game', 'content', 'launch', 'modules'] as const;
export type OnboardingStep = typeof ONBOARDING_STEPS[number];
export interface OnboardingProgress {
  schema: 1;
  step: OnboardingStep;
  completed: boolean;
  selectedModules: string[];
  installedModules: string[];
}
export interface StateStore { read(key: string): Promise<unknown>; write(key: string, value: unknown): Promise<void>; }
export const PROGRESS_KEY = 'onboarding.progress';
export const RELEASE_KEY = 'onboarding.releaseState';
export function parseProgress(value: unknown): OnboardingProgress {
  if (value === null || value === undefined) return { schema: 1, step: 'features', completed: false, selectedModules: [], installedModules: [] };
  if (typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid onboarding progress');
  const v = value as Record<string, unknown>;
  if (v.schema !== 1 || !ONBOARDING_STEPS.includes(v.step as OnboardingStep) || typeof v.completed !== 'boolean' ||
    !Array.isArray(v.selectedModules) || !v.selectedModules.every(x => typeof x === 'string') ||
    !Array.isArray(v.installedModules) || !v.installedModules.every(x => typeof x === 'string')) throw new Error('Invalid onboarding progress');
  return { schema: 1, step: v.step as OnboardingStep, completed: v.completed, selectedModules: [...new Set(v.selectedModules as string[])], installedModules: [...new Set(v.installedModules as string[])] };
}
/** Writes are acknowledged before the visible cursor moves. Failed writes leave the previous state usable. */
export class OnboardingSession {
  progress: OnboardingProgress = parseProgress(null);
  private readonly store: StateStore;
  constructor(store: StateStore) { this.store = store; }
  async load(): Promise<void> { this.progress = parseProgress(await this.store.read(PROGRESS_KEY)); }
  async save(patch: Partial<Omit<OnboardingProgress, 'schema'>>): Promise<void> {
    const next = parseProgress({ ...this.progress, ...patch });
    await this.store.write(PROGRESS_KEY, next);
    this.progress = next;
  }
  async advance(leviAvailable: boolean): Promise<void> {
    const steps = ONBOARDING_STEPS.filter(step => step !== 'levi' || leviAvailable);
    const index = steps.indexOf(this.progress.step);
    const next = steps[index + 1];
    await this.save(next ? { step: next } : { completed: true });
  }
}
export interface ReleaseState { schema: 1; highestVersion: string; watermark: number; pendingVersion?: string; pendingIds?: number[]; }
export interface BriefIdentity { id: number; version: string; }
export function compareVersions(a: string, b: string): number {
  const parse = (value: string) => {
    if (!/^\d+\.\d+\.\d+$/.test(value)) throw new Error('Invalid release version');
    return value.split('.').map(Number);
  };
  const x = parse(a), y = parse(b);
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i]! > y[i]! ? 1 : -1;
  return 0;
}
export function parseReleaseState(value: unknown): ReleaseState | null {
  if (value === null || value === undefined) return null;
  if (typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid release state');
  const v = value as ReleaseState;
  if (v.schema !== 1 || typeof v.highestVersion !== 'string' || !Number.isSafeInteger(v.watermark) || v.watermark < 0 ||
    (v.pendingVersion !== undefined && typeof v.pendingVersion !== 'string') ||
    (v.pendingIds !== undefined && (!Array.isArray(v.pendingIds) || !v.pendingIds.every(Number.isSafeInteger)))) throw new Error('Invalid release state');
  compareVersions(v.highestVersion, v.highestVersion);
  if (v.pendingVersion) compareVersions(v.pendingVersion, v.pendingVersion);
  return { ...v, pendingIds: v.pendingIds ? [...v.pendingIds] : undefined };
}
/** A persisted pending batch can be resumed after a crash; old skipped candidates never return. */
export function planBriefs(currentVersion: string, previous: ReleaseState | null, catalog: readonly BriefIdentity[]): { state: ReleaseState; ids: number[] } {
  compareVersions(currentVersion, currentVersion);
  if (catalog.length > 10 || new Set(catalog.map(x => x.id)).size !== catalog.length || catalog.some(x => !Number.isSafeInteger(x.id) || x.id <= 0)) throw new Error('Invalid local release catalog');
  const eligible = catalog.filter(x => compareVersions(x.version, currentVersion) <= 0).sort((a, b) => b.id - a.id);
  const watermark = Math.max(previous?.watermark ?? 0, ...eligible.map(x => x.id));
  if (!previous) return { state: { schema: 1, highestVersion: currentVersion, watermark }, ids: [] };
  if (previous.pendingVersion === currentVersion) return { state: previous, ids: (previous.pendingIds ?? []).filter(id => eligible.some(x => x.id === id)) };
  if (compareVersions(currentVersion, previous.highestVersion) <= 0) return { state: previous, ids: [] };
  const ids = eligible.filter(x => x.id > previous.watermark && compareVersions(x.version, previous.highestVersion) > 0).slice(0, 5).map(x => x.id);
  return { state: { schema: 1, highestVersion: currentVersion, watermark, ...(ids.length ? { pendingVersion: currentVersion, pendingIds: ids } : {}) }, ids };
}
export function acknowledgeBriefs(state: ReleaseState): ReleaseState {
  return { schema: 1, highestVersion: state.highestVersion, watermark: state.watermark };
}
