import { call } from './core';

export interface StorageRequest { launcherRoot?: string; gameRoot?: string }
export interface StorageStatus { launcherRoot: string; gameRoot: string; versions: string; packages: string; environmentOverride: boolean; pending: unknown | null; lastError: string | null; safSupported: boolean; restartRequired: boolean }
export interface StoragePreview { source: string; launcherRoot: string; gameRoot: string | null; restartRequired: boolean; preservesSource: boolean }
export interface StorageApplied { restartRequired: boolean; scheduled: boolean; completed: boolean }

/** 后台扫描状态：idle 未开始，running 扫描中，ready 有结果，failed 失败。 */
export type StorageScanState = 'idle' | 'running' | 'ready' | 'failed';
/** 单分类状态：pending 未开始，running 扫描中，done 本次已扫，cached 来自上次清单。 */
export type StorageCategoryState = 'pending' | 'running' | 'done' | 'cached' | 'failed';

export interface StorageCategoryScan {
  id: string;
  /** 未知时为 null：界面显示占位，不能当成 0。 */
  bytes: number | null;
  cleanable: boolean;
  state: StorageCategoryState;
  files: number;
}

export interface StorageScanSnapshot {
  scanId: number;
  state: StorageScanState;
  root: string;
  totalBytes: number | null;
  files: number;
  categories: StorageCategoryScan[];
  startedAt: number | null;
  finishedAt: number | null;
  error: string | null;
}

export interface StorageCleanResult { freedBytes: number; cleared: Record<string, number> }

export const storageStatus = () => call<StorageStatus>('storage_status');
export const storageSnapshot = storageStatus;
export const storagePreview = (request: StorageRequest) => call<StoragePreview>('storage_preview', { request });
export const storageApply = (request: StorageRequest) => call<StorageApplied>('storage_apply', { request });
export const storageCancel = () => call<void>('storage_cancel');

/** 读内存快照，零重 IO。 */
export const storageUsageSnapshot = () => call<StorageScanSnapshot>('storage_usage_snapshot');
/** 启动后台扫描，命令立刻返回启动瞬间的快照。 */
export const storageScanStart = (force = false) => call<StorageScanSnapshot>('storage_scan_start', { force });
/** 取消扫描，返回是否已经停下。 */
export const storageScanCancel = () => call<boolean>('storage_scan_cancel');
export const storageClean = (categories: string[]) => call<StorageCleanResult>('storage_clean', { categories });
