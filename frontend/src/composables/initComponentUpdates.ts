import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useI18n } from '../i18n';
import { showToast } from './useToast';
let initialized = false;
export async function initComponentUpdates(): Promise<void> {
  if (initialized) return;
  initialized = true;
  let notified = false;
  await listen('components-auto-updated', () => {
    if (notified) return;
    notified = true;
    showToast(useI18n().t('components.updatedNotice'), 'success');
  });
  // Command only schedules background work and returns immediately.
  await invoke('components_ui_ready');
}
