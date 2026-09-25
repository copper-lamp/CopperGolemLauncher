<script setup lang="ts">
// 内容详情页。
//
// 布局分上下两部分：
// - 上部：一张卡片，容纳内容信息（缩略图 / 名称 / 描述 / 作者 / 徽标 / 适配版本）与说明文档；
// - 下部：版本区，每个游戏版本（大版本）一张可折叠卡片，默认折叠，展开后列出该版本下的文件。
//
// 文件卡片整卡可点击：CF 直接投递下载、lip 走安装确认弹窗；点击后由点击坐标发出圆点，
// 沿抛物线飞向左侧「下载中心」图标，到达时触发水波纹提示。

import { computed, onMounted, onUnmounted, ref } from "vue";
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
import TipsRotator from "../../components/TipsRotator.vue";
import ContentBadge from "./ContentBadge.vue";
import {
  sourceBadgeLabel,
  sourceBadgeTone,
  typeBadgeLabel,
  typeBadgeTone,
  releaseBadgeLabel,
  releaseBadgeTone,
} from "./badges";
import { renderReadme, type ReadmeFormat } from "./readme";
import {
  contentDownloadDetail,
  contentDownloadReadme,
  contentDownloadDownload,
  contentDownloadLipEnv,
  contentDownloadLipInstall,
  formatBytes,
  type ContentDetail,
  type ContentFile,
  type LipEnv,
} from "./api";

const MB_KEY = "module.content-download";
const KB = `${MB_KEY}.detail`;

/** 下载中心的稳定 id（见 components/SideNav.vue）。 */
const DOWNLOAD_NAV_ID = "copper-nav-downloads";

const route = useRoute();
const router = useRouter();
const { t, locale } = useI18n();

const detail = ref<ContentDetail | null>(null);
const readmeHtml = ref<string | null>(null);
const readmeLoading = ref(false);
const loading = ref(true);
const error = ref<string | null>(null);
const lipEnv = ref<LipEnv | null>(null);

/** 已展开的版本分组（键为分组名）。默认全部折叠。 */
const expanded = ref<Set<string>>(new Set());

// 安装确认弹窗状态。
const installing = ref(false);
const installFile = ref<ContentFile | null>(null);
const installRunning = ref(false);

const isLip = computed(() => detail.value?.item.source === "lip");

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

/** 按游戏版本归类文件；无版本（lip）归入「全部版本」。 */
function groups(): { category: string; files: ContentFile[] }[] {
  const d = detail.value;
  if (!d) return [];
  const categories = d.gameVersions.filter(Boolean);
  if (categories.length === 0) {
    return [{ category: t(`${MB_KEY}.versionAll`), files: d.files }];
  }
  return categories
    .map((gv) => ({
      category: gv,
      files: d.files.filter((f) => f.gameVersions.includes(gv)),
    }))
    .filter((g) => g.files.length > 0);
}

function isExpanded(key: string): boolean {
  return expanded.value.has(key);
}

function toggleGroup(key: string) {
  const next = new Set(expanded.value);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  expanded.value = next;
}

/** readme 原文格式：CF 为 HTML 片段，lip 为 GitHub Markdown。 */
function readmeFormat(src: string): ReadmeFormat {
  return src === "lip" ? "markdown" : "html";
}

/** readme 相对资源基准地址（GitHub 仓库优先）。 */
function readmeBaseUrl(d: ContentDetail): string | null {
  return d.repoUrl || d.projectUrl || null;
}

async function load() {
  loading.value = true;
  error.value = null;
  expanded.value = new Set();
  const id = String(route.params.id ?? "");
  try {
    const data = await contentDownloadDetail(id);
    detail.value = data;
    // 面包屑末级写入具体内容名（TitleBar 优先取字面量 title）。
    const crumbs = route.meta.breadcrumb;
    if (Array.isArray(crumbs) && crumbs.length > 0) {
      const last = crumbs[crumbs.length - 1];
      if (last && typeof last === "object") {
        (last as { title?: string }).title = data.item.name;
      }
    }
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

/** 点击文件卡片：CF 直接下载；lip 走安装确认。 */
async function onPickFile(file: ContentFile, event: MouseEvent) {
  if (isLip.value) {
    openInstall(file);
    return;
  }
  const origin = { x: event.clientX, y: event.clientY };
  await downloadFile(file, origin);
}

/** CurseForge 文件直链下载。 */
async function downloadFile(file: ContentFile, origin?: { x: number; y: number }) {
  if (!detail.value) return;
  try {
    await contentDownloadDownload(detail.value.item.id, file.id);
    flyToDownloads(origin);
    showToast(t(`${KB}.downloadStarted`), "success");
  } catch (e) {
    const msg = String(e).includes("lip 安装") ? t(`${KB}.lipNotFound`) : String(e);
    showToast(msg.replace(/^Error:\s*/, ""), "error");
  }
}

/** 打开 lip 安装确认弹窗。 */
function openInstall(file: ContentFile) {
  if (!lipEnv.value?.lipAvailable) {
    showToast(t(`${KB}.lipNotFound`), "error");
    return;
  }
  installFile.value = file;
  installing.value = true;
}

async function confirmInstall() {
  if (!detail.value || !installFile.value) return;
  installRunning.value = true;
  try {
    const outcome = await contentDownloadLipInstall(
      detail.value.item.id,
      installFile.value.version,
    );
    if (!outcome.success) {
      showToast(
        t(`${KB}.libInstallFailed`, { message: outcome.stderr || outcome.stdout }),
        "error",
      );
    } else {
      showToast(t(`${KB}.installStarted`), "success");
      flyToDownloads();
    }
    installing.value = false;
  } catch (e) {
    showToast(String(e).replace(/^Error:\s*/, ""), "error");
  } finally {
    installRunning.value = false;
  }
}

/** 离开页面时清掉面包屑中的动态标题，避免影响其它路由复用。 */
function resetCrumbTitle() {
  const crumbs = route.meta.breadcrumb;
  if (!Array.isArray(crumbs) || crumbs.length === 0) return;
  const last = crumbs[crumbs.length - 1];
  if (last && typeof last === "object") delete (last as { title?: string }).title;
}

function goList() {
  void router.push("/content");
}

onMounted(load);
onUnmounted(resetCrumbTitle);
</script>

<template>
  <div class="cd-detail">
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
      <section class="cd-card cd-card--info">
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
              <ContentBadge
                :tone="typeBadgeTone(detail.item.contentType)"
                :label="typeBadgeLabel(detail.item.contentType)"
              />
              <ContentBadge
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
        <div class="cd-info__block cd-info__block--doc">
          <h2 class="cd-block__title">{{ t(`${KB}.readme`) }}</h2>
          <div v-if="readmeLoading" class="cd-doc__state">
            <LoaderCircle :size="15" class="spin" />
            {{ t(`${KB}.docLoading`) }}
          </div>
          <div v-else-if="readmeHtml" class="cd-doc" v-html="readmeHtml" />
          <p v-else class="cd-doc__state">{{ t(`${KB}.noReadme`) }}</p>
        </div>
      </section>

      <!-- ================= 下部：版本（按大版本折叠） ================= -->
      <section class="cd-versions">
        <h2 class="cd-versions__title">{{ t(`${KB}.versions`) }}</h2>

        <p v-if="!groups().length" class="cd-doc__state">{{ t(`${MB_KEY}.empty`) }}</p>

        <div
          v-for="g in groups()"
          :key="g.category"
          class="cd-version-card"
          :class="{ 'is-open': isExpanded(g.category) }"
        >
          <button
            class="cd-version-card__head"
            :aria-expanded="isExpanded(g.category)"
            @click="toggleGroup(g.category)"
          >
            <ChevronRight
              :size="16"
              class="cd-version-card__caret"
              :class="{ 'is-open': isExpanded(g.category) }"
            />
            <span class="cd-version-card__label">{{ g.category }}</span>
            <span class="cd-version-card__count">
              {{ t(`${KB}.versionCount`, { count: g.files.length }) }}
            </span>
          </button>

          <div v-show="isExpanded(g.category)" class="cd-version-card__body">
            <div
              v-for="file in g.files"
              :key="file.id"
              class="cd-file"
              role="button"
              tabindex="0"
              :title="isLip ? t(`${KB}.lipInstall`) : t(`${KB}.download`)"
              @click="onPickFile(file, $event)"
              @keydown.enter="onPickFile(file, $event as unknown as MouseEvent)"
            >
              <FileDigit :size="16" class="cd-file__icon" />
              <div class="cd-file__main">
                <div class="cd-file__row">
                  <span class="cd-file__version">{{ file.version }}</span>
                  <ContentBadge
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
                <PackageCheck v-if="isLip" :size="15" />
                {{ isLip ? t(`${KB}.lipInstall`) : t(`${KB}.download`) }}
              </span>
            </div>
          </div>
        </div>
      </section>
    </template>

    <!-- lip 安装确认弹窗 -->
    <div v-if="installing && installFile" class="cd-modal">
      <div class="cd-modal__mask" @click="installing = false" />
      <div class="cd-modal__card" role="dialog" aria-modal="true">
        <h3 class="cd-modal__title">{{ t(`${KB}.confirmInstallTitle`) }}</h3>
        <p class="cd-modal__text">
          {{
            t(`${KB}.confirmInstall`, {
              name: detail?.item.name ?? "",
              dir: t(`${KB}.defaultDir`),
            })
          }}
        </p>
        <p class="cd-modal__hint">
          <AlertTriangle :size="13" />
          {{ t(`${KB}.lipInstallHint`) }}
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
  </div>
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
.cd-detail {
  height: 100%;
  padding: var(--copper-space-5);
  overflow-y: auto;
}

/* 内容阅读宽度上限：避免超宽窗口下文档行宽过大影响可读性。 */
.cd-detail > * {
  max-width: 1080px;
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

.cd-doc {
  /* 阅读型排版：限制行宽并使用更松的行高，保证长文档可读性。 */
  max-width: 74ch;
  color: var(--copper-text);
  font-size: var(--copper-font-size-md);
  line-height: 1.75;
  overflow-wrap: break-word;
  word-break: break-word;
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
</style>
