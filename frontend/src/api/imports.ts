import { call } from './core';

/** Kernel-inspected, temporary ticket. Source paths never authorize a commit. */
export interface ImportPreview {
  ticket: string;
  filename: string;
  kind: 'pack' | 'addon' | 'world' | 'template' | 'module';
  name: string;
  version: string | null;
  permissions: string[];
  moduleId: string | null;
  publisherVerified: boolean;
  installedVersion: string | null;
  sha256: string;
}
export const importsPick = () => call<ImportPreview[]>('imports_pick', undefined, { timeoutMs: 180_000 });
export const importsPending = () => call<ImportPreview[]>('imports_pending', undefined, { timeoutMs: 180_000 });
export const importsDrop = (paths: string[]) => call<ImportPreview[]>('imports_drop', { paths }, { timeoutMs: 180_000 });
export const importsCancel = (ticket: string) => call<void>('imports_cancel', { ticket });
export const importsCommit = (ticket: string, instance: string | null, replace: boolean, acknowledgeRisk: boolean) =>
  call<void>('imports_commit', { ticket, instance, replace, acknowledgeRisk }, { timeoutMs: 600_000 });
