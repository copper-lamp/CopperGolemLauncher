<script setup lang="ts">
// 开始页 · 默认模式：左侧启动条 + 列表区，右侧小组件栏。
//
// 版面来自设计稿第 7 屏：
// - 顶部欢迎语；
// - 启动条：胶囊版本选择 + 菜单（详情）+ 启动；
// - 列表区：默认一行一个「图标 + 名字」，点击展开成封面大卡（2x2）；
//   大卡封面铺满，底部压一层自下而上的渐变衬托名字 / 实例 / 地址；
// - 右侧小组件栏：账户、实例、下载。
//
// 启动区与列表区共用 CoBoard 容器；列表行可拖拽重排，
// 重排动画由 TransitionGroup 的 move 过渡提供。

import { onMounted, ref, watch } from "vue";
import { Play, Square, LoaderCircle, Settings2, ChevronDown, UserRound, Layers, ArrowDownToLine, Globe2, Folder } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { useAccount } from "../../composables/useAccount";
import { useDownloads } from "../../composables/useDownloads";
import CoBoard from "../../components/layout/CoBoard.vue";
import { homeDashboardEntries, type DashboardEntry, type VersionView } from "../../api/home";
import type { GameSessionState } from "../../composables/useGameSession";

const props = defineProps<{
  versions: VersionView[];
  selectedName: string;
  session: GameSessionState;
  launching: boolean;
  running: boolean;
  stopping: boolean;
  welcome: string;
}>();

const emit = defineEmits<{
  select: [name: string];
  launch: [name: string];
  stop: [name: string];
  detail: [name: string];
}>();

const { t } = useI18n();
const { account } = useAccount();
const { activeCount } = useDownloads();

const entries = ref<DashboardEntry[]>([]);
const loading = ref(true);
const loadError = ref<string | null>(null);
const expandedId = ref<string | null>(null);

async function load() {
  loading.value = true;
  loadError.value = null;
  try {
    entries.value = await homeDashboardEntries();
  } catch (e) {
    loadError.value = String(e).replace(/^Error:\s*/, "");
  } finally {
    loading.value = false;
  }
}

onMounted(load);
watch(() => props.versions.length, () => void load());

const dragEntry = ref<string | null>(null);

function dropEntry(target: string) {
  const from = dragEntry.value;
  dragEntry.value = null;
  if (!from || from === target) return;
  const list = entries.value;
  const a = list.findIndex((e) => e.id === from);
  const b = list.findIndex((e) => e.id === target);
  if (a < 0 || b < 0) return;
  const [item] = list.splice(a, 1);
  list.splice(b, 0, item);
}

function tileStyle(entry: DashboardEntry): Record<string, string> {
  if (!entry.image_data_url) return {};
  return { backgroundImage: 'url("' + entry.image_data_url + '")' };
}

function toggleEntry(entry: DashboardEntry) {
  expandedId.value = expandedId.value === entry.id ? null : entry.id;
}

function kindLabel(entry: DashboardEntry): string {
  return entry.kind === "world"
    ? t("module.home.default.world")
    : t("module.home.default.server");
}

function detailOf(name: string) {
  emit("detail", name);
}
</script>

<template>
  <div class="dash">
    <div class="dash__main">
      <h1 class="dash__welcome">{{ welcome }}</h1>

      <!-- 启动区 -->
      <CoBoard :title="t('module.home.default.launch')">
        <div class="dash__launch">
          <label class="dash__version">
            <select
              class="dash__select"
              :value="selectedName"
              @change="emit('select', ($event.target as HTMLSelectElement).value)"
            >
              <option v-for="v in versions" :key="v.name" :value="v.name">
                {{ v.name }}
              </option>
            </select>
            <ChevronDown :size="14" class="dash__select-icon" />
          </label>

          <button
            type="button"
            class="dash__menu"
            :title="t('module.home.detail')"
            @click="detailOf(selectedName)"
          >
            <Settings2 :size="16" />
          </button>

          <button
            type="button"
            class="dash__play"
            :class="{ 'is-stop': running }"
            :disabled="launching || stopping"
            @click="running ? emit('stop', selectedName) : emit('launch', selectedName)"
          >
            <LoaderCircle v-if="launching" :size="15" class="spin" />
            <Square v-else-if="running" :size="12" />
            <Play v-else :size="14" />
            <span>{{ running ? t('module.home.stop') : t('module.home.play') }}</span>
          </button>
        </div>
      </CoBoard>

      <!-- 列表区 -->
      <CoBoard :title="t('module.home.default.list')" :hint="t('module.home.default.list_hint')">
        <template #actions>
          <button type="button" class="dash__rescan" @click="load">
            {{ t('module.home.default.rescan') }}
          </button>
        </template>

        <p v-if="loading" class="dash__state">{{ t('module.home.default.scanning') }}</p>
        <p v-else-if="loadError" class="dash__state dash__state--error">
          {{ t('module.home.default.scan_failed', { message: loadError }) }}
        </p>
        <p v-else-if="entries.length === 0" class="dash__state">{{ t('module.home.default.empty') }}</p>

        <TransitionGroup v-else name="list" tag="div" class="dash__entries">
          <div
            v-for="entry in entries"
            :key="entry.id"
            class="entry"
            :class="{ 'entry--open': expandedId === entry.id }"
            :style="tileStyle(entry)"
            role="button"
            tabindex="0"
            draggable="true"
            @dragstart="dragEntry = entry.id"
            @dragover.prevent
            @drop="dropEntry(entry.id)"
            @click="toggleEntry(entry)"
            @keydown.enter="toggleEntry(entry)"
          >
            <div class="entry__shade" />
            <span class="entry__icon">
              <Globe2 v-if="entry.kind === 'server'" :size="16" />
              <Folder v-else :size="16" />
            </span>
            <div class="entry__text">
              <span class="entry__name">{{ entry.name }}</span>
              <span class="entry__meta">
                {{ kindLabel(entry) }} · {{ entry.version }}
                <template v-if="expandedId === entry.id"> · {{ entry.detail }}</template>
              </span>
            </div>
          </div>
        </TransitionGroup>
      </CoBoard>
    </div>

    <!-- 小组件区 -->
    <aside class="dash__widgets">
      <div class="widget">
        <UserRound :size="16" class="widget__icon" />
        <span class="widget__label">{{ t('module.home.default.account') }}</span>
        <span class="widget__value">{{ account?.gamertag ?? t('module.home.default.offline') }}</span>
      </div>
      <div class="widget">
        <Layers :size="16" class="widget__icon" />
        <span class="widget__label">{{ t('module.home.default.instances') }}</span>
        <span class="widget__value">{{ versions.length }}</span>
        <span class="widget__hint">{{ selectedName || t('module.home.default.none') }}</span>
      </div>
      <div class="widget">
        <ArrowDownToLine :size="16" class="widget__icon" />
        <span class="widget__label">{{ t('module.home.default.downloads') }}</span>
        <span class="widget__value">
          {{ activeCount() > 0 ? t('module.home.default.downloads_active', { count: activeCount() }) : t('module.home.default.downloads_idle') }}
        </span>
      </div>
    </aside>
  </div>
</template>

<style scoped>
.dash {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 260px;
  gap: var(--copper-space-5);
  align-items: start;
  padding-top: var(--copper-space-2);
}

.dash__main {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-5);
}

.dash__welcome {
  font-size: 22px;
  font-weight: 700;
  line-height: 1.3;
}

/* 启动条：与简洁模式同一套胶囊语言。 */
.dash__launch {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  padding: 6px;
  padding-left: var(--copper-space-4);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface);
}

.dash__version {
  position: relative;
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
}

.dash__select {
  width: 100%;
  height: 40px;
  padding: 0 var(--copper-space-5) 0 var(--copper-space-2);
  border: none;
  outline: none;
  background: transparent;
  color: var(--copper-text);
  font-family: inherit;
  font-size: var(--copper-font-size-lg);
  font-weight: 500;
  appearance: none;
  cursor: pointer;
}

.dash__select-icon {
  position: absolute;
  right: var(--copper-space-2);
  color: var(--copper-text-secondary);
  pointer-events: none;
}

.dash__menu {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 36px;
  height: 36px;
  border: none;
  border-radius: var(--copper-radius-full);
  background: transparent;
  color: var(--copper-text-secondary);
  cursor: pointer;
}

.dash__menu:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.dash__play {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 40px;
  padding: 0 var(--copper-space-5);
  border: none;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
  color: var(--copper-accent-foreground);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  cursor: pointer;
}

.dash__play.is-stop {
  background: var(--copper-danger);
}

/* 列表区：默认一行一个，展开后成为 2x2 封面卡。 */
.dash__entries {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--copper-space-2);
}

.entry {
  position: relative;
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  min-height: 52px;
  padding: var(--copper-space-2) var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  background-color: var(--copper-surface);
  background-position: center;
  background-size: cover;
  cursor: pointer;
  user-select: none;
}

.entry:hover {
  border-color: color-mix(in srgb, var(--copper-accent) 40%, var(--copper-border));
}

.entry--open {
  grid-column: span 2;
  grid-row: span 2;
  min-height: 180px;
  align-items: flex-end;
}

.entry__shade {
  position: absolute;
  inset: 0;
  z-index: 1;
  border-radius: inherit;
  background: linear-gradient(
    to top,
    color-mix(in srgb, #000 74%, transparent) 0%,
    color-mix(in srgb, #000 30%, transparent) 46%,
    transparent 78%
  );
  opacity: 0;
  transition: opacity var(--copper-duration) var(--copper-easing);
  pointer-events: none;
}

.entry--open .entry__shade {
  opacity: 1;
}

.entry__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  z-index: 2;
}

.entry--open .entry__icon {
  display: none;
}

.entry__text {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
  z-index: 2;
}

.entry__name {
  overflow: hidden;
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.entry__meta {
  overflow: hidden;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
  text-overflow: ellipsis;
}

.entry--open .entry__name,
.entry--open .entry__meta {
  color: #fff;
  text-shadow: 0 1px 2px rgba(0, 0, 0, 0.45);
}

.dash__rescan {
  margin-left: auto;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
}

.dash__state {
  padding: var(--copper-space-4);
  border: 1px dashed var(--copper-border);
  border-radius: var(--copper-radius-lg);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  text-align: center;
}

.dash__state--error {
  color: var(--copper-danger);
}

/* 右侧小组件栏 */
.dash__widgets {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.widget {
  display: grid;
  grid-template-columns: auto 1fr;
  align-items: center;
  gap: 2px var(--copper-space-2);
  padding: var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
}

.widget__icon {
  grid-row: span 2;
  color: var(--copper-accent);
}

.widget__label {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.widget__value {
  overflow: hidden;
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.widget__hint {
  grid-column: 2;
  overflow: hidden;
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
  text-overflow: ellipsis;
}

@container (max-width: 760px) {
  .dash {
    grid-template-columns: minmax(0, 1fr);
  }

  .dash__entries {
    grid-template-columns: minmax(0, 1fr);
  }
}

.list-move {
  transition: transform var(--copper-duration) var(--copper-easing);
}

.spin {
  animation: spin 1.2s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}
</style>