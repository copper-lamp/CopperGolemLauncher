import { onboardingKey } from './onboardingMessages';
/** Bundled catalog only. Add real entries when a release is published; retain at most ten. */
export const RELEASE_BRIEFS = [
  { id: 4, version: '0.0.4', titleKey: onboardingKey('release.modules.title'), bodyKey: onboardingKey('release.modules.body'), image: '/app-512.png' },
] as const;
