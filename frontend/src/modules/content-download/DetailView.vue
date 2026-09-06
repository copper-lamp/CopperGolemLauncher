<script setup lang="ts">
// 内容下载详情页：来源徽标 / readme / 版本分类 / 依赖 / 下载（CF 直链 + lip 安装）
// 下载确认时带"飞入下载"抛物线动画反馈。

import { computed, onMounted, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import {
  ArrowLeft,
  ExternalLink,
  ImageIcon,
  Layers,
  Sun,
  Puzzle,
  Download,
  PackageCheck,
  LoaderCircle,
  AlertTriangle,
  FileDigit,
  Link2,
} from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
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

const route = useRoute();
const router = useRouter();
const { t } = useI18n();

const detail = ref<ContentDetail | null>(null);
const readmeHtml = ref<string | null>(null);
const loading = ref(true);
const error = ref<string | null>(null);
const lipEnv = ref<LipEnv | null>(null);

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

function typeLabel(ct: string): string {
  const suffix =
    { behavior_pack: "BehaviorPack", texture_pack: "TexturePack", shader: "Shader" }[
      ct as string
    ] ?? "LlMod";
  return t(`${MB_KEY}.type${suffix}`);
}

function sourceLabel(src: string): string {
  return src === "lip" ? t(`${MB_KEY}.sourceLip`) : t(`${MB_KEY}.sourceCurseforge`);
}

function releaseLabel(rt: string): string {
  const map: Record<string, string> = {
    release: "Release",
    beta: "Beta",
    alpha: "Alpha",
  };
  return t(`${KB}.releaseType.${(map[rt] ?? "Release").toLowerCase()}`);
}

/** 按游戏版本归类文件；无版本（lip）归入"全部"。 */
function groups():
  | { category: string; files: ContentFile[] }[]
  | null {
  const d = detail.value;
  if (!d) return null;
  const categories = d.game_versions.filter(Boolean);
  if (categories.length === 0) {
    return [{ category: t(`${MB_KEY}.typeAll`), files: d.files }];
  }
  return categories.map((gv) => ({
    category: gv,
    files: d.files.filter((f) => f.game_versions.includes(gv)),
  }));
}

function goBack() {
  void router.back();
}

async function load() {
  loading.value = true;
  error.value = null;
  const id = String(route.params.id ?? "");
  try {
    const data = await contentDownloadDetail(id);
    detail.value = data;
    // readme 仅 CurseForge 支持。
    if (data.item.source !== "lip") {
      contentDownloadReadme(id)
        .then((html) => (readmeHtml.value = html))
        .catch(() => (readmeHtml.value = null));
    }
    if (data.item.source === "lip") {
      contentDownloadLipEnv().then((env) => (lipEnv.value = env)).catch(() => null);
    }
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
  }
}

/** 把 n 从下载按钮"飞入"左侧下载入口（抛物线）。 */
function flyToDownloads(fromEl: Element) {
  const start = rectCenter(fromEl);
  const nav = document.querySelector('a[href="/downloads"]');
  const target = nav ? rectCenter(nav) : { x: 40, y: window.innerHeight - 44 };
  const node = document.createElement("div");
  node.className = "copper-fly-sprite";
  node.innerHTML =
    '<svg viewBox="0 0 24 24" width="18" height="18"><path fill="currentColor" d="M12 3v10m0 0 4-4m-4 4-4-4M5 17v2a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-2"/></svg>';
  node.style.transform = `translate3d(${start.x}px, ${start.y}px, 0)`;
  document.body.appendChild(node);

  // 采样二次贝塞尔（控制点向上抬升形成弧线）。
  const ctrl = { x: (start.x + target.x) / 2, y: Math.min(start.y, target.y) - 90 };
  const frames: Keyframe[] = [];
  const N = 32;
  for (let i = 0; i <= N; i++) {
    const a = i / N;
    const x =
      (1 - a) * (1 - a) * start.x + 2 * (1 - a) * a * ctrl.x + a * a * target.x;
    const y =
      (1 - a) * (1 - a) * start.y + 2 * (1 - a) * a * ctrl.y + a * a * target.y;
    frames.push({ transform: `translate3d(${x}px, ${y}px, 0)`, opacity: 1 });
  }
  frames.push({ transform: `translate3d(${target.x}px, ${target.y}px, 0)`, opacity: 0 });
  const anim = node.animate(frames, { duration: 680, easing: "ease-in" });
  anim.onfinish = () => node.remove();
}

function rectCenter(el: Element): { x: number; y: number } {
  const r = el.getBoundingClientRect();
  return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
}

/** CurseForge 文件直链下载。 */
async function downloadFile(file: ContentFile, el?: Element) {
  if (!detail.value) return;
  try {
    await contentDownloadDownload(detail.value.item.id, file.id);
    if (el) flyToDownloads(el);
    showToast(t(`${KB}.downloadStarted`), "success");
  } catch (e) {
    const msg =
      String(e).includes("lip 安装") ? t(`${KB}.lipNotFound`) : String(e);
    showToast(msg.replace(/^Error:\s*/, ""), "error");
  }
}

/** 打开 lip 安装确认弹窗。 */
function openInstall(file: ContentFile) {
  if (!lipEnv.value?.lip_available) {
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
      showToast(t(`${KB}.libInstallFailed`, { message: outcome.stderr || outcome.stdout }), "error");
    } else {
      showToast(t(`${KB}.installStarted`), "success");
    }
    installing.value = false;
  } catch (e) {
    showToast(String(e).replace(/^Error:\s*/, ""), "error");
  } finally {
    installRunning.value = false;
  }
}

onMounted(load);
</script>

<template>
  <div class="cd-detail">
    <button class="cd-detail__back" @click="goBack">
      <ArrowLeft :size="15" />
      {{ t(`${KB}.back`) }}
    </button>

    <template v-if="loading">
      <div class="cd-detail__skeleton">
        <div class="cd-detail__skeleton-hero" />
        <div class="cd-detail__skeleton-line" />
        <div class="cd-detail__skeleton-line cd-detail__skeleton-line--short" />
      </div>
    </template>

    <template v-else-if="error">
      <p class="cd-detail__error">{{ t(`${MB_KEY}.loadError`) }}</p>
    </template>

    <template v-else-if="detail">
      <!-- 头部 -->
      <header class="cd-detail__header">
        <div class="cd-detail__thumb">
          <img v-if="detail.item.icon_url" :src="detail.item.icon_url" :alt="detail.item.name" />
          <component :is="typeIcon(detail.item.content_type)" v-else :size="34" class="cd-detail__thumb-fallback" />
        </div>
        <div class="cd-detail__info">
          <div class="cd-detail__badges">
            <span class="cd-detail__badge cd-detail__badge--source">{{ sourceLabel(detail.item.source) }}</span>
            <span class="cd-detail__badge">{{ typeLabel(detail.item.content_type) }}</span>
          </div>
          <h1 class="cd-detail__name">{{ detail.item.name }}</h1>
          <p class="cd-detail__desc">{{ detail.item.description }}</p>
          <div class="cd-detail__links">
            <span v-if="detail.authors.length" class="cd-detail__author">
              {{ t(`${KB}.author`) }}：{{ detail.authors[0] }}
            </span>
            <a
              v-if="detail.project_url"
              class="cd-detail__link"
              :href="detail.project_url"
              target="_blank"
              rel="noopener noreferrer"
            >
              <ExternalLink :size="13" />
              {{ t(`${KB}.projectUrl`) }}
            </a>
          </div>
        </div>
      </header>

      <!-- 适配版本 -->
      <section v-if="detail.game_versions.length" class="cd-detail__section">
        <h2 class="cd-detail__section-title">{{ t(`${KB}.compatibleVersions`) }}</h2>
        <div class="cd-detail__chips">
          <span v-for="gv in detail.game_versions" :key="gv" class="cd-detail__chip">{{ gv }}</span>
        </div>
      </section>

      <!-- 版本分类（下载/安装列表） -->
      <section class="cd-detail__section">
        <h2 class="cd-detail__section-title">{{ t(`${KB}.files`) }}</h2>
        <template v-if="!groups()?.length">
          <p class="cd-detail__empty">{{ t(`${MB_KEY}.empty`) }}</p>
        </template>
        <div v-for="g in groups() || []" :key="g.category" class="cd-detail__group">
          <div class="cd-detail__group-label">{{ g.category }}</div>
          <ul class="cd-detail__filelist">
            <li v-for="file in g.files" :key="file.id" class="cd-detail__file">
              <FileDigit :size="16" class="cd-detail__file-icon" />
              <div class="cd-detail__file-main">
                <div class="cd-detail__file-row">
                  <span class="cd-detail__file-version">{{ file.version }}</span>
                  <span class="cd-detail__file-version-meta">{{ releaseLabel(file.release_type) }}</span>
                  <span v-if="file.size > 0" class="cd-detail__file-version-meta">
                    {{ formatBytes(file.size) }}
                  </span>
                </div>
                <div v-if="file.dependencies.length" class="cd-detail__deps">
                  <Link2 :size="12" class="cd-detail__deps-icon" />
                  <span
                    v-for="dep in file.dependencies"
                    :key="dep.ref_id"
                    class="cd-detail__dep"
                    :class="`is-${dep.kind}`"
                  >
                    {{ dep.name || dep.ref_id }}
                  </span>
                </div>
              </div>
              <button
                v-if="isLip"
                class="cd-detail__action"
                :disabled="installRunning"
                @click="openInstall(file)"
              >
                <PackageCheck :size="15" />
                {{ t(`${KB}.lipInstall`) }}
              </button>
              <button
                v-else
                class="cd-detail__action cd-detail__action--primary"
                @click="downloadFile(file, $event.currentTarget as unknown as Element)"
              >
                <Download :size="15" />
                {{ t(`${KB}.download`) }}
              </button>
            </li>
          </ul>
        </div>
      </section>

      <!-- readme -->
      <section v-if="!isLip" class="cd-detail__section">
        <h2 class="cd-detail__section-title">{{ t(`${KB}.readme`) }}</h2>
        <div v-if="readmeHtml" class="cd-detail__readme" v-html="readmeHtml" />
        <p v-else class="cd-detail__empty">{{ t(`${KB}.noReadme`) }}</p>
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
/* 飞行精灵为全局 fixed 元素，样式放非 scoped。 */
.copper-fly-sprite {
  position: fixed;
  left: 0;
  top: 0;
  width: 28px;
  height: 28px;
  z-index: 9999;
  pointer-events: none;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--copper-radius-full);
  color: var(--copper-on-accent, #fff);
  background: var(--copper-accent);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.25);
  will-change: transform;
}
</style>

<style scoped>
.cd-detail {
  height: 100%;
  padding: var(--copper-space-5);
  overflow-y: auto;
}

.cd-detail__back {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-1);
  margin-bottom: var(--copper-space-3);
  padding: 4px 10px;
  border: none;
  border-radius: var(--copper-radius-sm);
  background: transparent;
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  transition:
    background-color var(--copper-duration-fast) var(--copper-easing),
    color var(--copper-duration-fast) var(--copper-easing);
}

.cd-detail__back:hover {
  background: var(--copper-hover);
  color: var(--copper-text);
}

.cd-detail__header {
  display: flex;
  gap: var(--copper-space-4);
  margin-bottom: var(--copper-space-4);
}

.cd-detail__thumb {
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

.cd-detail__thumb img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.cd-detail__thumb-fallback {
  color: var(--copper-text-secondary);
}

.cd-detail__info {
  flex: 1;
  min-width: 0;
}

.cd-detail__badges {
  display: flex;
  gap: var(--copper-space-1);
  margin-bottom: var(--copper-space-1);
}

.cd-detail__badge {
  padding: 2px 8px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-2);
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.cd-detail__badge--source {
  background: color-mix(in srgb, var(--copper-accent) 14%, transparent);
  color: var(--copper-accent);
}

.cd-detail__name {
  font-size: var(--copper-font-size-2xl);
  font-weight: 700;
  line-height: 1.25;
}

.cd-detail__desc {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-md);
  margin-top: var(--copper-space-1);
}

.cd-detail__links {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--copper-space-3);
  margin-top: var(--copper-space-2);
}

.cd-detail__author {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.cd-detail__link {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  color: var(--copper-accent);
  font-size: var(--copper-font-size-xs);
  text-decoration: none;
}

.cd-detail__link:hover {
  text-decoration: underline;
}

.cd-detail__section {
  margin-top: var(--copper-space-5);
}

.cd-detail__section-title {
  font-size: var(--copper-font-size-lg);
  font-weight: 600;
  margin-bottom: var(--copper-space-2);
}

.cd-detail__chips {
  display: flex;
  flex-wrap: wrap;
  gap: var(--copper-space-1);
}

.cd-detail__chip {
  padding: 3px 10px;
  border-radius: var(--copper-radius-full);
  background: var(--copper-surface-2);
  color: var(--copper-text);
  font-size: var(--copper-font-size-xs);
}

.cd-detail__empty {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-sm);
}

.cd-detail__group {
  margin-top: var(--copper-space-3);
}

.cd-detail__group-label {
  font-size: var(--copper-font-size-xs);
  color: var(--copper-text-secondary);
  text-transform: uppercase;
  margin-bottom: var(--copper-space-1);
}

.cd-detail__filelist {
  list-style: none;
  display: flex;
  flex-direction: column;
  gap: var(--copper-space-2);
}

.cd-detail__file {
  display: flex;
  align-items: center;
  gap: var(--copper-space-3);
  padding: var(--copper-space-2) var(--copper-space-3);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
}

.cd-detail__file-icon {
  flex-shrink: 0;
  color: var(--copper-text-secondary);
}

.cd-detail__file-main {
  flex: 1;
  min-width: 0;
}

.cd-detail__file-row {
  display: flex;
  align-items: center;
  gap: var(--copper-space-2);
  flex-wrap: wrap;
}

.cd-detail__file-version {
  font-weight: 600;
}

.cd-detail__file-version-meta {
  color: var(--copper-text-secondary);
  font-size: var(--copper-font-size-xs);
}

.cd-detail__deps {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--copper-space-1);
  margin-top: 4px;
}

.cd-detail__deps-icon {
  color: var(--copper-text-secondary);
}

.cd-detail__dep {
  padding: 1px 8px;
  border-radius: var(--copper-radius-full);
  font-size: var(--copper-font-size-xs);
}

.cd-detail__dep.is-required {
  background: color-mix(in srgb, var(--copper-warning) 16%, transparent);
  color: var(--copper-warning);
}

.cd-detail__dep.is-optional {
  background: var(--copper-surface-3);
  color: var(--copper-text-secondary);
}

.cd-detail__action {
  display: inline-flex;
  align-items: center;
  gap: var(--copper-space-1);
  height: var(--copper-control-h-sm);
  padding: 0 var(--copper-space-3);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-md);
  background: var(--copper-surface);
  color: var(--copper-text);
  font-size: var(--copper-font-size-sm);
  cursor: pointer;
  flex-shrink: 0;
  transition: background-color var(--copper-duration-fast) var(--copper-easing);
}

.cd-detail__action:hover:not(:disabled) {
  background: var(--copper-surface-2);
}

.cd-detail__action--primary {
  border-color: var(--copper-accent);
  background: var(--copper-accent);
  color: var(--copper-on-accent, #fff);
}

.cd-detail__action--primary:hover:not(:disabled) {
  opacity: 0.92;
}

.cd-detail__action:disabled {
  opacity: 0.5;
  cursor: default;
}

.cd-detail__readme {
  color: var(--copper-text);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius-lg);
  padding: var(--copper-space-4);
  overflow-x: auto;
  line-height: 1.6;
}

.cd-detail__readme :deep(h1),
.cd-detail__readme :deep(h2),
.cd-detail__readme :deep(h3) {
  margin: 0.6em 0 0.3em;
}

.cd-detail__readme :deep(p) {
  margin: 0.4em 0;
}

.cd-detail__readme :deep(img) {
  max-width: 100%;
}

.cd-detail__readme :deep(code) {
  background: var(--copper-surface-2);
  padding: 1px 5px;
  border-radius: var(--copper-radius-xs);
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
  border-radius: var(--copper-radius-xs);
  background: var(--copper-surface-3);
}

.cd-detail__skeleton-line--short {
  width: 60%;
}

.cd-detail__error {
  color: var(--copper-danger);
  text-align: center;
  padding: var(--copper-space-6);
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
  background: rgba(0, 0, 0, 0.4);
}

.cd-modal__card {
  position: relative;
  width: min(420px, calc(100vw - var(--copper-space-6)));
  padding: var(--copper-space-4);
  border-radius: var(--copper-radius-lg);
  background: var(--copper-surface);
  border: 1px solid var(--copper-border);
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.3);
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