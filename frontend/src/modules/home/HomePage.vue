<script setup lang="ts">
// 开始页 · 简洁模式（V3）：欢迎语 + 胶囊操作条。
//
// - 欢迎语共 20 条，按本地时间每 6 小时换一条：用「当前时刻 / 6h」取槽位，
//   不需要持久化，也不会因为重启而跳回第一条；分钟级定时器只负责跨槽时刷新。
// - 胶囊操作条：左侧版本选择（单版本时退化为纯文本），右侧「详情」与「启动」。
//   版本选择即当前实例，必须落盘（content download 的落点取自它）。
// - 启动中在操作条下方显示内核随机提示；运行中主按钮变为「退出游戏」。
// - 默认模式（磁贴 / 拖拽容器）尚未实现，保持占位。
//
// 布局交给 CoPage：本页不写 padding / overflow，简单模式内部再撑满高度居中。

import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { Play, Download, Square, Settings2 } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { useSettings } from "../../composables/useSettings";
import { useAccount } from "../../composables/useAccount";
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
import CoInstanceSelect from "../../components/ui/CoInstanceSelect.vue";
import CoPage from "../../components/layout/CoPage.vue";
import DefaultHome from "./DefaultHome.vue";
import {
  homeGameKill,
  homeLaunch,
  homeVersionsList,
  type VersionView,
} from "../../api/home";
import { onVersionInstalled, onVersionRemoved, onVersionsChanged } from "../../events";

type HomeMode = "simple" | "default";

/** 欢迎语条数与轮换周期。 */
const WELCOME_COUNT = 20;
const WELCOME_PERIOD_MS = 6 * 60 * 60 * 1000;
const WELCOME_TICK_MS = 60 * 1000;

const { t } = useI18n();
const router = useRouter();

const settings = useSettings();
const mode = ref<HomeMode>(
  settings.get<string>("home.mode", "simple") === "default" ? "default" : "simple",
);
const versions = ref<VersionView[]>([]);
const selectedName = ref("");
const loading = ref(true);
// 只取账户名给欢迎语做称呼；未登录横幅已按产品要求彻底移除。
const { account } = useAccount();

const modeOptions = computed(() => [
  { value: "simple", label: t("module.home.mode.simple") },
  { value: "default", label: t("module.home.mode.default") },
]);

// 模式落盘：下次打开保持用户上次的选择。
watch(mode, (value) => {
  if (settings.get<string>("home.mode", "simple") !== value) {
    void settings.set("home.mode", value);
  }
});

// ---------------------------------------------------------------- 欢迎语轮换
//
// 槽位按**本地时间**对齐：先补上时区偏移再取整，否则 UTC 对齐会让中国用户的
// 换句时刻落在 02:00 / 08:00 / 14:00 / 20:00，与「每 6 小时」的直觉不符。
const nowTick = ref(Date.now());
let welcomeTimer: number | undefined;

const welcomeIndex = computed(() => {
  const offset = -new Date().getTimezoneOffset() * 60 * 1000;
  return Math.floor((nowTick.value + offset) / WELCOME_PERIOD_MS) % WELCOME_COUNT;
});

/** 未登录时用一个中性称呼，避免文案里出现空名字。 */
const playerName = computed(
  () => account.value?.gamertag?.trim() || t("module.home.welcome.friend"),
);

const welcome = computed(() =>
  t(`module.home.welcome.${welcomeIndex.value}`, { name: playerName.value }),
);

/** 当前版本（多版本时可经选择器切换）。 */
const current = computed(() => {
  if (!selectedName.value) return null;
  return versions.value.find((v) => v.name === selectedName.value) ?? null;
});

/** 实例选项：胶囊下拉里要显示封面，所以把 logo 一起带下去。 */
const versionOptions = computed(() =>
  versions.value.map((v) => ({
    value: v.name,
    label: v.name,
    icon: v.logo_data_url,
  })),
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
  welcomeTimer = window.setInterval(() => {
    nowTick.value = Date.now();
  }, WELCOME_TICK_MS);
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
  if (welcomeTimer !== undefined) {
    window.clearInterval(welcomeTimer);
    welcomeTimer = undefined;
  }
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

/** 默认模式：从磁贴直接启动 / 结束指定实例。 */
async function launchNamed(name: string) {
  if (!name || launching.value || stopping.value) return;
  if (selectedName.value !== name) selectedName.value = name;
  beginLaunch(name);
  try {
    await homeLaunch(name);
  } catch (e) {
    endSession();
    showToast(t("module.home.toast.launch_failed", { message: String(e) }), "error");
  }
}

async function stopNamed(name: string) {
  if (!name || stopping.value) return;
  stopping.value = true;
  try {
    await homeGameKill(name);
    showToast(t("module.home.toast.stopped"), "success");
  } catch (e) {
    showToast(t("module.home.toast.stop_failed", { message: String(e) }), "error");
    void syncSessionState(name);
  } finally {
    stopping.value = false;
  }
}

/** 默认模式：按磁贴 / 启动条指定的实例进入设置页。 */
function openVersionSettingsFor(name: string) {
  if (!name) return;
  void router.push({ path: "/version-settings", query: { name } });
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
  <CoPage :scroll="false">
    <div :class="['home-simple', { 'home-simple--default': mode === 'default' }]">
      <!--
        模式切换注入全局标题栏操作区。
        必须 `defer`：内嵌资源（release / custom-protocol）下路由分包已在本地，
        HomePage 会和外壳在同一次渲染里挂载，此时标题栏还没插进文档，
        Teleport 找不到目标 → 整棵 vnode 树损坏（首页永远停在加载态）。
        `defer` 把目标解析推迟到本次渲染结束后，等价于「只晚一个 tick」。
      -->
      <Teleport defer to="#copper-titlebar-actions">
        <CoSegmented v-model="mode" :options="modeOptions" />
      </Teleport>

      <!-- 默认模式：启动区 / 列表区 / 小组件区 -->
      <DefaultHome
        v-if="mode === 'default'"
        :versions="versions"
        :selected-name="selectedName"
        :session="session"
        :launching="launching"
        :running="running"
        :stopping="stopping"
        :welcome="welcome"
        @select="selectedName = $event"
        @launch="launchNamed"
        @stop="stopNamed"
        @detail="openVersionSettingsFor"
      />

      <div v-else class="home-simple__center">
        <template v-if="loading">
          <p class="home-simple__hint">{{ t("common.loading") }}</p>
          <!-- 版本清单扫描期间展示内核随机提示，避免空白等待 -->
          <TipsRotator compact />
        </template>

        <template v-else-if="!current">
          <h1 class="home-simple__welcome">{{ welcome }}</h1>
          <p class="home-simple__empty">{{ t("module.home.empty") }}</p>
          <p class="home-simple__hint">{{ t("module.home.install_hint") }}</p>
          <CoButton variant="primary" size="md" @click="goDownload">
            <Download :size="16" />
            <span>{{ t("module.home.go_download") }}</span>
          </CoButton>
        </template>

        <div v-else class="home-simple__stage">
          <h1 class="home-simple__welcome">{{ welcome }}</h1>

          <!--
            胶囊面板：整页的视觉中心。
            左 = 实例选择（图标 + 名字，点开是圆角矩形浮层，最多同屏 5 项），
            右 = 详情（只显示图标，标题用 tooltip 补）+ 开始游戏。
            注意：这一支走网格舞台（不是 flex 居中）—— 只有面板本身居中，
            欢迎语挂在面板上方，否则整组居中会让面板看起来偏下。
          -->
          <div class="home-simple__dock">
            <CoInstanceSelect
              v-model="selectedName"
              :options="versionOptions"
              class="home-simple__dock-select"
            />

            <div class="home-simple__dock-actions">
              <button
                type="button"
                class="home-simple__detail"
                :title="t('module.home.detail')"
                :aria-label="t('module.home.detail')"
                @click="openVersionSettings"
              >
                <Settings2 :size="19" />
              </button>
              <CoButton
                class="home-simple__play"
                :variant="running ? 'danger' : 'primary'"
                size="md"
                :disabled="launching || stopping"
                @click="onPrimaryClick"
              >
                <CoRingProgress v-if="launching" :size="17" :stroke="2" />
                <Square v-else-if="running" :size="14" />
                <Play v-else :size="17" />
                <span>{{ primaryLabel }}</span>
              </CoButton>
            </div>
          </div>

          <!-- 启动确认期间在面板下方给一条提示：这段时间可达数十秒，不能留白 -->
          <TipsRotator v-if="launching" compact class="home-simple__tips" />
        </div>
      </div>
    </div>
  </CoPage>
</template>

<style scoped>
.home-simple {
  height: 100%;
  min-height: 0;
  /* 竖屏 / 矮窗口下允许整页滚动：欢迎语与操作条不能被裁掉。 */
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  align-items: center;
  padding-top: var(--copper-space-4);
}

/* 加载 / 空态：整组在剩余高度里居中。 */
.home-simple__center {
  flex: 1;
  min-height: 0;
  width: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  /* safe center：内容高于可视区时从顶部开始排，而不是把标题挤出滚动区。 */
  justify-content: safe center;
  gap: var(--copper-space-5);
  padding-bottom: var(--copper-space-6);
}

/*
 * 正常态：网格舞台。
 *
 * 上下两行等分（minmax(min-content, 1fr)），胶囊面板落在中间那一行 ——
 * 于是面板本身严格居中，欢迎语挂在它上方。此前用 flex 居中整个
 * 「欢迎语 + 面板」组，面板会被欢迎语顶得偏下，正是要修的问题。
 * 上下行留 min-content 下限，矮窗口时舞台撑高、由外层滚动，不会裁掉内容。
 */
.home-simple__stage {
  flex: 1;
  min-height: 0;
  width: 100%;
  display: grid;
  grid-template-rows: minmax(min-content, 1fr) auto minmax(min-content, 1fr);
  justify-items: center;
  align-items: center;
  row-gap: var(--copper-space-5);
  padding-bottom: var(--copper-space-4);
}

.home-simple__stage .home-simple__welcome {
  grid-row: 1;
  align-self: end;
}

.home-simple__stage .home-simple__dock {
  grid-row: 2;
}

.home-simple__welcome {
  max-width: 720px;
  padding: 0 var(--copper-space-4);
  /* 识别度：欢迎语是整页唯一的大字，过小会让页面显得空。 */
  font-size: 30px;
  font-weight: 700;
  letter-spacing: 0.01em;
  line-height: 1.28;
  text-align: center;
}

/* 胶囊面板：整页视觉中心。无描边，靠底色阶差 + 阴影浮起。 */
.home-simple__dock {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  width: min(720px, 100%);
  padding: 8px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow);
}

.home-simple__dock-select {
  flex: 1;
  min-width: 0;
}

.home-simple__tips {
  grid-row: 3;
  align-self: start;
}

.home-simple__dock-actions {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
}

/* 详情：只显示图标，文字靠 tooltip / aria-label 补。 */
.home-simple__detail {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 56px;
  height: 56px;
  border: none;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing),
    transform var(--copper-duration-fast) var(--copper-easing);
}

.home-simple__detail:hover {
  background: var(--copper-surface-3);
  color: var(--copper-text);
}

.home-simple__detail:active {
  transform: scale(0.96);
}

.home-simple__play {
  height: 56px;
  padding: 0 var(--copper-space-6);
  border-radius: var(--copper-radius-full);
  font-size: 15px;
  font-weight: 600;
}

.home-simple__empty {
  color: var(--copper-text-secondary);
  text-align: center;
}

.home-simple__hint {
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-sm);
  text-align: center;
}

.home-simple--default {
  align-items: stretch;
  justify-content: flex-start;
  overflow-y: auto;
  padding-bottom: var(--copper-space-5);
}

.home-simple__placeholder {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--copper-text-secondary);
}
</style>
