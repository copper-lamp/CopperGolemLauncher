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
import CoSelect from "../../components/ui/CoSelect.vue";
import CoPage from "../../components/layout/CoPage.vue";
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
const {
  account,
  loginWaiting,
  beginWamSignIn,
  profileMismatch,
  clearProfileMismatch,
} = useAccount();
const showAccountBanner = computed(() => !account.value || !!profileMismatch.value);
const accountBannerText = computed(() =>
  profileMismatch.value
    ? t("account.profile_mismatch_title")
    : t("account.not_logged_in"),
);
async function handleAccountBanner() {
  await beginWamSignIn();
  clearProfileMismatch();
}

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
    <div class="home-simple">
      <div v-if="showAccountBanner" class="home-simple__account">
        <span>{{ accountBannerText }}</span>
        <CoButton
          variant="primary"
          size="sm"
          :disabled="loginWaiting"
          @click="handleAccountBanner"
        >
          {{ t("account.store_sign_in") }}
        </CoButton>
      </div>

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

      <!-- 默认模式占位（后续阶段实现） -->
      <div v-if="mode === 'default'" class="home-simple__placeholder">
        <p>{{ t("module.home.mode.default_placeholder") }}</p>
      </div>

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

        <template v-else>
          <h1 class="home-simple__welcome">{{ welcome }}</h1>

          <!-- 胶囊操作条：左版本选择，右详情 + 启动 -->
          <div class="home-simple__bar">
            <div class="home-simple__version">
              <CoSelect
                v-if="versionOptions.length > 1"
                v-model="selectedName"
                :options="versionOptions"
                class="home-simple__version-select"
              />
              <span v-else class="home-simple__version-name" :title="current.name">
                {{ current.name }}
              </span>
            </div>

            <div class="home-simple__actions">
              <button
                type="button"
                class="home-simple__detail"
                @click="openVersionSettings"
              >
                <Settings2 :size="15" />
                <span>{{ t("module.home.detail") }}</span>
              </button>
              <CoButton
                class="home-simple__play"
                :variant="running ? 'danger' : 'primary'"
                size="md"
                :disabled="launching || stopping"
                @click="onPrimaryClick"
              >
                <CoRingProgress v-if="launching" :size="15" :stroke="2" />
                <Square v-else-if="running" :size="13" />
                <Play v-else :size="15" />
                <span>{{ primaryLabel }}</span>
              </CoButton>
            </div>
          </div>

          <!-- 启动确认期间在操作条下方给一条提示：这段时间可达数十秒，不能留白 -->
          <TipsRotator v-if="launching" compact />
        </template>
      </div>
    </div>
  </CoPage>
</template>

<style scoped>
.home-simple {
  height: 100%;
  min-height: 0;
  display: flex;
  flex-direction: column;
  align-items: center;
  padding-top: var(--copper-space-4);
}

.home-simple__account {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  max-width: 640px;
  margin-bottom: var(--copper-space-4);
  padding: var(--copper-space-2) var(--copper-space-3) var(--copper-space-2)
    var(--copper-space-4);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

/* 欢迎语与操作条整体在剩余高度里居中。 */
.home-simple__center {
  flex: 1;
  min-height: 0;
  width: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: var(--copper-space-5);
  padding-bottom: var(--copper-space-6);
}

.home-simple__welcome {
  max-width: 640px;
  padding: 0 var(--copper-space-4);
  font-size: 24px;
  font-weight: 700;
  letter-spacing: 0.02em;
  line-height: 1.3;
  text-align: center;
}

/* 胶囊操作条：版本选择在左，动作在右；整条宽度有限，避免拉满窗口。 */
.home-simple__bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--copper-space-3);
  width: min(560px, 100%);
  padding: 6px;
  padding-left: var(--copper-space-4);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface);
  box-shadow: var(--copper-shadow-subtle);
}

.home-simple__version {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
}

.home-simple__version-name {
  overflow: hidden;
  color: var(--copper-text);
  font-size: var(--copper-font-size-lg);
  font-weight: 500;
  white-space: nowrap;
  text-overflow: ellipsis;
}

/* 版本选择也做成胶囊：去掉原生 select 的边框与底色，只留文字与箭头。 */
.home-simple__version-select :deep(.co-select select) {
  height: 40px;
  padding-right: var(--copper-space-5);
  border-color: transparent;
  border-radius: var(--copper-radius-full);
  background: transparent;
  color: var(--copper-text);
  font-size: var(--copper-font-size-lg);
  font-weight: 500;
}

.home-simple__version-select :deep(.co-select select:hover) {
  background: var(--copper-hover);
}

.home-simple__actions {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
}

.home-simple__detail {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 40px;
  padding: 0 var(--copper-space-3);
  border: none;
  border-radius: var(--copper-radius-full);
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.home-simple__detail:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.home-simple__play {
  height: 40px;
  padding: 0 var(--copper-space-5);
  border-radius: var(--copper-radius-full);
  font-size: var(--copper-font-size-md);
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

.home-simple__placeholder {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--copper-text-secondary);
}
</style>
