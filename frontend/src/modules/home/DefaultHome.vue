<script setup lang="ts">
// 开始页 · 默认模式：左侧启动区 + 列表区，右侧小组件栏。
//
// 本轮版面对齐设计稿第 7 屏，并统一遵守「无描边」：所有分块靠底色阶差 + 阴影分层。
//
// - 启动区：矩形卡片，**不写"启动"标题**。上半是实例切换（图标 + 名字），
//   下半是「开始游戏 / 退出」与「详情」（详情只有图标）。
// - 列表区：未展开是一行「图标 + 名字」、无背景、文字向右铺满；
//   点击展开成封面大卡 —— 黑体大标题 + 来源实例，标题过宽时改为滚动（marquee），
//   右下是「启动」与设置图标（设置按钮在启动左边），设置菜单里可顶置 / 切换预览图。
// - 小组件：本轮只落地「模组管理」——标题 + 进入，列表最多同屏 5 行，超出滚动，
//   可更新的模组带徽标。
//
// 文本统一经 `McText` 渲染：世界名 / 服务器名 / 模组名里的 § 样式码不能原样漏出来。

import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import {
  ArrowRight,
  Play,
  PlugZap,
  RefreshCw,
  Square,
  LoaderCircle,
  Settings2,
  ChevronRight,
  Folder,
  Globe2,
  Puzzle,
  Syringe,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import { useSettings } from "../../composables/useSettings";
import CoInstanceSelect from "../../components/ui/CoInstanceSelect.vue";
import McText from "./McText.vue";
import {
  homeDashboardEntries,
  homeModsList,
  homeModsUpdates,
  type ClientView,
  type DashboardEntry,
  type ModView,
  type VersionView,
} from "../../api/home";
import { onModsChanged } from "../../events";
import type { GameSessionState } from "../../composables/useGameSession";

const props = defineProps<{
  versions: VersionView[];
  selectedName: string;
  session: GameSessionState;
  launching: boolean;
  running: boolean;
  stopping: boolean;
  welcome: string;
  /** 客户端模式：开启后版本切换变成「客户端 -> 注入 -> 实例」两个下拉框。 */
  clientMode: boolean;
  clients: ClientView[];
  selectedClientId: string;
  reinjecting: boolean;
  /** 「重新注入」按钮是否可见（客户端模式 + 进程已创建）。 */
  reinjectVisible: boolean;
}>();

const emit = defineEmits<{
  select: [name: string];
  launch: [name: string];
  stop: [name: string];
  detail: [name: string];
  selectClient: [id: string];
  reinject: [];
}>();

const { t } = useI18n();
const router = useRouter();
const settings = useSettings();

/** 默认模式列表里最多同时展开一张卡，展开态需要量标题宽度做 marquee。 */
const SETTINGS_PINNED = "home.dashboard.pinned";

// ---------------------------------------------------------------- 实例选择

const versionOptions = computed(() =>
  props.versions.map((v) => ({
    value: v.name,
    label: v.name,
    icon: v.logo_data_url,
  })),
);

/** 客户端下拉框选项（只有文件仍在的客户端可选）。 */
const clientOptions = computed(() =>
  props.clients
    .filter((client) => client.available)
    .map((client) => ({ value: client.id, label: client.name, icon: null })),
);

// ---------------------------------------------------------------- 列表区

const entries = ref<DashboardEntry[]>([]);
const loading = ref(true);
const loadError = ref<string | null>(null);
const expandedId = ref<string | null>(null);

/** 顶置的条目 id（顺序即置顶后的展示顺序），持久化到设置。 */
const pinned = ref<string[]>([]);

function readPinned(): string[] {
  const raw = settings.get<unknown>(SETTINGS_PINNED, []);
  return Array.isArray(raw) ? raw.filter((id): id is string => typeof id === "string") : [];
}

/** 顶置在前，其余保持扫描顺序。 */
const sortedEntries = computed(() => {
  const order = new Map(pinned.value.map((id, index) => [id, index]));
  return [...entries.value].sort((a, b) => {
    const pa = order.get(a.id);
    const pb = order.get(b.id);
    if (pa !== undefined && pb !== undefined) return pa - pb;
    if (pa !== undefined) return -1;
    if (pb !== undefined) return 1;
    return 0;
  });
});

function isPinned(entry: DashboardEntry): boolean {
  return pinned.value.includes(entry.id);
}

async function togglePin(entry: DashboardEntry) {
  const list = pinned.value;
  const next = list.includes(entry.id)
    ? list.filter((id) => id !== entry.id)
    : [entry.id, ...list];
  pinned.value = next;
  menuFor.value = null;
  await settings.set(SETTINGS_PINNED, next);
}

/** 每张卡的预览图下标（同一条目可切换多张图）。 */
const previewIndex = ref<Record<string, number>>({});

function previewOf(entry: DashboardEntry): string | null {
  const images = entry.images ?? [];
  if (images.length === 0) return entry.image_data_url;
  const index = previewIndex.value[entry.id] ?? 0;
  return images[index % images.length] ?? entry.image_data_url;
}

/** 展开卡的背景图样式（没有预览图时保持纯底色）。 */
function cardStyle(entry: DashboardEntry): Record<string, string> | undefined {
  const image = previewOf(entry);
  return image ? { backgroundImage: `url("${image}")` } : undefined;
}

function cyclePreview(entry: DashboardEntry) {
  const images = entry.images ?? [];
  if (images.length <= 1) return;
  const index = previewIndex.value[entry.id] ?? 0;
  previewIndex.value = { ...previewIndex.value, [entry.id]: (index + 1) % images.length };
}

/** 展开卡的菜单：同一时刻只开一个。 */
const menuFor = ref<string | null>(null);

function toggleMenu(entry: DashboardEntry) {
  menuFor.value = menuFor.value === entry.id ? null : entry.id;
}

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

function toggleEntry(entry: DashboardEntry) {
  expandedId.value = expandedId.value === entry.id ? null : entry.id;
  menuFor.value = null;
}

function kindLabel(entry: DashboardEntry): string {
  return entry.kind === "world"
    ? t("module.home.default.world")
    : t("module.home.default.server");
}

function detailOf(name: string) {
  emit("detail", name);
}

/** 展开卡点「启动」：先把实例切到该条目所属实例，再启动。 */
function launchEntry(entry: DashboardEntry) {
  if (props.selectedName !== entry.version) emit("select", entry.version);
  emit("launch", entry.version);
}

// ---------------------------------------------------------------- marquee

const titleEl = ref<HTMLElement | HTMLElement[] | null>(null);
const marqueeShift = ref("0px");
const titleOverflow = ref(false);

/** 标题比可用宽度长多少：只量当前展开的那一张，避免 N 次布局抖动。 */
async function measureTitle() {
  await nextTick();
  // `ref` 写在 `v-for` 里时 Vue 收集成数组；这里只有展开的那一张，取第一个即可。
  const el = Array.isArray(titleEl.value) ? titleEl.value[0] : titleEl.value;
  if (!el) {
    marqueeShift.value = "0px";
    titleOverflow.value = false;
    return;
  }
  const overflow = el.scrollWidth - el.clientWidth;
  titleOverflow.value = overflow > 4;
  marqueeShift.value = overflow > 4 ? `-${overflow}px` : "0px";
}

watch(expandedId, () => void measureTitle());

// ---------------------------------------------------------------- 模组小组件

const mods = ref<ModView[]>([]);
const modsLoading = ref(false);
const updates = ref<Record<string, string>>({});

let disposeMods: (() => void) | null = null;

async function loadMods() {
  const name = props.selectedName;
  if (!name) {
    mods.value = [];
    updates.value = {};
    return;
  }
  modsLoading.value = true;
  try {
    const result = await homeModsList(name);
    mods.value = result.mods;
  } catch {
    // 实例没有模组目录 / 读取失败：当作空列表，不编造数据。
    mods.value = [];
  } finally {
    modsLoading.value = false;
  }
  // 更新检查要联网，单独跑：先出列表，徽标随后亮。
  try {
    const list = await homeModsUpdates(name);
    if (name !== props.selectedName) return;
    const map: Record<string, string> = {};
    for (const item of list) map[item.folder] = item.latest_version;
    updates.value = map;
  } catch {
    updates.value = {};
  }
}

function openMods() {
  if (!props.selectedName) return;
  void router.push({ path: "/version-settings/mods", query: { name: props.selectedName } });
}

watch(
  () => props.selectedName,
  () => void loadMods(),
);

onMounted(async () => {
  window.addEventListener("resize", measureTitle);
  pinned.value = readPinned();
  await load();
  await loadMods();
  disposeMods = await onModsChanged((name) => {
    if (name === props.selectedName) void loadMods();
  });
});

onUnmounted(() => {
  window.removeEventListener("resize", measureTitle);
  disposeMods?.();
  disposeMods = null;
});

watch(
  () => props.versions.length,
  () => void load(),
);
</script>

<template>
  <div class="dash">
    <div class="dash__main">
      <h1 class="dash__welcome">{{ welcome }}</h1>

      <!-- 启动区：矩形，无"启动"标题；上切换实例，下开始游戏 + 详情 -->
      <section class="launch">
        <!-- 客户端模式：客户端 -> 注入 -> 实例；关闭时保持单个实例下拉框。 -->
        <div v-if="clientMode" class="launch__selects">
          <CoInstanceSelect
            :model-value="selectedClientId"
            :options="clientOptions"
            :icon="PlugZap"
            :placeholder="t('module.home.client.client_select')"
            variant="block"
            class="launch__select"
            @update:model-value="emit('selectClient', $event)"
          />
          <span class="launch__inject" :title="t('module.home.client.inject_arrow')">
            <Syringe :size="15" :stroke-width="1.9" />
            <ArrowRight :size="13" :stroke-width="2.2" />
          </span>
          <CoInstanceSelect
            :model-value="selectedName"
            :options="versionOptions"
            variant="block"
            class="launch__select"
            @update:model-value="emit('select', $event)"
          />
        </div>
        <CoInstanceSelect
          v-else
          :model-value="selectedName"
          :options="versionOptions"
          variant="block"
          @update:model-value="emit('select', $event)"
        />
        <div class="launch__row">
          <button
            type="button"
            class="launch__play"
            :class="{ 'is-stop': running }"
            :disabled="launching || stopping"
            @click="running ? emit('stop', selectedName) : emit('launch', selectedName)"
          >
            <LoaderCircle v-if="launching" :size="17" class="spin" />
            <Square v-else-if="running" :size="14" />
            <Play v-else :size="16" />
            <span>{{ running ? t("module.home.stop") : t("module.home.play") }}</span>
          </button>
          <button
            v-if="reinjectVisible"
            type="button"
            class="launch__reinject"
            :title="t('module.home.client.retry_inject')"
            :aria-label="t('module.home.client.retry_inject')"
            :disabled="reinjecting"
            @click="emit('reinject')"
          >
            <RefreshCw :size="16" :class="{ spin: reinjecting }" />
            <span>{{ t("module.home.client.retry_inject") }}</span>
          </button>
          <button
            type="button"
            class="launch__detail"
            :title="t('module.home.detail')"
            :aria-label="t('module.home.detail')"
            @click="detailOf(selectedName)"
          >
            <Settings2 :size="18" />
          </button>
        </div>
      </section>

      <!-- 列表区 -->
      <section class="list">
        <header class="list__head">
          <h2 class="list__title">{{ t("module.home.default.list") }}</h2>
          <span class="list__hint">{{ t("module.home.default.list_hint") }}</span>
          <button type="button" class="list__rescan" @click="load">
            {{ t("module.home.default.rescan") }}
          </button>
        </header>

        <p v-if="loading" class="list__state">{{ t("module.home.default.scanning") }}</p>
        <p v-else-if="loadError" class="list__state list__state--error">
          {{ t("module.home.default.scan_failed", { message: loadError }) }}
        </p>
        <p v-else-if="entries.length === 0" class="list__state">
          {{ t("module.home.default.empty") }}
        </p>

        <TransitionGroup v-else name="rows" tag="div" class="list__items">
          <div
            v-for="entry in sortedEntries"
            :key="entry.id"
            :class="['row', { 'row--open': expandedId === entry.id }]"
            role="button"
            tabindex="0"
            draggable="true"
            @dragstart="dragEntry = entry.id"
            @dragover.prevent
            @drop="dropEntry(entry.id)"
            @click="toggleEntry(entry)"
            @keydown.enter="toggleEntry(entry)"
          >
            <!-- 未展开：图标 + 文本，无背景，文字向右铺满 -->
            <template v-if="expandedId !== entry.id">
              <span class="row__icon">
                <!-- 有预览图就直接用图（世界几乎都有），没有时退回图标。 -->
                <img v-if="previewOf(entry)" :src="previewOf(entry) ?? ''" alt="" />
                <Globe2 v-else-if="entry.kind === 'server'" :size="18" :stroke-width="1.9" />
                <Folder v-else :size="18" :stroke-width="1.9" />
              </span>
              <span class="row__text">
                <span class="row__name"><McText :text="entry.name" /></span>
                <span class="row__meta">
                  {{ kindLabel(entry) }} · {{ entry.version }}
                  <span v-if="isPinned(entry)" class="row__pin">· {{ t("module.home.default.pinned") }}</span>
                </span>
              </span>
              <span v-if="isPinned(entry)" class="row__pin-dot" />
            </template>

            <!-- 展开：封面 + 黑体大标题 + 来源实例 + 启动 / 设置 -->
            <template v-else>
              <div class="card" :style="cardStyle(entry)">
                <div class="card__shade" />

                <div class="card__info">
                  <span
                    ref="titleEl"
                    class="card__title"
                    :class="{ 'card__title--marquee': titleOverflow }"
                    :style="{ '--marquee-shift': marqueeShift }"
                  >
                    <span class="card__title-inner">
                      <McText :text="entry.name" on-dark />
                    </span>
                  </span>
                  <span class="card__source">
                    {{ kindLabel(entry) }} · {{ entry.version }}
                    <template v-if="entry.detail"> · {{ entry.detail }}</template>
                  </span>
                </div>

                <div class="card__actions" @click.stop>
                  <div class="card__menu-wrap">
                    <button
                      type="button"
                      class="card__icon-btn"
                      :title="t('module.home.default.configure')"
                      :aria-label="t('module.home.default.configure')"
                      @click="toggleMenu(entry)"
                    >
                      <Settings2 :size="17" />
                    </button>
                    <div v-if="menuFor === entry.id" class="card__menu">
                      <button type="button" class="card__menu-item" @click="togglePin(entry)">
                        {{ isPinned(entry) ? t("module.home.default.unpin") : t("module.home.default.pin") }}
                      </button>
                      <button
                        type="button"
                        class="card__menu-item"
                        :disabled="(entry.images?.length ?? 0) <= 1"
                        :title="(entry.images?.length ?? 0) <= 1 ? t('module.home.default.preview_single') : ''"
                        @click="cyclePreview(entry)"
                      >
                        {{ t("module.home.default.switch_preview") }}
                      </button>
                    </div>
                  </div>

                  <button
                    type="button"
                    class="card__play"
                    :disabled="launching || stopping"
                    @click="launchEntry(entry)"
                  >
                    <LoaderCircle v-if="launching" :size="16" class="spin" />
                    <Play v-else :size="15" />
                    <span>{{ t("module.home.default.play") }}</span>
                  </button>
                </div>
              </div>
            </template>
          </div>
        </TransitionGroup>
      </section>
    </div>

    <!-- 小组件区：本轮只落地模组管理 -->
    <aside class="dash__widgets">
      <section class="widget">
        <header class="widget__head">
          <h3 class="widget__title">{{ t("module.home.widget.mods.title") }}</h3>
          <button type="button" class="widget__enter" @click="openMods">
            <span>{{ t("module.home.widget.mods.enter") }}</span>
            <ChevronRight :size="14" />
          </button>
        </header>

        <p v-if="modsLoading && mods.length === 0" class="widget__state">
          {{ t("common.loading") }}
        </p>
        <p v-else-if="mods.length === 0" class="widget__state">
          {{ t("module.home.mods.empty") }}
        </p>

        <ul v-else class="widget__list">
          <li v-for="mod in mods" :key="mod.folder" class="mod">
            <span class="mod__icon">
              <Puzzle :size="15" />
            </span>
            <span class="mod__main">
              <span class="mod__name"><McText :text="mod.name" /></span>
              <span class="mod__meta">{{ mod.version }}</span>
            </span>
            <span
              v-if="updates[mod.folder]"
              class="mod__badge"
              :title="t('module.home.widget.mods.update_hint', { version: updates[mod.folder] })"
            >
              {{ t("module.home.widget.mods.update") }}
            </span>
          </li>
        </ul>
      </section>
    </aside>
  </div>
</template>

<style scoped>
.dash {
  display: grid;
  /* 左右 6 : 4。 */
  grid-template-columns: minmax(0, 3fr) minmax(0, 2fr);
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
  font-size: 26px;
  font-weight: 700;
  line-height: 1.3;
}

/* ---------------------------------------------------------------- 启动区 */

/* 矩形卡片：无描边，底色阶差 + 阴影分层。 */
.launch {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding: var(--copper-space-4);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow-subtle);
}

.launch__selects {
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
}

.launch__select {
  flex: 1;
  min-width: 0;
}

/* 注入方向提示：客户端 -> 实例。无描边，只用强调色与箭头表达方向。 */
.launch__inject {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 1px;
  padding: 0 2px;
  color: var(--copper-accent);
  opacity: 0.85;
}

.launch__row {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.launch__play {
  flex: 1;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-2);
  height: 52px;
  border: none;
  border-radius: var(--copper-radius-md);
  background: var(--copper-accent);
  color: var(--copper-accent-foreground);
  font-family: inherit;
  font-size: 15px;
  font-weight: 600;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

.launch__play:hover:not(:disabled) {
  background: color-mix(in srgb, var(--copper-accent) 88%, #fff);
}

.launch__play:active:not(:disabled) {
  transform: translateY(1px);
}

.launch__play.is-stop {
  background: var(--copper-danger);
  color: #fff;
}

.launch__play:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.launch__reinject {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  height: 52px;
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  white-space: nowrap;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.launch__reinject:hover:not(:disabled) {
  background: var(--copper-surface-3);
  color: var(--copper-text);
}

.launch__reinject:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

.launch__detail {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 52px;
  height: 52px;
  border: none;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.launch__detail:hover {
  background: var(--copper-surface-3);
  color: var(--copper-text);
}

/* ---------------------------------------------------------------- 列表区 */

.list {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.list__head {
  display: flex;
  align-items: baseline;
  gap: var(--copper-space-2);
  padding: 0 var(--copper-space-1);
}

.list__title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
}

.list__hint {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
}

.list__rescan {
  margin-left: auto;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
}

.list__rescan:hover {
  color: var(--copper-text);
}

.list__state {
  padding: var(--copper-space-5);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  text-align: center;
}

.list__state--error {
  color: var(--copper-danger);
}

.list__items {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

/* 未展开行：无背景，hover 才浮一层底色。 */
.row {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  min-height: 52px;
  padding: var(--copper-space-1) var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  cursor: pointer;
  user-select: none;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.row:hover {
  background: var(--copper-hover);
}

.row--open {
  display: block;
  padding: 0;
  background: transparent;
}

.row__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 40px;
  height: 40px;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  overflow: hidden;
}

/* 世界预览图当行首图标：比通用图标更容易认，也顺手解决"图标像是空的"。 */
.row__icon img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

/* 文本向右铺满：flex:1 + min-width:0，长名字截断而不是把行撑宽。 */
.row__text {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.row__name {
  overflow: hidden;
  font-size: var(--copper-font-size-md);
  font-weight: 500;
  white-space: nowrap;
  text-overflow: ellipsis;
}

/* 次要信息用 secondary 而不是 disabled：后者在浅色底上只有 3:1，读起来吃力。 */
.row__meta {
  overflow: hidden;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
  text-overflow: ellipsis;
}

.row__pin {
  color: var(--copper-accent);
}

.row__pin-dot {
  flex-shrink: 0;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--copper-accent);
}

/* 展开卡：封面铺满，底部渐变压住标题。 */
.card {
  position: relative;
  height: 240px;
  border-radius: var(--copper-radius-lg);
  background-color: var(--copper-surface-2);
  background-position: center;
  background-size: cover;
  overflow: hidden;
  box-shadow: var(--copper-shadow);
}

.card__shade {
  position: absolute;
  inset: 0;
  background: linear-gradient(
    to top,
    color-mix(in srgb, #000 78%, transparent) 0%,
    color-mix(in srgb, #000 38%, transparent) 42%,
    transparent 76%
  );
  pointer-events: none;
}

.card__info {
  position: absolute;
  left: var(--copper-space-4);
  right: 180px;
  bottom: var(--copper-space-4);
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

/* 黑体大标题：过宽时由 measureTitle 写入 --marquee-shift，内层左右滚动。 */
.card__title {
  overflow: hidden;
  color: #fff;
  font-size: 24px;
  font-weight: 800;
  line-height: 1.2;
  white-space: nowrap;
  text-shadow: 0 2px 8px rgba(0, 0, 0, 0.5);
}

.card__title-inner {
  display: inline-block;
  min-width: 100%;
}

.card__title--marquee .card__title-inner {
  animation: marquee 9s ease-in-out infinite alternate;
}

@keyframes marquee {
  from {
    transform: translateX(0);
  }
  to {
    transform: translateX(var(--marquee-shift, 0));
  }
}

.card__source {
  overflow: hidden;
  color: rgba(255, 255, 255, 0.78);
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
  text-overflow: ellipsis;
}

/* 右下动作：设置图标在启动按钮左边。 */
.card__actions {
  position: absolute;
  right: var(--copper-space-4);
  bottom: var(--copper-space-4);
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

.card__menu-wrap {
  position: relative;
}

.card__icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 44px;
  height: 44px;
  border: none;
  border-radius: var(--copper-radius-md);
  background: rgba(0, 0, 0, 0.42);
  color: #fff;
  cursor: pointer;
  backdrop-filter: blur(6px);
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.card__icon-btn:hover {
  background: rgba(0, 0, 0, 0.6);
}

.card__menu {
  position: absolute;
  right: 0;
  bottom: calc(100% + 8px);
  z-index: 20;
  min-width: 148px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 6px;
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow);
}

.card__menu-item {
  height: 34px;
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  text-align: left;
  cursor: pointer;
}

.card__menu-item:hover:not(:disabled) {
  background: var(--copper-hover);
}

.card__menu-item:disabled {
  color: var(--copper-text-disabled);
  cursor: not-allowed;
}

.card__play {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-2);
  height: 44px;
  padding: 0 var(--copper-space-4);
  border: none;
  border-radius: var(--copper-radius-md);
  background: var(--copper-accent);
  color: var(--copper-accent-foreground);
  font-family: inherit;
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.card__play:hover:not(:disabled) {
  background: color-mix(in srgb, var(--copper-accent) 88%, #fff);
}

.card__play:disabled {
  opacity: 0.6;
  cursor: not-allowed;
}

/* ---------------------------------------------------------------- 小组件 */

.dash__widgets {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  min-width: 0;
}

.widget {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding: var(--copper-space-4);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow-subtle);
}

.widget__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-2);
}

.widget__title {
  font-size: var(--copper-font-size-md);
  font-weight: 600;
}

.widget__enter {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  height: 28px;
  padding: 0 var(--copper-space-2);
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-xs);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.widget__enter:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.widget__state {
  padding: var(--copper-space-3);
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
  text-align: center;
}

/* 列表最多同屏 5 行（5 × 48 + 间隙），超出在框内滚动。 */
.widget__list {
  list-style: none;
  max-height: 250px;
  overflow-y: auto;
  overscroll-behavior: contain;
  display: flex;
  flex-direction: column;
  gap: 2px;
  margin: 0 calc(-1 * var(--copper-space-2));
  padding: 0 var(--copper-space-2);
}

.mod {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  min-height: 48px;
  padding: 0 var(--copper-space-1);
  border-radius: var(--copper-radius-sm);
}

.mod:hover {
  background: var(--copper-hover);
}

.mod__icon {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border-radius: var(--copper-radius-sm);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
}

.mod__main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.mod__name {
  overflow: hidden;
  font-size: var(--copper-font-size-sm);
  white-space: nowrap;
  text-overflow: ellipsis;
}

.mod__meta {
  overflow: hidden;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  white-space: nowrap;
  text-overflow: ellipsis;
}

.mod__badge {
  flex-shrink: 0;
  padding: 2px 8px;
  border-radius: var(--copper-radius-full);
  background: color-mix(in srgb, var(--copper-accent) 18%, transparent);
  color: color-mix(in srgb, var(--copper-accent) 85%, var(--copper-text));
  font-size: var(--copper-font-size-xs);
}

@container (max-width: 760px) {
  .dash {
    grid-template-columns: minmax(0, 1fr);
  }

  .card__info {
    right: var(--copper-space-4);
    bottom: 76px;
  }
}

.rows-move {
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