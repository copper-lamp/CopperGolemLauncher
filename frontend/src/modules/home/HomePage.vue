<script setup lang="ts">
// 开始页 · 简洁模式：页面中央竖直排版 文案 → 版本名 → 主按钮 → 版本设置。
// 主按钮上方是版本名，下方是版本设置；启动确认期间主按钮内嵌环形进度，
// 按钮下方补一条随机提示，避免「点了没反应」的空白等待。
// 左上角为 简洁 / 默认 模式切换；默认模式（Win10 磁贴风格）本次不实现，展示占位。
//
// 多个版本时版本名以选择器呈现（核心功能「选择版本」），否则仅显示版本名。

import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { Play, Gamepad2, Download, Square } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { useSettings } from "../../composables/useSettings";
import {
  beginLaunch,
  endSession,
  gameSessionState,
  gameSessionName,
  syncSessionState,
  type GameSessionState,
} from "../../composables/useGameSession";
import TipsRotator from "../../components/TipsRotator.vue";
import CoButton from "../../components/ui/CoButton.vue";
import CoRingProgress from "../../components/ui/CoRingProgress.vue";
import CoSegmented from "../../components/ui/CoSegmented.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import {
  homeGameKill,
  homeLaunch,
  homeVersionsList,
  type VersionView,
} from "../../api/home";
import { onVersionInstalled, onVersionRemoved, onVersionsChanged } from "../../events";

type HomeMode = "simple" | "default";

const { t } = useI18n();
const router = useRouter();

const mode = ref<HomeMode>("simple");
const versions = ref<VersionView[]>([]);
const selectedName = ref("");
const settings = useSettings();
const loading = ref(true);

const modeOptions = computed(() => [
  { value: "simple", label: t("module.home.mode.simple") },
  { value: "default", label: t("module.home.mode.default") },
]);

/** 当前版本（多版本时可经选择器切换）。 */
const current = computed(() => {
  if (!selectedName.value) return null;
  return (
    versions.value.find((v) => v.name === selectedName.value) ?? null
  );
});

const versionOptions = computed(() =>
  versions.value.map((v) => ({ value: v.name, label: v.name })),
);

/** 会话状态只对当前选中版本生效，避免 A 实例的状态染到 B 的按钮上。 */
const session = computed<GameSessionState>(() =>
  gameSessionName.value === selectedName.value ? gameSessionState.value : "idle",
);

/** 启动确认中：按钮内嵌环形进度，不可再次点击。 */
const launching = computed(() => session.value === "launching");
/** 已在运行：按钮转为「退出」。 */
const running = computed(() => session.value === "running");
/** 结束游戏请求进行中（终止进程到进程真正消失之间的短暂窗口）。 */
const stopping = ref(false);

const primaryLabel = computed(() => {
  if (launching.value) return t("module.home.launching");
  if (running.value) return t("module.home.stop");
  return t("module.home.play");
});

onMounted(async () => {
  await refresh();
  void syncSessionState(selectedName.value);
  // 版本安装 / 删除事件直达刷新，无需重启。
  unlisten = [
    await onVersionInstalled(() => void refresh()),
    await onVersionRemoved(() => void refresh()),
    await onVersionsChanged(() => void refresh()),
  ];
});

let unlisten: Array<() => void> = [];

onUnmounted(() => {
  unlisten.forEach((u) => u());
  unlisten = [];
});

async function refresh() {
  try {
    versions.value = await homeVersionsList();
    if (!versions.value.some((v) => v.name === selectedName.value)) {
      // 首选开始页上次的选择（`launch.default_version`）；设置被清空时退回首个版本。
      const stored = settings.get<string>("launch.default_version", "").trim();
      selectedName.value =
        versions.value.find((v) => v.name === stored)?.name ??
        versions.value[0]?.name ??
        "";
    }
  } catch (e) {
    showToast(String(e), "error");
  } finally {
    loading.value = false;
  }
}

// 开始页的版本选择即「当前实例」，必须落盘：
// 内容下载的落点取的就是这个值（后端 `launch.default_version`）。不落盘的话
// 详情页会解析不到目标版本，把内容丢进系统下载目录，而用户明明在开始页选好了。
watch(selectedName, (name) => {
  if (!name) return;
  if (settings.get<string>("launch.default_version", "") === name) return;
  void settings.set("launch.default_version", name);
  // 切换实例后对齐一次真实进程状态（启动器重启后游戏可能仍在运行）。
  void syncSessionState(name);
});

async function onPrimaryClick() {
  if (!current.value || launching.value || stopping.value) return;
  if (running.value) {
    await stop();
  } else {
    await launch();
  }
}

async function launch() {
  const version = current.value;
  if (!version) return;
  // 先进入「启动中」再发命令：按钮形态由内核事件收尾，中途失败会在
  // 下面的 catch 里复位，不留「点不动」的按钮。
  beginLaunch(version.name);
  try {
    await homeLaunch(version.name);
  } catch (e) {
    // 命令本身就失败 ⇒ 后端没有起监控任务，不会再有事件来收尾，必须手动复位。
    endSession();
    showToast(t("module.home.toast.launch_failed", { message: String(e) }), "error");
  }
}

async function stop() {
  const version = current.value;
  if (!version) return;
  stopping.value = true;
  try {
    await homeGameKill(version.name);
    showToast(t("module.home.toast.stopped"), "success");
  } catch (e) {
    showToast(t("module.home.toast.stop_failed", { message: String(e) }), "error");
    // 终止失败时以真实进程状态为准：可能游戏已经自己退了。
    void syncSessionState(version.name);
  } finally {
    stopping.value = false;
  }
}

function openVersionSettings() {
  if (!current.value) return;
  void router.push({
    path: "/version-settings",
    query: { name: current.value.name },
  });
}

function goDownload() {
  void router.push("/game-download");
}
</script>

<template>
  <div class="home-page">
    <!-- 模式切换注入全局标题栏操作区 -->
    <Teleport to="#copper-titlebar-actions">
      <CoSegmented v-model="mode" :options="modeOptions" />
    </Teleport>

    <!-- 默认模式占位（后续阶段实现） -->
    <div v-if="mode === 'default'" class="home-page__default-placeholder">
      <Gamepad2 :size="40" :stroke-width="1.5" />
      <p>{{ t("module.home.mode.default_placeholder") }}</p>
    </div>

    <!-- 简洁模式 -->
    <div v-else class="home-page__simple">
      <template v-if="loading">
        <p class="home-page__hint">{{ t("common.loading") }}</p>
        <!-- 版本清单扫描期间展示内核随机提示，避免空白等待 -->
        <TipsRotator compact />
      </template>

      <template v-else-if="!current">
        <h1 class="home-page__hero">{{ t("module.home.hero_title") }}</h1>
        <p class="home-page__empty">{{ t("module.home.empty") }}</p>
        <p class="home-page__hint">{{ t("module.home.install_hint") }}</p>
        <div class="home-page__actions">
          <CoButton variant="primary" size="md" @click="goDownload">
            <Download :size="16" />
            <span>{{ t("module.home.go_download") }}</span>
          </CoButton>
        </div>
      </template>

      <template v-else>
        <h1 class="home-page__hero">{{ t("module.home.hero_title") }}</h1>
        <div class="home-page__version">
          <CoSelect
            v-if="versionOptions.length > 1"
            v-model="selectedName"
            :options="versionOptions"
            class="home-page__version-select"
          />
          <span v-else class="home-page__version-name">{{ current.name }}</span>
        </div>
        <div class="home-page__actions">
          <CoButton
            class="home-page__primary"
            :variant="running ? 'danger' : 'primary'"
            size="md"
            :disabled="launching || stopping"
            @click="onPrimaryClick"
          >
            <CoRingProgress v-if="launching" :size="16" :stroke="2" />
            <Square v-else-if="running" :size="14" />
            <Play v-else :size="16" />
            <span>{{ primaryLabel }}</span>
          </CoButton>
          <CoButton variant="ghost" size="md" @click="openVersionSettings">
            {{ t("module.home.settings") }}
          </CoButton>
        </div>
        <!-- 启动确认期间在按钮下方给一条提示：这段时间可达数十秒，不能留白 -->
        <TipsRotator v-if="launching" compact />
      </template>
    </div>
  </div>
</template>

<style scoped>
.home-page {
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  overflow-y: auto;
}

.home-page__simple {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-4);
  padding: var(--copper-space-6);
  max-width: 480px;
}

.home-page__hero {
  font-size: 28px;
  font-weight: 700;
  letter-spacing: 0.02em;
}

.home-page__version {
  min-height: var(--copper-control-h);
  display: flex;
  align-items: center;
  justify-content: center;
}

.home-page__version-name {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-lg);
}

.home-page__version-select :deep(select) {
  background: transparent;
  border-color: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-lg);
  font-weight: 500;
  height: calc(var(--copper-control-h) + 4px);
}

.home-page__actions {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-2);
  margin-top: var(--copper-space-2);
}

/* 主按钮在竖排里独占一行，「版本设置」紧随其下。 */
.home-page__actions .home-page__primary {
  min-width: 160px;
  height: 44px;
  font-size: var(--copper-font-size-lg);
  border-radius: var(--copper-radius-lg);
}

.home-page__actions .home-page__primary.co-btn--primary {
  box-shadow: 0 4px 18px color-mix(in srgb, var(--copper-accent) 35%, transparent);
}

.home-page__empty-icon {
  color: var(--copper-text-disabled);
}

.home-page__empty {
  color: var(--copper-text-secondary);
  text-align: center;
}

.home-page__hint {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
  text-align: center;
}

.home-page__default-placeholder {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: var(--copper-space-3);
  color: var(--copper-text-secondary);
}
</style>
