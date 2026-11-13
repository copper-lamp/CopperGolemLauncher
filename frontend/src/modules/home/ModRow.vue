<script setup lang="ts">
// 模组列表里的单行。
//
// 一行承担「浏览 + 快捷操作」两件事：平时只有图标、名字、介绍、版本号，
// 悬停或选中时才把四个操作按钮铺到右侧 —— 模组多起来以后，常驻按钮会把
// 名字挤成一团。行的容器用 div 而不是 button，因为里面还要嵌按钮。
//
// 图标由后端解析为 data URL；缺失或加载失败时使用主题彩底首字母。

import { computed } from "vue";
import { CircleCheck, CircleSlash, FolderOpen, Info, Trash2 } from "@lucide/vue";
import CoContentIcon from "../../components/ui/CoContentIcon.vue";

import { useI18n } from "../../i18n";
import type { ModView } from "../../api/home";

const props = defineProps<{
  mod: ModView;
  selected: boolean;
  /** 目录里的最新版本号；有值表示该模组可更新。 */
  updateVersion?: string | null;
}>();

const emit = defineEmits<{
  toggle: [];
  detail: [];
  folder: [];
  remove: [];
  enable: [];
}>();

const { t } = useI18n();

/**
 * 介绍行：清单里的 `description` 优先，没有就退回「类型 · 作者」。
 * 大多数第三方模组不写 description，空白行比退回信息更糟。
 */
const description = computed(() => {
  const own = props.mod.description.trim();
  if (own) return own;
  const author = props.mod.author.trim();
  return author ? `${props.mod.mod_type} · ${author}` : props.mod.mod_type;
});
</script>

<template>
  <div
    :class="[
      'row',
      {
        'row--selected': selected,
        'row--off': !mod.enabled,
      },
    ]"
    role="option"
    :aria-selected="selected"
    tabindex="0"
    :title="mod.name"
    @click="emit('toggle')"
    @keydown.enter.prevent="emit('toggle')"
    @keydown.space.prevent="emit('toggle')"
  >
    <span class="row__icon">
      <CoContentIcon :name="mod.name" :src="mod.icon" />
    </span>

    <span class="row__text">
      <span class="row__name">{{ mod.name }}</span>
      <span class="row__desc">{{ description }}</span>
    </span>

    <span class="row__version">
      <span v-if="!mod.enabled" class="row__off">{{ t("module.home.mods.disabled") }}</span>
      <span class="row__version-text">{{ mod.version || "—" }}</span>
      <span v-if="updateVersion" class="row__update" :title="updateVersion">
        <span class="row__update-arrow">↑</span>
        {{ updateVersion }}
      </span>
    </span>

    <span class="row__actions" @click.stop>
      <button
        type="button"
        class="row__action"
        :title="t('module.home.mods.detail')"
        @click="emit('detail')"
      >
        <Info :size="15" />
      </button>
      <button
        type="button"
        class="row__action"
        :title="t('module.home.mods.open_folder')"
        @click="emit('folder')"
      >
        <FolderOpen :size="15" />
      </button>
      <button
        type="button"
        class="row__action"
        :title="mod.enabled ? t('module.home.mods.disable') : t('module.home.mods.enable')"
        @click="emit('enable')"
      >
        <CircleSlash v-if="mod.enabled" :size="15" />
        <CircleCheck v-else :size="15" />
      </button>
      <button
        type="button"
        class="row__action row__action--danger"
        :title="t('module.home.mods.remove')"
        @click="emit('remove')"
      >
        <Trash2 :size="15" />
      </button>
    </span>
  </div>
</template>

<style scoped>
/* 行本身没有边框和底色：悬停先给一层很薄的色阶，选中再叠阴影。 */
.row {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  width: 100%;
  min-width: 0;
  min-height: 56px;
  padding: var(--copper-space-2) var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  background: transparent;
  cursor: pointer;
  outline: none;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    box-shadow var(--copper-duration-fast) var(--copper-easing);
}

.row:hover {
  background: var(--copper-surface-2);
}

.row:focus-visible {
  outline: 2px solid color-mix(in srgb, var(--copper-accent) 60%, transparent);
  outline-offset: -2px;
}

.row--selected,
.row--selected:hover {
  background: color-mix(in srgb, var(--copper-accent) 12%, var(--copper-surface));
  box-shadow: var(--copper-shadow-raised);
}

.row__icon {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 34px;
  height: 34px;
  overflow: hidden;
  border-radius: var(--copper-radius-sm);
  background: color-mix(in srgb, var(--copper-text) 6%, transparent);
  color: var(--copper-text-secondary);
}

.row__icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.row--off .row__icon {
  filter: grayscale(0.7);
  opacity: 0.7;
}

.row__text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.row__name {
  font-size: var(--copper-font-size-md);
  font-weight: 500;
  line-height: 1.35;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row--off .row__name {
  color: var(--copper-text-secondary);
}

.row__desc {
  font-size: var(--copper-font-size-xs);
  line-height: 1.4;
  color: var(--copper-text-disabled);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.row__version {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-disabled);
  font-variant-numeric: tabular-nums;
}

.row__off {
  padding: 1px 6px;
  border-radius: var(--copper-radius-full);
  background: color-mix(in srgb, var(--copper-text) 7%, transparent);
  color: var(--copper-text-disabled);
}

.row__update {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  padding: 1px 6px;
  border-radius: var(--copper-radius-full);
  background: color-mix(in srgb, var(--copper-accent) 16%, transparent);
  color: color-mix(in srgb, var(--copper-accent) 85%, var(--copper-text));
}

.row__update-arrow {
  font-size: 10px;
  line-height: 1;
}

/* 悬停 / 选中才出现的操作区；用宽度占位避免出现时整行文字跳动。 */
.row__actions {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 2px;
  opacity: 0;
  pointer-events: none;
  transition: opacity var(--copper-duration-fast) var(--copper-easing);
}

.row:hover .row__actions,
.row--selected .row__actions,
.row:focus-within .row__actions {
  opacity: 1;
  pointer-events: auto;
}

.row__action {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 28px;
  height: 28px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.row__action:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.row__action--danger:hover {
  background: color-mix(in srgb, var(--copper-danger) 14%, transparent);
  color: var(--copper-danger);
}
</style>