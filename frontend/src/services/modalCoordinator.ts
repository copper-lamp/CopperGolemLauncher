import { computed, shallowRef } from 'vue';
interface ModalEntry { id: string; priority: number; order: number; }
const entries = shallowRef<ModalEntry[]>([]);
let order = 0;
export const activeModalId = computed(() => [...entries.value].sort((a, b) => b.priority - a.priority || b.order - a.order)[0]?.id ?? null);
/** Register an open modal. Only the active owner may trap keys or restore focus. Release is idempotent. */
export function acquireModal(id: string, priority = 0): () => void {
  if (entries.value.some(entry => entry.id === id)) throw new Error(`Modal already registered: ${id}`);
  entries.value = [...entries.value, { id, priority, order: ++order }];
  let released = false;
  return () => { if (released) return; released = true; entries.value = entries.value.filter(entry => entry.id !== id); };
}
export function isModalActive(id: string): boolean { return activeModalId.value === id; }
/** Capture-phase focus boundary also excludes unrelated Teleports while onboarding is mandatory. */
export function guardModalFocus(id: string, panel: () => HTMLElement | null, dismiss?: () => void): () => void {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  function focusInside() { if (isModalActive(id)) panel()?.focus(); }
  function ownedSelect(target: EventTarget | null): boolean {
    if (!(target instanceof HTMLElement)) return false;
    const popup = target.closest<HTMLElement>('[data-co-select-owner]');
    const owner = popup?.dataset.coSelectOwner;
    const trigger = owner ? document.getElementById(owner) : null;
    return !!trigger && !!panel()?.contains(trigger);
  }
  function focus(event: FocusEvent) {
    if (isModalActive(id) && panel() && !panel()!.contains(event.target as Node) && !ownedSelect(event.target)) { event.stopImmediatePropagation(); focusInside(); }
  }
  function key(event: KeyboardEvent) {
    if (!isModalActive(id)) return;
    if (ownedSelect(event.target)) return;
    const target = event.target;
    if ((event.key === 'Escape' || event.key === 'Tab') && target instanceof HTMLElement && panel()?.contains(target) && target.matches('[aria-expanded="true"][aria-haspopup="listbox"]')) {
      target.dispatchEvent(new Event('co-select-close'));
      event.preventDefault(); event.stopImmediatePropagation(); return;
    }
    if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); dismiss?.(); return; }
    if (event.key !== 'Tab') return;
    const nodes = [...(panel()?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), a[href], [tabindex="0"]') ?? [])].filter(node => node.getClientRects().length > 0);
    const first = nodes[0], last = nodes[nodes.length - 1];
    if (!first) { event.preventDefault(); focusInside(); }
    else if (event.shiftKey && (document.activeElement === first || document.activeElement === panel())) { event.preventDefault(); last?.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
  }
  document.addEventListener('focusin', focus, true);
  document.addEventListener('keydown', key, true);
  queueMicrotask(focusInside);
  return () => {
    document.removeEventListener('focusin', focus, true);
    document.removeEventListener('keydown', key, true);
    if (!activeModalId.value && previous?.isConnected) previous.focus();
  };
}
