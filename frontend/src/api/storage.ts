import { call } from './core';
export interface StorageRequest { launcherRoot?: string; gameRoot?: string }
export interface StorageStatus { launcherRoot: string; gameRoot: string; versions: string; packages: string; environmentOverride: boolean; pending: unknown | null; lastError: string | null; safSupported: boolean; restartRequired: boolean }
export interface StoragePreview { source: string; launcherRoot: string; gameRoot: string | null; restartRequired: boolean; preservesSource: boolean }
export interface StorageApplied { restartRequired: boolean; scheduled: boolean; completed: boolean }
export const storageStatus = () => call<StorageStatus>('storage_status');
export const storageSnapshot = storageStatus;
export const storagePreview = (request: StorageRequest) => call<StoragePreview>('storage_preview', { request });
export const storageApply = (request: StorageRequest) => call<StorageApplied>('storage_apply', { request });
export const storageCancel = () => call<void>('storage_cancel');
