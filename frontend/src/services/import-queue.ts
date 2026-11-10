import { ref } from 'vue';
import { importsPick, type ImportPreview } from '../api/imports';
import { showToast } from '../composables/useToast';
export const importQueue = ref<ImportPreview[]>([]);
export const importPicking = ref(false);
const retired = new Set<string>();
export function retireImport(ticket: string) {
  retired.add(ticket);
  if (retired.size > 512) retired.delete(retired.values().next().value!);
  importQueue.value = importQueue.value.filter(item => item.ticket !== ticket);
}
export function enqueueImports(previews: ImportPreview[]) {
  const known = new Set(importQueue.value.map(item => item.ticket));
  for (const preview of previews) if (!known.has(preview.ticket) && !retired.has(preview.ticket)) { importQueue.value.push(preview); known.add(preview.ticket); }
}
export async function pickGlobalImport() {
  if (importPicking.value) return;
  importPicking.value = true;
  try { enqueueImports(await importsPick()); }
  catch (error) { showToast(String(error), 'error'); }
  finally { importPicking.value = false; }
}
