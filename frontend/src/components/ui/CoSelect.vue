<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, useId, watch, type Component } from "vue";
import { ChevronDown, Check } from "@lucide/vue";
import { activeModalId } from "../../services/modalCoordinator";
export interface SelectOption { value: string; label: string; disabled?: boolean; icon?: Component; image?: string | null; hint?: string }
const props = withDefaults(defineProps<{ modelValue: string; options: SelectOption[]; placeholder?: string; disabled?: boolean; label?: string; emptyText?: string; searchable?: boolean; searchLabel?: string }>(), { placeholder: "", disabled: false, label: "", emptyText: "", searchable: false, searchLabel: "" });
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const id = useId();
const trigger = ref<HTMLButtonElement>(); const panel = ref<HTMLElement>(); const search = ref<HTMLInputElement>();
const open = ref(false); const query = ref(""); const active = ref(-1); const position = ref<Record<string, string>>({});
const selected = computed(() => props.options.find(o => o.value === props.modelValue));
const filtered = computed(() => props.options.filter(o => o.label.toLocaleLowerCase().includes(query.value.toLocaleLowerCase())));
function place() {
  if (!trigger.value) return;
  const r = trigger.value.getBoundingClientRect(); if (r.bottom < 0 || r.top > window.innerHeight || !trigger.value.isConnected) { close(); return; } const below = window.innerHeight - r.bottom - 8; const above = r.top - 8; const up = below < 220 && above > below;
  position.value = { position: "fixed", left: `${Math.max(8, Math.min(r.left, window.innerWidth - Math.max(r.width, 200) - 8))}px`, width: `${Math.min(Math.max(r.width, 200), window.innerWidth - 16)}px`, maxHeight: `${Math.max(64, Math.min(360, up ? above - 6 : below - 6))}px`, ...(up ? { bottom: `${window.innerHeight - r.top + 6}px` } : { top: `${r.bottom + 6}px` }) };
}
async function show() {
  if (props.disabled) return;
  query.value = ""; open.value = true; active.value = filtered.value.findIndex(o => o.value === props.modelValue && !o.disabled);
  if (active.value < 0) active.value = filtered.value.findIndex(o => !o.disabled);
  place(); await nextTick(); (props.searchable ? search.value : panel.value)?.focus(); reveal();
}
function close(restore = false) { open.value = false; if (restore) trigger.value?.focus(); }
function choose(index: number) { const option = filtered.value[index]; if (!option || option.disabled) return; emit("update:modelValue", option.value); close(true); }
function reveal() { void nextTick(() => document.getElementById(`${id}-${active.value}`)?.scrollIntoView({ block: "nearest" })); }
function move(direction: number) { const n = filtered.value.length; for (let step = 1; step <= n; step++) { const index = ((active.value + direction * step) % n + n) % n; if (!filtered.value[index]?.disabled) { active.value = index; reveal(); return; } } }
let typed = ""; let typingTimer = 0;
function keydown(event: KeyboardEvent) {
  event.stopPropagation();
  if (["ArrowDown", "ArrowUp", "Home", "End", "Enter", "Escape", " "].includes(event.key) && !(event.target === search.value && [" ", "Home", "End"].includes(event.key))) event.preventDefault();
  if (event.key === "Escape") close(true);
  else if (event.key === "Tab") { event.preventDefault(); close(true); }
  else if (event.key === "ArrowDown") move(1);
  else if (event.key === "ArrowUp") move(-1);
  else if (event.key === "Home" && event.target !== search.value) { active.value = -1; move(1); }
  else if (event.key === "End" && event.target !== search.value) { active.value = 0; move(-1); }
  else if (event.key === "Enter" || (event.key === " " && event.target !== search.value)) choose(active.value);
  else if (!props.searchable && event.key.length === 1 && !event.ctrlKey && !event.metaKey) { typed += event.key.toLocaleLowerCase(); window.clearTimeout(typingTimer); typingTimer = window.setTimeout(() => { typed = ""; }, 650); const index = filtered.value.findIndex(o => !o.disabled && o.label.toLocaleLowerCase().startsWith(typed)); if (index >= 0) { active.value = index; reveal(); } }
}
function outside(event: PointerEvent) { if (!panel.value?.contains(event.target as Node) && !trigger.value?.contains(event.target as Node)) close(); }
function cleanup() { document.removeEventListener("pointerdown", outside, true); window.removeEventListener("resize", place); document.removeEventListener("scroll", place, true); }
watch(open, value => { if (value) { document.addEventListener("pointerdown", outside, true); window.addEventListener("resize", place); document.addEventListener("scroll", place, true); } else cleanup(); });
watch(activeModalId, () => close());
watch(query, () => { active.value = filtered.value.findIndex(o => !o.disabled); });
watch(() => props.disabled, value => { if (value) close(); });
watch(() => props.options, () => { if (open.value && !filtered.value[active.value]) active.value = filtered.value.findIndex(o => !o.disabled); });
onBeforeUnmount(() => { cleanup(); window.clearTimeout(typingTimer); });
</script>
<template>
  <button :id="`${id}-trigger`" ref="trigger" type="button" class="co-select" :disabled="disabled" :aria-label="label || undefined" aria-haspopup="listbox" :aria-expanded="open" :aria-controls="open ? id : undefined" @co-select-close="close(true)" @keydown.esc="open && close(true)" @click="open ? close() : show()" @keydown.down.prevent="show" @keydown.up.prevent="show">
    <span v-if="label" class="co-select__caption">{{ label }}</span>
    <slot name="value" :option="selected"><component :is="selected.icon" v-if="selected?.icon" :size="18" /><img v-if="selected?.image" :src="selected.image" alt="" /><span class="co-select__value">{{ selected?.label ?? placeholder }}</span></slot>
    <ChevronDown :size="16" class="co-select__chevron" :class="{ 'is-open': open }" />
  </button>
  <Teleport to="body"><Transition name="co-pop"><div v-if="open" ref="panel" :data-co-select-owner="`${id}-trigger`" class="co-select__panel" :style="position" tabindex="-1" @keydown="keydown">
    <input v-if="searchable" ref="search" v-model="query" class="co-select__search" :aria-label="searchLabel || label || placeholder" :placeholder="searchLabel" :aria-controls="id" :aria-activedescendant="active >= 0 ? `${id}-${active}` : undefined" role="combobox" aria-expanded="true" />
    <div :id="id" role="listbox" :aria-label="label || placeholder || undefined" :aria-activedescendant="active >= 0 ? `${id}-${active}` : undefined" class="co-select__list">
      <div v-for="(option, index) in filtered" :id="`${id}-${index}`" :key="option.value" role="option" :aria-selected="option.value === modelValue" :aria-disabled="option.disabled || undefined" class="co-select__option" :class="{ 'is-active': active === index, 'is-disabled': option.disabled }" @pointermove="!option.disabled && (active = index)" @pointerdown.prevent @click="choose(index)">
        <slot name="option" :option="option"><component :is="option.icon" v-if="option.icon" :size="18" /><img v-if="option.image" :src="option.image" alt="" /><span class="co-select__option-label">{{ option.label }}<small v-if="option.hint">{{ option.hint }}</small></span></slot><Check v-if="option.value === modelValue" :size="15" />
      </div><p v-if="!filtered.length" class="co-select__empty">{{ emptyText }}</p>
    </div>
  </div></Transition></Teleport>
</template>
<style scoped>
.co-select { display:inline-flex; align-items:center; gap:8px; min-width:0; min-height:var(--copper-control-h); padding:7px 12px; border:0; border-radius:var(--copper-radius-control); background:var(--copper-surface-2); color:var(--copper-text); font:inherit; cursor:pointer; box-shadow:var(--copper-shadow-raised); transition:transform var(--copper-motion-component-press) var(--copper-easing-jelly),background var(--copper-duration-fast); text-align:left; }
.co-select:hover:not(:disabled) { background:var(--copper-surface-3); }
.co-select:active:not(:disabled) { transform:scale(.98); }
.co-select:disabled { opacity:.5; cursor:not-allowed; }
.co-select__caption { color:var(--copper-text-secondary); }
.co-select__value { flex:1; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.co-select__chevron { margin-left:auto; flex-shrink:0; transition:transform var(--copper-motion-component-enter) var(--copper-easing-jelly); }
.co-select__chevron.is-open { transform:rotate(180deg); }
.co-select img,.co-select__option img { width:24px; height:24px; object-fit:cover; border-radius:var(--copper-radius-sm); }
.co-select__panel { z-index:1600; display:flex; flex-direction:column; padding:6px; border-radius:var(--copper-radius-card); background:var(--copper-surface); color:var(--copper-text); box-shadow:var(--copper-shadow-card); overflow:hidden; transform-origin:top center; }
.co-select__list { overflow-y:auto; min-height:0; overscroll-behavior:contain; }
.co-select__option { display:flex; align-items:center; gap:10px; min-height:38px; padding:9px 10px; border-radius:var(--copper-radius-control); cursor:pointer; }
.co-select__option.is-active { background:var(--copper-hover); }
.co-select__option[aria-selected="true"] { color:var(--copper-accent); }
.co-select__option.is-disabled { opacity:.45; cursor:not-allowed; }
.co-select__option-label { flex:1; min-width:0; overflow-wrap:anywhere; }
.co-select__option-label small { display:block; color:var(--copper-text-secondary); font-size:var(--copper-font-size-xs); }
.co-select__search { flex-shrink:0; width:100%; padding:9px; margin-bottom:5px; border:0; border-radius:var(--copper-radius-control); background:var(--copper-surface-2); color:inherit; font:inherit; }
.co-select__empty { padding:12px; color:var(--copper-text-secondary); }
</style>
