<script setup lang="ts">
// 内容详情页。
//
// 布局（V3 骨架）：主栏 + 右侧「本页」导航。
// - 主栏：一张信息卡（缩略图 / 名称 / 描述 / 作者 / 徽标 / 适配版本 / 说明文档）+ 版本区；
// - 副栏：版本分组锚点，点击展开对应分组并滚动到它；吸顶，长文档滚动时仍可跳转；
// - 版本区：每个游戏版本（大版本）一张可折叠卡片，默认折叠，展开后列出该版本下的文件。
//
// 文件卡片整卡可点击：CF 直接投递下载、lip 走安装确认弹窗；点击后由点击坐标发出圆点，
// 沿抛物线飞向左侧「下载中心」图标，到达时触发水波纹提示。

import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import {
  ExternalLink,
  ImageIcon,
  Layers,
  Sun,
  Puzzle,
  PackageCheck,
  LoaderCircle,
  AlertTriangle,
  FileDigit,
  Link2,
  ChevronRight,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import { useSettings } from "../../composables/useSettings";
import { onContentDownloadLocation, type ContentDownloadLocation } from "../../events";
import { homeVersionsList, type VersionView } from "../../api/home";
import TipsRotator from "../../components/TipsRotator.vue";
import CoPage from "../../components/layout/CoPage.vue";
import CoSplitLayout from "../../components/layout/CoSplitLayout.vue";
import CoBadge from "../../components/ui/CoBadge.vue";
import {
  sourceBadgeLabel,
  sourceBadgeTone,
  typeBadgeLabel,
  typeBadgeTone,
  releaseBadgeLabel,
  releaseBadgeTone,
} from "./badges";
import { detailCrumbTitle } from "./detailCrumb";
import RailToc from "./RailToc.vue";
import { renderReadme, type ReadmeFormat } from "./readme";
import {
  contentDownloadDetail,
  contentDownloadReadme,
  contentDownloadDownload,
  contentDownloadPlan,
  contentDownloadLipEnv,
  contentDownloadLipInstall,
  formatBytes,
  type ContentDetail,
  type ContentFile,
  type LipEnv,
  type LipInstallOutcome,
} from "./api";

const MB_KEY = "module.content-download";
const KB = `${MB_KEY}.detail`;

/** 下载中心的稳定 id（见 components/SideNav.vue）。 */
const DOWNLOAD_NAV_ID = "copper-nav-downloads";

const route = useRoute();
const router = useRouter();
const { t, locale } = useI18n();
const settings = useSettings();

const detail = ref<ContentDetail | null>(null);
const readmeHtml = ref<string | null>(null);
const readmeLoading = ref(false);
const loading = ref(true);
const error = ref<string | null>(null);
const lipEnv = ref<LipEnv | null>(null);

/** 页面骨架实例：正文的滚动容器在 CoPage 上（目录高亮以它为参照）。 */
const pageRef = ref<InstanceType<typeof CoPage> | null>(null);
/** 正文 HTML 的宿主：目录 id 写在渲染后的标题元素上。 */
const docRef = ref<HTMLElement | null>(null);
/** 扁平化后的目录（带层级用于缩进）。 */
const tocFlat = ref<Array<{ id: string; text: string; level: number }>>([]);


/** 传给轨道目录的条目（正文标题 → { id, label, depth }）。 */
const tocItems = computed(() =>
  tocFlat.value.map((item) => ({ id: item.id, label: item.text, depth: item.level })),
);
/** 正文的滚动容器：CoPage 挂载后才能拿到。 */
const scrollEl = computed(() => pageRef.value?.bodyEl ?? null);
/** 导航条目 = 三个固定一级区块（信息 / 介绍 / 版本）+ 介绍下的文档标题 + 版本分组。 */
const railItems = computed(() => [
  { id: "cd-sec-info", label: t(KB + ".sectionInfo"), depth: 0 },
  { id: "cd-sec-intro", label: t(KB + ".sectionIntro"), depth: 0 },
  ...tocItems.value.map((item) => ({ ...item, depth: item.depth + 1 })),
  { id: "cd-sec-versions", label: t(KB + ".sectionVersions"), depth: 0 },
  ...versionTree.value.map((group, index) => ({
    id: "cd-version-" + index,
    label: group.branch,
    depth: 1,
  })),
]);


/**
 * 从渲染后的正文里解析标题层级，顺便给每个标题写 id。
 *
 * 事后读 DOM 而不是提前扫 HTML 字符串：v-html 渲染的就是这份 HTML，
 * 直接读 DOM 拿到的是「实际显示出来」的标题，不会和被过滤掉的内容对不上。
 */
async function buildToc() {
  await nextTick();
  tocFlat.value = [];
  const root = docRef.value;
  if (!root) return;
  // CurseForge 的描述里大量用标题标签排版整段文字，原样收进目录就是一堆段落。
  // 这里只认「像标题的标题」：长度受控、不以句末标点结尾；层级收敛到 h1~h3。
  const headings = Array.from(root.querySelectorAll<HTMLElement>("h1,h2,h3")).filter((el) => {
    const text = (el.textContent ?? "").trim();
    if (!text) return false;
    if (text.length > 48) return false;
    if (/[。！？；.?!;]$/.test(text)) return false;
    return true;
  });
  if (headings.length === 0) return;
  const items: Array<{ id: string; text: string; level: number }> = [];
  headings.forEach((el, index) => {
    const id = "cd-sec-" + index;
    el.id = id;
    el.classList.add("cd-doc__heading");
    items.push({
      id,
      text: (el.textContent ?? "").trim(),
      level: Number(el.tagName.substring(1)) - 1,
    });
  });
  tocFlat.value = items;
}

/** 已展开的版本分组（键为分组名）。默认全部折叠。 */
const expanded = ref<Set<string>>(new Set());

// 安装确认弹窗状态。
const installing = ref(false);
const installFile = ref<ContentFile | null>(null);
const installRunning = ref(false);

/**
 * 桌面 LL 模组（lip）：走 lipd 安装确认弹窗。
 *
 * 安卓 LL 模组（`lla`）**不是** lip：lipd 在安卓不可用，它走目录直链下载
 * + 后端解包落位，因此不进 lip 环境探测分支（见 docs/模块/内容下载/设计.md
 * 安卓章节）。两者都要确认目标版本，所以共用同一个弹窗。
 */
const isLip = computed(() => detail.value?.item.source === "lip");
const isLla = computed(() => detail.value?.item.source === "lla");
/** 需要确认目标版本的来源（lip 安装 / 安卓模组安装）。 */
const needsTargetConfirm = computed(() => isLip.value || isLla.value);

/** lip 安装目标版本（`launch.default_version`，与后端解析口径一致）。 */
const targetVersion = ref("");
const targetFolder = ref("");

async function resolveTargetVersion() {
  const selected = settings.get<string>("launch.default_version", "").trim();
  const versions = await homeVersionsList();
  const current = versions.find((version: VersionView) => version.name === selected);
  targetVersion.value = current?.name ?? "";
  targetFolder.value = current?.folder ?? "";
}
/** 弹窗展示的目标描述：无默认版本时明确提示，避免「默认版本目录」这种含糊说法。 */
const targetLabel = computed(
  () => targetVersion.value || t(`${KB}.noVersionTarget`),
);

/** lip 安装错误码 → 本地化提示键（未知码回退后端原文）。 */
const LIP_ERROR_KEYS: Record<string, string> = {
  ERR_LIP_NOT_INSTALLED: "lipNotFound",
  ERR_LIP_PACKAGE_INVALID_IDENTIFIER: "lipErrInvalidIdentifier",
  ERR_LIP_PACKAGE_VERSION_REQUIRED: "lipErrVersionRequired",
  ERR_TARGET_NOT_FOUND: "lipErrTargetNotFound",
};

/** 把 lip 安装失败结果转成用户可读提示。 */
function lipErrorMessage(outcome: LipInstallOutcome): string {
  const key = LIP_ERROR_KEYS[outcome.errorCode ?? ""];
  if (key) return t(`${KB}.${key}`);
  // 安装类失败：附上 daemon 原文便于排查。
  const detail = (outcome.stderr || outcome.stdout).trim();
  return t(`${KB}.libInstallFailed`, {
    message: detail || outcome.errorCode || "",
  });
}

function typeIcon(ct: string) {
  switch (ct) {
    case "behavior_pack":
      return Puzzle;
    case "texture_pack":
      return Layers;
    case "shader":
      return Sun;
    default:
      return ImageIcon;
  }
}

/** 文件行尾提示与title：lip / 安卓模组为「安装」，其余为「下载」。 */
const pickHint = computed(() =>
  needsTargetConfirm.value ? t(`${KB}.install`) : t(`${KB}.download`),
);

/** 版本号 → 分层键：取前两段（1.21.0.3 → 1.21）；没有数字段时归入「其他」。 */
function versionBranch(version: string): string {
  const parts = version.split(/[^0-9]+/).filter(Boolean);
  if (parts.length >= 2) return parts[0] + "." + parts[1];
  if (parts.length === 1) return parts[0];
  return t(KB + ".otherVersions");
}

/** 大版本在前：1.21 排在 1.9 前面（逐段数值比较，不做字符串比较）。 */
function compareBranch(a: string, b: string): number {
  const pa = a.split(".").map((n) => Number.parseInt(n, 10));
  const pb = b.split(".").map((n) => Number.parseInt(n, 10));
  const len = Math.max(pa.length, pb.length);
  for (let i = 0; i < len; i += 1) {
    const x = Number.isFinite(pa[i]) ? pa[i] : 0;
    const y = Number.isFinite(pb[i]) ? pb[i] : 0;
    if (x !== y) return y - x;
  }
  return 0;
}

/** 版本树：顶层是版本号前两段，叶子是该分支下的全部文件。 */
const versionTree = computed<{ branch: string; files: ContentFile[] }[]>(() => {
  const d = detail.value;
  if (!d) return [];
  const map = new Map<string, ContentFile[]>();
  for (const file of d.files) {
    const key = versionBranch(file.version);
    const list = map.get(key);
    if (list) list.push(file);
    else map.set(key, [file]);
  }
  return [...map.entries()]
    .sort((a, b) => compareBranch(a[0], b[0]))
    .map(([branch, files]) => ({ branch, files }));
});

function isExpanded(key: string): boolean {
  return expanded.value.has(key);
}

function toggleGroup(key: string) {
  const next = new Set(expanded.value);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  expanded.value = next;
}

/** readme 原文格式：CF 为 HTML 片段，lip / 安卓目录（`lla`）为 GitHub Markdown。 */
function readmeFormat(src: string): ReadmeFormat {
  return src === "lip" || src === "lla" ? "markdown" : "html";
}

/** readme 相对资源基准地址（GitHub 仓库优先）。 */
function readmeBaseUrl(d: ContentDetail): string | null {
  return d.repoUrl || d.projectUrl || null;
}

/** 详情请求序号：同一组件实例内切 id 时，丢弃过期响应。 */
let loadSeq = 0;

async function load() {
  const seq = ++loadSeq;
  loading.value = true;
  error.value = null;
  expanded.value = new Set();
  detail.value = null;
  tocFlat.value = [];
  // 先清空面包屑末级：上一个内容的名字绝不能留在新页面上。
  detailCrumbTitle.value = "";
  const id = String(route.params.id ?? "");
  try {
    const data = await contentDownloadDetail(id);
    if (seq !== loadSeq) return;
    detail.value = data;
    // 面包屑末级取当前内容名（响应式引用，见 detailCrumb.ts）。
    detailCrumbTitle.value = data.item.name;
    if (data.item.source === "lip") {
      contentDownloadLipEnv().then((env) => (lipEnv.value = env)).catch(() => null);
    }
    void loadReadme(id, data);
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

/** 拉取并渲染 readme：CF 与 lip 都支持，格式按来源区分。 */
async function loadReadme(id: string, data: ContentDetail) {
  readmeLoading.value = true;
  try {
    const raw = await contentDownloadReadme(id, locale.value);
    readmeHtml.value = raw
      ? renderReadme(raw, readmeFormat(data.item.source), readmeBaseUrl(data))
      : null;
  } catch {
    readmeHtml.value = null;
  } finally {
    readmeLoading.value = false;
  }
  // 必须等 loading 关掉之后再解析：加载态占着正文位置，此时 docRef 还是 null。
  await buildToc();
}

/** 圆点抛物线飞行：从点击坐标飞向下载中心，抵达时触发水波纹。 */
function flyToDownloads(origin?: { x: number; y: number }) {
  const nav = document.getElementById(DOWNLOAD_NAV_ID);
  const target = nav ? rectCenter(nav) : { x: 40, y: window.innerHeight - 44 };
  const start = origin ?? target;

  const node = document.createElement("div");
  node.className = "copper-fly-dot";
  node.style.transform = `translate3d(${start.x}px, ${start.y}px, 0)`;
  document.body.appendChild(node);

  // 采样二次贝塞尔（控制点向上抬升形成弧线）。
  const ctrl = { x: (start.x + target.x) / 2, y: Math.min(start.y, target.y) - 120 };
  const frames: Keyframe[] = [];
  const N = 32;
  for (let i = 0; i <= N; i++) {
    const a = i / N;
    const x = (1 - a) * (1 - a) * start.x + 2 * (1 - a) * a * ctrl.x + a * a * target.x;
    const y = (1 - a) * (1 - a) * start.y + 2 * (1 - a) * a * ctrl.y + a * a * target.y;
    const scale = 1 - 0.35 * a;
    frames.push({
      transform: `translate3d(${x}px, ${y}px, 0) scale(${scale})`,
      opacity: 1,
    });
  }
  frames.push({
    transform: `translate3d(${target.x}px, ${target.y}px, 0) scale(0.6)`,
    opacity: 0.9,
  });

  const anim = node.animate(frames, { duration: 640, easing: "ease-in" });
  anim.onfinish = () => {
    node.remove();
    pulseDownloadTarget(nav);
  };
}

/** 下载中心水波纹提示（class 由 SideNav 定义，动画结束自动移除）。 */
function pulseDownloadTarget(nav: HTMLElement | null) {
  if (!nav) return;
  const cls = "side-nav__item--pulse";
  nav.classList.remove(cls);
  // 强制回流以重启动画。
  void nav.offsetWidth;
  nav.classList.add(cls);
  window.setTimeout(() => nav.classList.remove(cls), 700);
}

function rectCenter(el: Element): { x: number; y: number } {
  const r = el.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
}

/** 点击文件卡片：CF 直链直接下载；lip / 安卓模组走目标版本确认。 */
async function onPickFile(file: ContentFile, event: MouseEvent) {
  if (needsTargetConfirm.value) {
    openInstall(file);
    return;
  }
  const origin = { x: event.clientX, y: event.clientY };
  await downloadFile(file, origin);
}

/** 无 MC 实例时的下载确认弹窗状态。 */
const downloadOnlyConfirm = ref<ContentFile | null>(null);
/** 落点预演结果（确认框要显示落点目录）。 */
const downloadOnlyDir = ref("");

/** CurseForge 文件直链下载。
 *
 * 先调 `plan` 预演落点：无 MC 实例时后端会把落点改到系统下载目录，这种情况下
 * **必须每次都问一次**——内容没进游戏，用户不知道就会一直以为装好了。
 * 确认后把 `plan` 解析出的版本钉住下发，避免两次调用之间用户改了开始页选择。
 */
async function downloadFile(file: ContentFile, origin?: { x: number; y: number }) {
  if (!detail.value) return;
  const itemId = detail.value.item.id;
  try {
    const plan = await contentDownloadPlan(itemId, file.id);
    if (plan.kind === "downloadOnly") {
      downloadOnlyDir.value = plan.dir;
      downloadOnlyConfirm.value = file;
      pendingOrigin.value = origin;
      return;
    }
    // `llpMod` 的目标版本已在弹窗里确认过，钉住它。
    await contentDownloadDownload(itemId, file.id, plan.version);
    flyToDownloads(origin);
    showToast(t(`${KB}.downloadStarted`), "success");
  } catch (e) {
    showToast(String(e).replace(/^Error:\s*/, ""), "error");
  }
}

/** 确认框发起时的点击坐标，供确认后播放抛物线动画。 */
const pendingOrigin = ref<{ x: number; y: number } | undefined>();

/** 用户确认「只下载，不安装」。 */
async function confirmDownloadOnly() {
  const file = downloadOnlyConfirm.value;
  const origin = pendingOrigin.value;
  if (!file || !detail.value) return;
  downloadOnlyConfirm.value = null;
  pendingOrigin.value = undefined;
  try {
    await contentDownloadDownload(detail.value.item.id, file.id);
    flyToDownloads(origin);
    showToast(t(`${KB}.downloadedTo`, { dir: downloadOnlyDir.value }), "info");
  } catch (e) {
    showToast(String(e).replace(/^Error:\s*/, ""), "error");
  }
}

function cancelDownloadOnly() {
  downloadOnlyConfirm.value = null;
  pendingOrigin.value = undefined;
}

/** 落点事件 → 吐司。
 *
 * 订阅放在页面级而非全局：无 MC 实例的提示由确认框承担，这里只负责
 * 「落位成功 / 失败」两类结果提示——那些在用户已经离开详情页时也要看到，
 * 故由下载中心那条链路的调用方订阅更合适；本页订阅则覆盖「装完立刻看到」。
 */
function reportLocation(payload: ContentDownloadLocation) {
  if (payload.error) {
    showToast(t(`${KB}.installFailed`, { message: payload.error }), "error");
    return;
  }
  if (payload.kind === "download_only") {
    showToast(t(`${KB}.downloadedTo`, { dir: payload.dir ?? "" }), "info");
    return;
  }
  if (payload.kind === "installed" && payload.version) {
    showToast(t(`${KB}.installedToVersion`, { version: payload.version }), "success");
  }
}

/** 打开安装确认弹窗（lip 装 BDS 包 / 安卓装 .so 包）。 */
async function openInstall(file: ContentFile) {
  // lip 环境探测只对 lip 生效：安卓模组不经 lipd，无需 lipd 可用。
  if (isLip.value && !lipEnv.value?.lipAvailable) {
    showToast(t(`${KB}.lipNotFound`), "error");
    return;
  }
  try {
    await resolveTargetVersion();
  } catch {
    showToast(t(`${KB}.lipErrTargetNotFound`), "error");
    return;
  }
  if (!targetVersion.value || !targetFolder.value) {
    showToast(t(`${KB}.lipErrTargetNotFound`), "error");
    return;
  }
  installFile.value = file;
  installing.value = true;
}

function publishInstallingRecord() {
  if (!detail.value || !installFile.value) return;
  window.dispatchEvent(
    new CustomEvent("content-download-installing", {
      detail: {
        id: detail.value.item.id,
        source: detail.value.item.source,
        contentType: detail.value.item.contentType,
        name: detail.value.item.name,
        version: installFile.value.version,
        state: "installing",
        dest: targetFolder.value,
        taskId: null,
        error: null,
        updatedAt: Date.now(),
      },
    }),
  );
}

async function confirmInstall() {
  if (!detail.value || !installFile.value) return;
  const file = installFile.value;
  installing.value = false;
  installRunning.value = false;

  if (isLla.value) {
    // 安卓模组：与 CF 同一条投递路径（后端按 `lla:` 路由到解包落位），
    // 只是多了目标版本确认，因此不发布 installing 记录——下载队列事件
    // 会自行写入 downloading → installed / failed。
    await downloadFile(file);
    return;
  }

  publishInstallingRecord();
  installRunning.value = true;
  try {
    // variant 必须独立下发：后端据此拼 `github.com/owner/repo#<variant>@<version>`。
    const outcome = await contentDownloadLipInstall(
      detail.value.item.id,
      file.version,
      file.variant ?? undefined,
      targetFolder.value,
    );
    window.dispatchEvent(new Event("content-download-records-updated"));
    if (!outcome.success) {
      showToast(lipErrorMessage(outcome), "error");
    } else {
      showToast(t(`${KB}.installStarted`), "success");
      flyToDownloads();
    }
  } catch (e) {
    showToast(String(e).replace(/^Error:\s*/, ""), "error");
  } finally {
    installRunning.value = false;
  }
}

/** 离开页面时清掉面包屑中的动态标题，避免影响其它路由复用。 */
function resetCrumbTitle() {
  detailCrumbTitle.value = "";
}

function goList() {
  void router.push("/content");
}

// 同一路由记录内切换 id（详情 → 详情）时组件会被复用，必须自己监听参数变化。
watch(
  () => route.params.id,
  (next, prev) => {
    if (next && next !== prev) void load();
  },
);

onMounted(async () => {
  await load();
  unlistenLocation = await onContentDownloadLocation(reportLocation);
});
onUnmounted(() => {
  unlistenLocation();
  resetCrumbTitle();
});

let unlistenLocation: () => void = () => {};
</script>

<template>
  <CoPage ref="pageRef">
    <div class="cd-detail" :class="{ 'cd-detail--no-toc': !tocFlat.length }">
      <CoSplitLayout aside-position="end">
      <template v-if="loading">
        <div class="cd-detail__skeleton">
          <div class="cd-detail__skeleton-hero" />
          <div class="cd-detail__skeleton-line" />
          <div class="cd-detail__skeleton-line cd-detail__skeleton-line--short" />
        </div>
        <!-- 详情拉取期间展示内核随机提示 -->
        <TipsRotator compact />
      </template>

      <template v-else-if="error">
        <p class="cd-detail__error">{{ t(`${MB_KEY}.loadError`) }}</p>
        <div class="cd-detail__error-actions">
          <button class="cd-btn" @click="goList">{{ t(`${KB}.back`) }}</button>
          <button class="cd-btn cd-btn--primary" @click="load">{{ t(`${MB_KEY}.retry`) }}</button>
        </div>
      </template>

      <template v-else-if="detail">
        <!-- ================= 上部：信息 + 文档（单卡片） ================= -->
        <section id="cd-sec-info" class="cd-info">
          <h2 class="cd-block__title">{{ t(KB + ".sectionInfo") }}</h2>
          <header class="cd-info__head">
            <div class="cd-info__thumb">
              <img v-if="detail.item.iconUrl" :src="detail.item.iconUrl" :alt="detail.item.name" />
              <component
                :is="typeIcon(detail.item.contentType)"
                v-else
                :size="34"
                class="cd-info__thumb-fallback"
              />
            </div>

            <div class="cd-info__main">
              <div class="cd-info__badges">
                <CoBadge
                  :tone="typeBadgeTone(detail.item.contentType)"
                  :label="typeBadgeLabel(detail.item.contentType, detail.item.source)"
                />
                <CoBadge
                  :tone="sourceBadgeTone(detail.item.source)"
                  :label="sourceBadgeLabel(detail.item.source)"
                />
              </div>
              <h1 class="cd-info__name">{{ detail.item.name }}</h1>
              <p class="cd-info__desc">{{ detail.item.description }}</p>
              <div class="cd-info__links">
                <span v-if="detail.authors.length" class="cd-info__author">
                  {{ t(`${KB}.author`) }}：{{ detail.authors[0] }}
                </span>
                <a
                  v-if="detail.projectUrl"
                  class="cd-info__link"
                  :href="detail.projectUrl"
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  <ExternalLink :size="13" />
                  {{ t(`${KB}.projectUrl`) }}
                </a>
              </div>
            </div>
          </header>

          <!-- 适配游戏版本 -->
          <div v-if="detail.gameVersions.length" class="cd-info__block">
            <h2 class="cd-block__title">{{ t(`${KB}.compatibleVersions`) }}</h2>
            <div class="cd-chips">
              <span v-for="gv in detail.gameVersions" :key="gv" class="cd-chip">{{ gv }}</span>
            </div>
          </div>

          <!-- 说明文档 -->
        </section>

        <section id="cd-sec-intro" class="cd-doc-section">
          <h2 class="cd-block__title">{{ t(KB + ".sectionIntro") }}</h2>
            <h2 class="cd-block__title">{{ t(`${KB}.readme`) }}</h2>
            <div v-if="readmeLoading" class="cd-doc__state">
              <LoaderCircle :size="15" class="spin" />
              {{ t(`${KB}.docLoading`) }}
            </div>
            <template v-else-if="readmeHtml">
                <div ref="docRef" class="cd-doc" v-html="readmeHtml" />


            </template>
            <p v-else class="cd-doc__state">{{ t(`${KB}.noReadme`) }}</p>
        </section>

        <!-- ================= 下部：版本（按大版本折叠） ================= -->
        <section id="cd-sec-versions" class="cd-versions">
          <h2 class="cd-versions__title">{{ t(`${KB}.versions`) }}</h2>

          <p v-if="!versionTree.length" class="cd-doc__state">{{ t(`${MB_KEY}.empty`) }}</p>

          <div
            v-for="(g, index) in versionTree"
            :id="'cd-version-' + index"
            :key="g.branch"
            class="cd-version-card"
            :class="{ 'is-open': isExpanded(g.branch) }"
          >
            <button
              class="cd-version-card__head"
              :aria-expanded="isExpanded(g.branch)"
              @click="toggleGroup(g.branch)"
            >
              <ChevronRight
                :size="16"
                class="cd-version-card__caret"
                :class="{ 'is-open': isExpanded(g.branch) }"
              />
              <span class="cd-version-card__label">{{ g.branch }}</span>
              <span class="cd-version-card__count">
                {{ t(`${KB}.versionCount`, { count: g.files.length }) }}
              </span>
            </button>

            <div v-show="isExpanded(g.branch)" class="cd-version-card__body">
              <div
                v-for="file in g.files"
                :key="file.id"
                class="cd-file"
                role="button"
                tabindex="0"
                :title="pickHint"
                @click="onPickFile(file, $event)"
                @keydown.enter="onPickFile(file, $event as unknown as MouseEvent)"
              >
                <FileDigit :size="16" class="cd-file__icon" />
                <div class="cd-file__main">
                  <div class="cd-file__row">
                    <span class="cd-file__version">{{ file.version }}</span>
                    <CoBadge
                      :tone="releaseBadgeTone(file.releaseType)"
                      :label="releaseBadgeLabel(file.releaseType)"
                    />
                    <span v-if="file.size > 0" class="cd-file__meta">
                      {{ formatBytes(file.size) }}
                    </span>
                  </div>
                  <div v-if="file.dependencies.length" class="cd-file__deps">
                    <Link2 :size="12" class="cd-file__deps-icon" />
                    <span
                      v-for="dep in file.dependencies"
                      :key="dep.refId"
                      class="cd-file__dep"
                      :class="`is-${dep.kind}`"
                    >
                      {{ dep.name || dep.refId }}
                    </span>
                  </div>
                </div>
                <span class="cd-file__hint">
                  <PackageCheck v-if="needsTargetConfirm" :size="15" />
                  {{ pickHint }}
                </span>
              </div>
            </div>
          </div>
        </section>
      </template>
      <template #aside>
        <RailToc
          v-if="railItems.length"
          :items="railItems"
          :scroller="scrollEl"
          :title="t(KB + '.onThisPage')"
        />
      </template>
      </CoSplitLayout>
    </div>

    <!-- 无 MC 实例确认弹窗：内容只会落系统下载目录，不会装进游戏 -->
    <div v-if="downloadOnlyConfirm" class="cd-modal">
      <div class="cd-modal__mask" @click="cancelDownloadOnly" />
      <div class="cd-modal__card" role="dialog" aria-modal="true">
        <h3 class="cd-modal__title">{{ t(`${KB}.noInstanceTitle`) }}</h3>
        <p class="cd-modal__text">
          {{ t(`${KB}.noInstanceText`, { dir: downloadOnlyDir }) }}
        </p>
        <p class="cd-modal__hint">
          <AlertTriangle :size="13" />
          {{ t(`${KB}.noInstanceHint`) }}
        </p>
        <div class="cd-modal__actions">
          <button class="cd-modal__btn" @click="cancelDownloadOnly">
            {{ t("common.cancel") }}
          </button>
          <button class="cd-modal__btn cd-modal__btn--primary" @click="confirmDownloadOnly">
            {{ t(`${KB}.continueDownload`) }}
          </button>
        </div>
      </div>
    </div>

    <!-- 安装目标版本确认弹窗（lip 安装 / 安卓模组安装共用） -->
    <div v-if="installing && installFile" class="cd-modal">
      <div class="cd-modal__mask" @click="installing = false" />
      <div class="cd-modal__card" role="dialog" aria-modal="true">
        <h3 class="cd-modal__title">{{ t(`${KB}.confirmInstallTitle`) }}</h3>
        <p class="cd-modal__text">
          {{
            t(`${KB}.confirmInstall`, {
              name: detail?.item.name ?? "",
              dir: targetLabel,
            })
          }}
        </p>
        <p class="cd-modal__hint">
          <AlertTriangle :size="13" />
          {{ isLla ? t(`${KB}.llaInstallHint`) : t(`${KB}.lipInstallHint`) }}
        </p>
        <div class="cd-modal__actions">
          <button class="cd-modal__btn" :disabled="installRunning" @click="installing = false">
            {{ t("common.cancel") }}
          </button>
          <button
            class="cd-modal__btn cd-modal__btn--primary"
            :disabled="installRunning"
            @click="confirmInstall"
          >
            <LoaderCircle v-if="installRunning" :size="15" class="spin" />
            {{ installRunning ? t(`${KB}.installing`) : t(`${KB}.startInstall`) }}
          </button>
        </div>
      </div>
    </div>
  </CoPage>
</template>

<style>
/* 飞行圆点为全局 fixed 元素，样式放非 scoped。 */
.copper-fly-dot {
  position: fixed;
  left: 0;
  top: 0;
  width: 18px;
  height: 18px;
  margin: -9px 0 0 -9px;
  z-index: 9999;
  pointer-events: none;
  border-radius: var(--copper-radius-full);
  background: var(--copper-accent);
  box-shadow:
    0 0 0 4px color-mix(in srgb, var(--copper-accent) 22%, transparent),
    0 4px 12px rgba(0, 0, 0, 0.28);
  will-change: transform;
}
</style>

<style scoped>
/* 副栏：本页版本锚点。卡片语言与主栏一致，吸顶由 CoSplitLayout 负责。 */
.cd-rail {
  padding: var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
}

.cd-rail__title {
  margin-bottom: var(--copper-space-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  font-weight: 600;
}

.cd-rail__nav {
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.cd-rail__item {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  width: 100%;
  padding: 6px var(--copper-space-2);
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  text-align: left;
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.cd-rail__item:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.cd-rail__label {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.cd-rail__count {
  flex-shrink: 0;
  color: var(--copper-text-disabled);
  font-size: var(--copper-font-size-xs);
}

/* 折叠成单列后锚点排在正文末尾，已失去导航意义，直接隐藏。 */
@container (max-width: 760px) {
  .cd-rail {
    display: none;
  }
}

.cd-card {
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  padding: var(--copper-space-4);
}

/* 文档区随窗口自适应内边距，窄窗口收紧、宽窗口放松。 */
.cd-card--info {
  padding: clamp(var(--copper-space-3), 2.4vw, var(--copper-space-5));
}

.cd-info__head {
  display: flex;
  gap: var(--copper-space-4);
}

.cd-info__thumb {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 96px;
  height: 96px;
  flex-shrink: 0;
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface-2);
  overflow: hidden;
}

.cd-info__thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.cd-info__thumb-fallback {
  color: var(--copper-text-secondary);
}

.cd-info__main {
  flex: 1;
  min-width: 0;
}

.cd-info__badges {
  display: flex;
  flex-wrap: wrap;
  gap: var(--copper-space-1);
  margin-bottom: var(--copper-space-1);
}

.cd-info__name {
  font-size: var(--copper-font-size-xl);
  font-weight: 700;
  line-height: 1.25;
}

.cd-info__desc {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-md);
  margin-top: var(--copper-space-1);
}

.cd-info__links {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--copper-space-3);
  margin-top: var(--copper-space-2);
}

.cd-info__author {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.cd-info__link {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--copper-accent);
  font-size: var(--copper-font-size-xs);
  text-decoration: none;
}

.cd-info__link:hover {
  text-decoration: underline;
}

.cd-info__block {
  margin-top: var(--copper-space-4);
}

.cd-doc-section {
  margin-top: var(--copper-space-5);
}

.cd-info__block--doc {
  border-top: 1px solid var(--copper-border);
  padding-top: var(--copper-space-4);
}

.cd-block__title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
  margin-bottom: var(--copper-space-2);
}

.cd-chips {
  display: flex;
  flex-wrap: wrap;
  gap: var(--copper-space-1);
}

.cd-chip {
  padding: 3px 10px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-2);
  color: var(--copper-text);
  font-size: var(--copper-font-size-xs);
}

/* ---- 文档 ---- */
.cd-doc__state {
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.cd-doc-frame {
  position: relative;
  width: min(100%, 74ch);
  max-width: 74ch;
  margin-inline: auto;
  max-height: 8.75em;
  overflow: hidden;
}

.cd-doc-frame:not(.is-expanded)::after {
  position: absolute;
  right: 0;
  bottom: 0;
  left: 0;
  height: 3.5em;
  background: linear-gradient(to bottom, transparent, var(--copper-surface));
  content: "";
  pointer-events: none;
}

.cd-doc-frame.is-expanded {
  max-height: none;
  overflow: visible;
}

.cd-doc {
  width: 100%;
  max-width: 74ch;
  margin-inline: auto;
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  line-height: 1.75;
  overflow-wrap: break-word;
  word-break: break-word;
}

.cd-doc-toggle {
  display: flex;
  margin: var(--copper-space-2) auto 0;
  padding: 4px 10px;
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-sm);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  cursor: pointer;
  font-size: var(--copper-font-size-sm);
}

.cd-doc-toggle:hover {
  background: var(--copper-surface-3);
  color: var(--copper-text);
}

.cd-doc :deep(.cd-readme-align-center) {
  text-align: center;
}

.cd-doc :deep(.cd-readme-align-center > img),
.cd-doc :deep(.cd-readme-align-center img) {
  display: inline-block;
  margin-right: auto;
  margin-left: auto;
}

.cd-doc :deep(.cd-readme-align-center > table),
.cd-doc :deep(.cd-readme-align-center table) {
  margin-right: auto;
  margin-left: auto;
  text-align: left;
}

.cd-doc :deep(h1),
.cd-doc :deep(h2),
.cd-doc :deep(h3),
.cd-doc :deep(h4) {
  margin: 1.3em 0 0.5em;
  line-height: 1.35;
  font-weight: 600;
}

.cd-doc :deep(h1) {
  font-size: var(--copper-font-size-xl);
}

.cd-doc :deep(h2) {
  font-size: var(--copper-font-size-lg);
}

.cd-doc :deep(h3),
.cd-doc :deep(h4) {
  font-size: var(--copper-font-size-md);
}

.cd-doc :deep(> :first-child) {
  margin-top: 0;
}

.cd-doc :deep(p) {
  margin: 0.65em 0;
}

.cd-doc :deep(a) {
  color: var(--copper-accent);
  text-decoration: none;
}

.cd-doc :deep(a:hover) {
  text-decoration: underline;
}

.cd-doc :deep(ul),
.cd-doc :deep(ol) {
  margin: 0.65em 0;
  padding-left: 1.5em;
}

.cd-doc :deep(li) {
  margin: 0.25em 0;
}

.cd-doc :deep(img) {
  max-width: 100%;
  height: auto;
  border-radius: var(--copper-radius-md);
  margin: 0.5em 0;
}

.cd-doc :deep(code) {
  background: var(--copper-surface-2);
  padding: 1px 5px;
  border-radius: var(--copper-radius-sm);
  font-size: 0.92em;
}

.cd-doc :deep(pre) {
  background: var(--copper-surface-2);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  padding: var(--copper-space-3);
  overflow-x: auto;
}

.cd-doc :deep(pre code) {
  background: none;
  padding: 0;
}

.cd-doc :deep(blockquote) {
  margin: 0.8em 0;
  padding: 0.2em 1em;
  border-left: 3px solid var(--copper-border);
  color: var(--copper-text-secondary);
}

/* GitHub 告警块（> [!NOTE] 等）：按类型着色，标题独立成行。 */
.cd-doc :deep(.cd-alert) {
  padding: 0.6em 1em;
  border-left-width: 4px;
  border-radius: var(--copper-radius-sm);
  background: color-mix(in srgb, var(--alert-color) 10%, transparent);
  border-left-color: var(--alert-color);
  color: var(--copper-text-secondary);
}

.cd-doc :deep(.cd-alert__title) {
  margin: 0 0 0.3em;
  font-weight: 600;
  letter-spacing: 0.04em;
  color: var(--alert-color);
}

.cd-doc :deep(.cd-alert--note) {
  --alert-color: var(--copper-accent);
}

.cd-doc :deep(.cd-alert--tip) {
  --alert-color: var(--copper-success, #3fb950);
}

.cd-doc :deep(.cd-alert--important) {
  --alert-color: var(--copper-accent);
}

.cd-doc :deep(.cd-alert--warning) {
  --alert-color: var(--copper-warning);
}

.cd-doc :deep(.cd-alert--caution) {
  --alert-color: var(--copper-danger);
}

.cd-doc :deep(.cd-alert > p:last-child) {
  margin-bottom: 0;
}

/* 宽表格 / 宽内容横向滚动，不撑破卡片。 */
.cd-doc :deep(table) {
  display: block;
  width: max-content;
  max-width: 100%;
  overflow-x: auto;
  border-collapse: collapse;
  margin: 0.8em 0;
}

.cd-doc :deep(th),
.cd-doc :deep(td) {
  border: 1px solid var(--copper-border);
  padding: 4px 10px;
  font-size: var(--copper-font-size-sm);
}

.cd-doc :deep(hr) {
  border: none;
  border-top: 1px solid var(--copper-border);
  margin: 1.2em 0;
}

.cd-doc :deep(details) {
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  padding: var(--copper-space-2) var(--copper-space-3);
  margin: 0.6em 0;
}

.cd-doc :deep(summary) {
  cursor: pointer;
  font-weight: 600;
}

/* ---- 版本区 ---- */
.cd-versions {
  margin-top: var(--copper-space-5);
}

.cd-versions__title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
  margin-bottom: var(--copper-space-3);
}

.cd-version-card {
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  margin-bottom: var(--copper-space-3);
  overflow: hidden;
  /* 锚点跳转落点留出与页面顶边的呼吸位。 */
  scroll-margin-top: var(--copper-space-3);
}

.cd-version-card__head {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  width: 100%;
  padding: var(--copper-space-3) var(--copper-space-4);
  background: transparent;
  border: none;
  color: var(--copper-text);
  text-align: left;
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.cd-version-card__head:hover {
  background: var(--copper-surface-2);
}

.cd-version-card__caret {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
  transition: transform var(--copper-duration-fast) var(--copper-easing);
}

.cd-version-card__caret.is-open {
  transform: rotate(90deg);
}

.cd-version-card__label {
  flex: 1;
  min-width: 0;
  font-weight: 600;
}

.cd-version-card__count {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.cd-version-card__body {
  padding: 0 var(--copper-space-2) var(--copper-space-2);
}

/* 单个版本：前景透明，悬停变灰且无描边。 */
.cd-file {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-2) var(--copper-space-3);
  border: none;
  border-radius: var(--copper-radius-md);
  background: transparent;
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.cd-file:hover,
.cd-file:focus-visible {
  background: var(--copper-surface-2);
  outline: none;
}

.cd-file__icon {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
}

.cd-file__main {
  flex: 1;
  min-width: 0;
}

.cd-file__row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--copper-space-2);
}

.cd-file__version {
  font-weight: 600;
  color: var(--copper-text);
}

.cd-file__meta {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.cd-file__deps {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--copper-space-1);
  margin-top: 4px;
}

.cd-file__deps-icon {
  color: var(--copper-text-secondary);
}

.cd-file__dep {
  padding: 1px 8px;
  border-radius: var(--copper-radius-full);
  font-size: var(--copper-font-size-xs);
}

.cd-file__dep.is-required {
  background: color-mix(in srgb, var(--copper-warning) 16%, transparent);
  color: var(--copper-warning);
}

.cd-file__dep.is-optional {
  background: var(--copper-surface-3);
  color: var(--copper-text-secondary);
}

.cd-file__hint {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  color: var(--copper-accent);
  font-size: var(--copper-font-size-xs);
  opacity: 0;
  transition: opacity var(--copper-duration-fast) var(--copper-easing);
}

.cd-file:hover .cd-file__hint,
.cd-file:focus-visible .cd-file__hint {
  opacity: 1;
}

/* ---- 状态 ---- */
.cd-btn {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-1);
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-4);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.cd-btn:hover {
  background: var(--copper-surface-2);
}

.cd-btn--primary {
  border-color: var(--copper-accent);
  background: var(--copper-accent);
  color: var(--copper-on-accent, #fff);
}

.cd-btn--primary:hover {
  opacity: 0.92;
}

.cd-detail__skeleton-hero {
  width: 96px;
  height: 96px;
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface-3);
}

.cd-detail__skeleton-line {
  height: 14px;
  margin-top: var(--copper-space-3);
  border-radius: var(--copper-radius-sm);
  background: var(--copper-surface-3);
}

.cd-detail__skeleton-line--short {
  width: 60%;
}

.cd-detail__error {
  color: var(--copper-danger);
  text-align: center;
  padding: var(--copper-space-6) var(--copper-space-6) var(--copper-space-2);
}

.cd-detail__error-actions {
  display: flex;
  justify-content: center;
  gap: var(--copper-space-2);
}

/* 弹窗 */
.cd-modal {
  position: fixed;
  inset: 0;
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
}

.cd-modal__mask {
  position: absolute;
  inset: 0;
  background: var(--copper-overlay, rgba(0, 0, 0, 0.4));
}

.cd-modal__card {
  position: relative;
  width: min(420px, calc(100vw - var(--copper-space-6)));
  padding: var(--copper-space-4);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  box-shadow: var(--copper-shadow-lg, 0 16px 48px rgba(0, 0, 0, 0.3));
}

.cd-modal__title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
  margin-bottom: var(--copper-space-2);
}

.cd-modal__text {
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
}

.cd-modal__hint {
  display: flex;
  align-items: center;
  gap: var(--copper-space-1);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
  margin-top: var(--copper-space-2);
}

.cd-modal__hint svg {
  color: var(--copper-warning);
  flex-shrink: 0;
}

.cd-modal__actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--copper-space-2);
  margin-top: var(--copper-space-4);
}

.cd-modal__btn {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-1);
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-4);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.cd-modal__btn:hover:not(:disabled) {
  background: var(--copper-surface-2);
}

.cd-modal__btn--primary {
  border-color: var(--copper-accent);
  background: var(--copper-accent);
  color: var(--copper-on-accent, #fff);
}

.cd-modal__btn--primary:hover:not(:disabled) {
  opacity: 0.92;
}

.cd-modal__btn:disabled {
  opacity: 0.5;
  cursor: default;
}

.spin {
  animation: spin 1.2s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

/* ---------------------------------------------------------------- 钩子导航（右栏目录） */
.cd-toc {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
  padding-top: var(--copper-space-2);
}

.cd-toc__title {
  margin: 0;
  padding-right: 12px;
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  font-weight: 600;
  text-align: right;
}

/* 钩子线：虚线沿右缘走，选中的小节从线上伸出一小段实线勾住文字。 */
.cd-toc__nav {
  display: flex;
  flex-direction: column;
  gap: 1px;
  border-right: 1px dashed var(--copper-border);
}

.cd-toc__item {
  position: relative;
  overflow-wrap: anywhere;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  line-height: 1.45;
  text-align: right;
  padding: 4px 12px;
  cursor: pointer;
  transition:
    color var(--copper-duration-fast) var(--copper-easing),
    background-color var(--copper-duration-fast) var(--copper-easing);
}

.cd-toc__item:hover {
  color: var(--copper-text);
}

.cd-toc__item.is-active {
  color: var(--copper-text);
  font-weight: 600;
}

/* 钩子：选中项在右侧虚线上「勾」一下。 */
.cd-toc__item.is-active::after {
  content: "";
  position: absolute;
  right: -1px;
  top: 50%;
  width: 9px;
  height: 2px;
  transform: translateY(-50%);
  border-radius: 1px;
  background: var(--copper-accent);
}

/* 没有小节标题时收起右栏：目录是可选导航，空着不如让正文占满。 */
.cd-detail--no-toc :deep(.co-split__inner) {
  grid-template-columns: minmax(0, 1fr);
}

.cd-detail--no-toc :deep(.co-split__aside) {
  display: none;
}

/* ---------------------------------------------------------------- 轨道式目录 */
/*
 * 参照 Rare UI 的 rail-toc：左侧一条轨道，章节节点挂在轨道上，层级越深节点越向内缩、
 * 用一段横向短虚线接回轨道；当前章节由一枚纸飞机标记，读过的轨道从虚线变实线。
 */
.rail-toc {
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-3);
  padding-top: var(--copper-space-2);
}

.rail-toc__head {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  font-weight: 600;
}

.rail-toc__nav {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.rail-toc__item {
  position: relative;
  display: flex;
  align-items: center;
  border: none;
  background: transparent;
  color: var(--copper-text-secondary);
  font-family: inherit;
  font-size: var(--copper-font-size-sm);
  line-height: 1.45;
  text-align: left;
  padding: 6px 8px 6px calc(26px + var(--depth, 0) * 16px);
  cursor: pointer;
  transition: color var(--copper-duration-fast) var(--copper-easing);
}

.rail-toc__item:hover {
  color: var(--copper-text);
}

.rail-toc__item.is-active {
  color: var(--copper-text);
  font-weight: 600;
}

/* 轨道：每行一段竖线，横坐标随层级内缩。 */
.rail-toc__rail {
  position: absolute;
  top: 0;
  bottom: 0;
  left: calc(10px + var(--depth, 0) * 16px);
  border-left: 1px dashed var(--copper-border);
}

/* 读过的部分点亮成实线（纸飞机飞过的地方）。 */
.rail-toc__item.is-passed .rail-toc__rail {
  border-left-style: solid;
  border-left-color: color-mix(in srgb, var(--copper-text) 45%, transparent);
}

/* 层级折角：内缩那一层用一段横虚线接回轨道。 */
.rail-toc__item::before {
  content: "";
  position: absolute;
  top: 50%;
  left: 10px;
  width: calc(var(--depth, 0) * 16px);
  border-top: 1px dashed var(--copper-border);
}

.rail-toc__item.is-passed::before {
  border-top-style: solid;
  border-top-color: color-mix(in srgb, var(--copper-text) 45%, transparent);
}

/* 节点：骑在轨道上的小圆点。 */
.rail-toc__node {
  position: absolute;
  top: 50%;
  left: calc(6.5px + var(--depth, 0) * 16px);
  width: 7px;
  height: 7px;
  margin-top: -3.5px;
  border: 1px solid var(--copper-border);
  border-radius: 50%;
  background: var(--copper-bg);
}

.rail-toc__item.is-passed .rail-toc__node {
  border-color: color-mix(in srgb, var(--copper-text) 45%, transparent);
}

/* 纸飞机：当前章节的游标，停在这一行的轨道上。 */
.rail-toc__plane {
  position: absolute;
  left: calc(2px + var(--depth, 0) * 16px);
  top: 50%;
  transform: translateY(-50%) rotate(-45deg);
  color: var(--copper-accent);
  animation: rail-plane-in 240ms var(--copper-easing);
}

@keyframes rail-plane-in {
  from {
    opacity: 0;
    transform: translateY(-50%) rotate(-45deg) scale(0.7);
  }
}

.rail-toc__text {
  overflow-wrap: anywhere;
}


/* ---------------------------------------------------------------- 介绍区（阅读栏，无卡片） */
.cd-doc-section {
  margin-top: var(--copper-space-6);
}

.cd-doc {
  max-width: 78ch;
  color: var(--copper-text);
  font-size: 14px;
  line-height: 1.75;
}

.cd-doc :where(p, ul, ol, blockquote, table, pre, figure) {
  margin: 0 0 1em;
}

.cd-doc :where(h1, h2, h3, h4) {
  margin: 1.8em 0 0.6em;
  line-height: 1.3;
  font-weight: 600;
  /* 目录跳转按 96px 到达线对齐，标题自身也要留出这段距离。 */
  scroll-margin-top: 96px;
}

.cd-doc h1 { font-size: 24px; }
.cd-doc h2 { font-size: 20px; }
.cd-doc h3 { font-size: 16px; }
.cd-doc h4 { font-size: 14px; }

.cd-doc a {
  color: var(--copper-accent);
  text-decoration: underline;
  text-underline-offset: 2px;
}

.cd-doc img {
  display: block;
  max-width: 100%;
  height: auto;
  margin: 1em auto;
  border-radius: var(--copper-radius-md);
}

.cd-doc iframe,
.cd-doc video {
  display: block;
  width: 100%;
  max-width: 100%;
  aspect-ratio: 16 / 9;
  border: none;
  border-radius: var(--copper-radius-md);
}

.cd-doc table {
  display: block;
  max-width: 100%;
  overflow-x: auto;
  border-collapse: collapse;
  font-size: var(--copper-font-size-sm);
}

.cd-doc :where(th, td) {
  padding: 6px 10px;
  border-bottom: 1px solid var(--copper-border);
  text-align: left;
}

.cd-doc blockquote {
  padding-left: var(--copper-space-3);
  border-left: 2px solid var(--copper-accent);
  color: var(--copper-text-secondary);
}

.cd-doc pre {
  padding: var(--copper-space-3);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface-2);
  overflow-x: auto;
}

.cd-doc code {
  padding: 1px 4px;
  border-radius: 4px;
  background: var(--copper-surface-2);
  font-size: 0.92em;
}

.cd-doc pre code {
  padding: 0;
  background: transparent;
}

/* CurseForge 的富文本带内联字号 / 颜色，会把主题撑坏：只保留布局，表现交给令牌。 */
.cd-doc [style] {
  color: inherit !important;
  font-family: inherit !important;
  font-size: inherit !important;
  background-color: transparent !important;
}

/* ---------------------------------------------------------------- 信息区（平摊 + 大黑体） */
.cd-info {
  margin-top: var(--copper-space-2);
}

.cd-info__head {
  display: flex;
  align-items: center;
  gap: var(--copper-space-5);
}

.cd-info__thumb {
  width: 96px;
  height: 96px;
  flex-shrink: 0;
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface-2);
  overflow: hidden;
}

.cd-info__thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.cd-info__badges {
  display: flex;
  gap: 6px;
}

/* 大黑体：模组名是整页的主标题。 */
.cd-info__name {
  margin: 8px 0 0;
  font-size: 38px;
  font-weight: 800;
  line-height: 1.14;
  letter-spacing: -0.01em;
  overflow-wrap: anywhere;
}

.cd-info__desc {
  margin-top: 10px;
  max-width: 78ch;
  color: var(--copper-text-secondary);
  font-size: 15px;
  line-height: 1.6;
}

.cd-info__links {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--copper-space-3);
  margin-top: 10px;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.cd-info__block {
  margin-top: var(--copper-space-5);
}
</style>
