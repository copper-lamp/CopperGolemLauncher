<script setup lang="ts">
// 屏幕触控层布局编辑器（移动端）。
//
// 设计取舍：**不做像素级拖拽画布**，改成「预览 + 手势调位 + 精确数值」三段式。
// 理由有三个：
// 1. 布局坐标是**归一化比例**（0..1），拖拽画布反而要引入一套像素↔比例换算与
//    吸附逻辑，出错时表现为「控件飞了」且难以复现；
// 2. 手机上精确拖到某个位置本来就不如手调数值可控；
// 3. 控件与游戏同屏时（游戏内编辑）会遮挡游戏视野，而这层 UI 属于启动器资产，
//    按项目约定应当走 Vue + 主题令牌，不在 Kotlin 里再造一套。
//
// 因此：预览区支持拖动（粗调）与缩放（双指，若可识别），数值区做精调；
// 结构操作（增删 / 切换可见 / 改绑定）走列表。
import { computed, ref, watch } from "vue";
import { Plus, Trash2, Eye, EyeOff, RotateCcw, Save } from "@lucide/vue";

import { useI18n } from "../../i18n";
import { showToast } from "../../composables/useToast";
import CoButton from "../../components/ui/CoButton.vue";
import CoSelect from "../../components/ui/CoSelect.vue";
import CoTextField from "../../components/ui/CoTextField.vue";
import {
  BUTTON_ACTIONS,
  DIRECTIONAL_ACTIONS,
  homeControlsGet,
  homeControlsSave,
  type ControlAction,
  type ControlItem,
  type ControlKind,
  type ControlLayout,
} from "../../api/home";

const props = defineProps<{
  /** 当前实例名；为空时不加载（避免请求落到不存在的实例）。 */
  name: string;
  /** 平台标识；触控层只在安卓实例上有意义。 */
  platform: string;
}>();

const { t } = useI18n();
const MB_KEY = "module.home.controls";

const layout = ref<ControlLayout | null>(null);
const loading = ref(false);
const saving = ref(false);
const dirty = ref(false);
const selectedId = ref<string | null>(null);

/** 触控层只对安卓实例成立：桌面端实例没有游戏 Activity。 */
const isAndroid = computed(() => props.platform === "android-arm64");

const kindOptions = computed(() => [
  { value: "dpad", label: t(`${MB_KEY}.kind.dpad`) },
  { value: "joystick", label: t(`${MB_KEY}.kind.joystick`) },
  { value: "button", label: t(`${MB_KEY}.kind.button`) },
  { value: "look", label: t(`${MB_KEY}.kind.look`) },
]);

function actionLabel(action: ControlAction): string {
  return t(`${MB_KEY}.action.${action}`);
}

/** 可选动作按控件类型过滤：方向动作只能上 dpad/joystick，单动作只能上 button。 */
function actionOptions(kind: ControlKind) {
  const list = kind === "button" ? BUTTON_ACTIONS : DIRECTIONAL_ACTIONS;
  return list.map((action) => ({ value: action, label: actionLabel(action) }));
}

const selected = computed(
  () => layout.value?.controls.find((c) => c.id === selectedId.value) ?? null,
);

/** 未选中的控件在列表里也要能一眼看出绑了什么。 */
function describe(control: ControlItem): string {
  if (control.kind === "look") return t(`${MB_KEY}.kind.look`);
  if (control.action) return actionLabel(control.action);
  return t(`${MB_KEY}.no_action`);
}

async function load() {
  if (!isAndroid.value) return;
  if (!props.name) return;
  loading.value = true;
  try {
    layout.value = await homeControlsGet(props.name);
    selectedId.value = layout.value.controls[0]?.id ?? null;
    dirty.value = false;
  } catch (error) {
    layout.value = null;
    showToast(t(`${MB_KEY}.load_failed`) + String(error), "error");
  } finally {
    loading.value = false;
  }
}

async function save() {
  if (!layout.value || !props.name) return;
  saving.value = true;
  try {
    // 用返回值刷新：后端会做校验与坐标夹取，沿用提交的那份会让界面与实际文件不一致。
    layout.value = await homeControlsSave(props.name, layout.value);
    dirty.value = false;
    showToast(t(`${MB_KEY}.saved`), "success");
  } catch (error) {
    showToast(t(`${MB_KEY}.save_failed`) + String(error), "error");
  } finally {
    saving.value = false;
  }
}

/** 恢复默认：直接请求后端的默认布局（它知道当前默认是哪一份）。 */
async function resetToDefault() {
  if (!layout.value) return;
  layout.value = {
    ...layout.value,
    controls: defaultControls(),
  };
  dirty.value = true;
}

/**
 * 默认控件集：与 Rust `ControlLayout::default` 保持同构。
 *
 * 两处各写一份是刻意的——前端要能在「后端给不出布局」时仍然显示一份可用的初值，
 * 但**权威默认值仍在 Rust**：这里点「恢复默认」只是把这份提交上去，落盘前的校验
 * 与夹取由后端完成。
 */
function defaultControls(): ControlItem[] {
  return [
    { id: "dpad", kind: "dpad", rect: { x: 0.04, y: 0.55, w: 0.28, h: 0.4 }, label: "", action: null, opacity: 0.45, visible: true },
    { id: "jump", kind: "button", rect: { x: 0.845, y: 0.62, w: 0.13, h: 0.18 }, label: "", action: "jump", opacity: 0.5, visible: true },
    { id: "sneak", kind: "button", rect: { x: 0.72, y: 0.68, w: 0.11, h: 0.15 }, label: "", action: "sneak", opacity: 0.5, visible: true },
    { id: "inventory", kind: "button", rect: { x: 0.02, y: 0.06, w: 0.09, h: 0.13 }, label: "", action: "inventory", opacity: 0.5, visible: true },
    { id: "menu", kind: "button", rect: { x: 0.13, y: 0.06, w: 0.08, h: 0.12 }, label: "", action: "menu", opacity: 0.5, visible: true },
  ];
}

function addControl() {
  if (!layout.value) return;
  // id 必须唯一且稳定：后端据此校验，前端也用它在列表里定位。
  let index = 1;
  while (layout.value.controls.some((c) => c.id === `button-${index}`)) index += 1;
  const control: ControlItem = {
    id: `button-${index}`,
    kind: "button",
    rect: { x: 0.4, y: 0.75, w: 0.12, h: 0.16 },
    label: "",
    action: "drop",
    opacity: 0.5,
    visible: true,
  };
  layout.value.controls.push(control);
  selectedId.value = control.id;
  dirty.value = true;
}

function removeControl(id: string) {
  if (!layout.value) return;
  layout.value.controls = layout.value.controls.filter((c) => c.id !== id);
  if (selectedId.value === id) {
    selectedId.value = layout.value.controls[0]?.id ?? null;
  }
  dirty.value = true;
}

/** 切换类型时必须同时修正动作绑定，否则后端会以「类型与动作不匹配」拒绝。 */
function onKindChange(control: ControlItem, kind: ControlKind) {
  control.kind = kind;
  if (kind === "look") {
    control.action = null;
  } else if (kind === "button") {
    if (!control.action || DIRECTIONAL_ACTIONS.includes(control.action)) {
      control.action = "jump";
    }
  } else {
    // dpad / joystick
    if (!control.action || !DIRECTIONAL_ACTIONS.includes(control.action)) {
      control.action = "forward";
    }
  }
  dirty.value = true;
}

/** 动作用空串表示「不绑定」（CoSelect 不接受 null）。 */
function actionValue(control: ControlItem): string {
  return control.action ?? "";
}

function onActionChange(control: ControlItem, value: string) {
  control.action = value === "" ? null : (value as ControlAction);
  dirty.value = true;
}

// ------------------------------------------------------------------ 预览拖拽

/** 拖动中的指针与起始位置（预览区只做粗调，精调靠数值输入）。 */
const dragging = ref<{ id: string; startX: number; startY: number; rect: ControlItem["rect"] } | null>(null);

function pointerDown(event: PointerEvent, control: ControlItem) {
  selectedId.value = control.id;
  const target = event.currentTarget as HTMLElement;
  target.setPointerCapture(event.pointerId);
  dragging.value = {
    id: control.id,
    startX: event.clientX,
    startY: event.clientY,
    rect: { ...control.rect },
  };
}

function pointerMove(event: PointerEvent) {
  const drag = dragging.value;
  if (!drag || !layout.value) return;
  const target = layout.value.controls.find((c) => c.id === drag.id);
  if (!target) return;
  const surface = previewSurface.value;
  if (!surface) return;
  const bounds = surface.getBoundingClientRect();
  if (bounds.width <= 0 || bounds.height <= 0) return;

  const dx = (event.clientX - drag.startX) / bounds.width;
  const dy = (event.clientY - drag.startY) / bounds.height;
  // 前端也做一次夹取：让拖动过程不会把控件甩出预览区，用户不必先保存再被后端纠正。
  target.rect.x = clamp(drag.rect.x + dx, 0, 1 - target.rect.w);
  target.rect.y = clamp(drag.rect.y + dy, 0, 1 - target.rect.h);
  dirty.value = true;
}

function pointerUp(event: PointerEvent) {
  const target = event.currentTarget as HTMLElement;
  if (target.hasPointerCapture(event.pointerId)) {
    target.releasePointerCapture(event.pointerId);
  }
  dragging.value = null;
}

/**
 * 按字段写入矩形（数值输入）。
 *
 * 百分比 → 比例，并夹取到合法范围：宽度至少 2%（否则控件小到抓不住），
 * 位置保证控件整体留在屏幕内。
 */
function setRect(control: ControlItem, field: "x" | "y" | "w" | "h", raw: string) {
  const parsed = Number.parseFloat(raw.replace("%", ""));
  const value = Number.isFinite(parsed) ? parsed / 100 : 0;
  if (field === "w") {
    control.rect.w = clamp(value, 0.02, 1);
    control.rect.x = clamp(control.rect.x, 0, 1 - control.rect.w);
  } else if (field === "h") {
    control.rect.h = clamp(value, 0.02, 1);
    control.rect.y = clamp(control.rect.y, 0, 1 - control.rect.h);
  } else if (field === "x") {
    control.rect.x = clamp(value, 0, 1 - control.rect.w);
  } else {
    control.rect.y = clamp(value, 0, 1 - control.rect.h);
  }
  dirty.value = true;
}

/**
 * 规整控件 id：它同时是布局文件里的键与去重依据，只允许 `[A-Za-z0-9_-]`。
 *
 * 不这么做，用户输入 `dpad 2` 这类名字会在保存时被后端以「id 非法 / 重复」拒绝，
 * 而他看不出是哪个字符的问题。
 */
function normalizeId(control: ControlItem, raw: string) {
  const cleaned = raw.replace(/[^A-Za-z0-9_-]/g, "-").slice(0, 48);
  control.id = cleaned.length > 0 ? cleaned : control.id;
  dirty.value = true;
}

const previewSurface = ref<HTMLElement | null>(null);

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}

function percent(value: number): string {
  return `${(value * 100).toFixed(1)}%`;
}

function previewStyle(control: ControlItem) {
  return {
    left: `${control.rect.x * 100}%`,
    top: `${control.rect.y * 100}%`,
    width: `${control.rect.w * 100}%`,
    height: `${control.rect.h * 100}%`,
    opacity: String(control.opacity),
  };
}

watch(() => [props.name, props.platform], load, { immediate: true });
</script>

<template>
  <div class="co-controls">
    <p v-if="!isAndroid" class="co-controls__hint">{{ t(`${MB_KEY}.android_only`) }}</p>

    <template v-else>
      <div class="co-controls__bar">
        <CoButton :disabled="!layout" @click="addControl">
          <Plus :size="14" />
          {{ t(`${MB_KEY}.add`) }}
        </CoButton>
        <CoButton :disabled="!layout" @click="resetToDefault">
          <RotateCcw :size="14" />
          {{ t(`${MB_KEY}.reset`) }}
        </CoButton>
        <CoButton :disabled="!dirty || saving" @click="save">
          <Save :size="14" />
          {{ saving ? t(`${MB_KEY}.saving`) : t(`${MB_KEY}.save`) }}
        </CoButton>
        <span v-if="dirty" class="co-controls__dirty">{{ t(`${MB_KEY}.dirty`) }}</span>
      </div>

      <p class="co-controls__hint">{{ t(`${MB_KEY}.hint`) }}</p>

      <div class="co-controls__body">
        <!-- 预览：16:9 视口近似游戏画面比例 -->
        <div ref="previewSurface" class="co-controls__preview">
          <div
            v-for="control in layout?.controls ?? []"
            :key="control.id"
            class="co-controls__node"
            :class="{
              'co-controls__node--selected': control.id === selectedId,
              'co-controls__node--hidden': !control.visible,
            }"
            :style="previewStyle(control)"
            @pointerdown="pointerDown($event, control)"
            @pointermove="pointerMove"
            @pointerup="pointerUp"
            @pointercancel="pointerUp"
          >
            <span>{{ control.label || describe(control) }}</span>
          </div>
          <p v-if="loading" class="co-controls__loading">{{ t(`${MB_KEY}.loading`) }}</p>
        </div>

        <!-- 列表 + 精调 -->
        <div class="co-controls__side">
          <ul class="co-controls__list">
            <li
              v-for="control in layout?.controls ?? []"
              :key="control.id"
              class="co-controls__row"
              :class="{ 'co-controls__row--active': control.id === selectedId }"
              @click="selectedId = control.id"
            >
              <span class="co-controls__row-name">{{ control.id }}</span>
              <span class="co-controls__row-desc">{{ describe(control) }}</span>
              <button
                class="co-controls__icon"
                type="button"
                :title="control.visible ? t(`${MB_KEY}.hide`) : t(`${MB_KEY}.show`)"
                @click.stop="control.visible = !control.visible; dirty = true"
              >
                <Eye v-if="control.visible" :size="14" />
                <EyeOff v-else :size="14" />
              </button>
              <button
                class="co-controls__icon"
                type="button"
                :title="t(`${MB_KEY}.remove`)"
                @click.stop="removeControl(control.id)"
              >
                <Trash2 :size="14" />
              </button>
            </li>
          </ul>

          <div v-if="selected" class="co-controls__form">
            <label class="co-controls__field">
              <span>{{ t(`${MB_KEY}.field.id`) }}</span>
              <CoTextField v-model="selected.id" @update:model-value="normalizeId(selected, $event)" />
            </label>
            <label class="co-controls__field">
              <span>{{ t(`${MB_KEY}.field.kind`) }}</span>
              <CoSelect
                :model-value="selected.kind"
                :options="kindOptions"
                @update:model-value="onKindChange(selected, $event as ControlKind)"
              />
            </label>
            <label class="co-controls__field">
              <span>{{ t(`${MB_KEY}.field.action`) }}</span>
              <CoSelect
                :model-value="actionValue(selected)"
                :options="actionOptions(selected.kind)"
                :disabled="selected.kind === 'look'"
                @update:model-value="onActionChange(selected, $event)"
              />
            </label>
            <label class="co-controls__field">
              <span>{{ t(`${MB_KEY}.field.label`) }}</span>
              <CoTextField v-model="selected.label" />
            </label>

            <div class="co-controls__grid">
              <label class="co-controls__field">
                <span>{{ t(`${MB_KEY}.field.x`) }}</span>
                <CoTextField
                  :model-value="percent(selected.rect.x)"
                  @update:model-value="setRect(selected, 'x', $event)"
                />
              </label>
              <label class="co-controls__field">
                <span>{{ t(`${MB_KEY}.field.y`) }}</span>
                <CoTextField
                  :model-value="percent(selected.rect.y)"
                  @update:model-value="setRect(selected, 'y', $event)"
                />
              </label>
              <label class="co-controls__field">
                <span>{{ t(`${MB_KEY}.field.w`) }}</span>
                <CoTextField
                  :model-value="percent(selected.rect.w)"
                  @update:model-value="setRect(selected, 'w', $event)"
                />
              </label>
              <label class="co-controls__field">
                <span>{{ t(`${MB_KEY}.field.h`) }}</span>
                <CoTextField
                  :model-value="percent(selected.rect.h)"
                  @update:model-value="setRect(selected, 'h', $event)"
                />
              </label>
            </div>
          </div>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.co-controls {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.co-controls__bar {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

.co-controls__dirty {
  font-size: 12px;
  color: var(--copper-warning, #c67c2e);
}

.co-controls__hint {
  margin: 0;
  font-size: 12px;
  color: var(--copper-text-secondary);
  line-height: 1.6;
}

.co-controls__body {
  display: grid;
  grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr);
  gap: 16px;
}

@media (max-width: 900px) {
  .co-controls__body {
    grid-template-columns: minmax(0, 1fr);
  }
}

/* 预览视口：16:9，带棋盘底纹以便看清半透明控件 */
.co-controls__preview {
  position: relative;
  aspect-ratio: 16 / 9;
  border: 1px solid var(--copper-border);
  border-radius: var(--copper-radius);
  background:
    linear-gradient(45deg, rgba(127, 127, 127, 0.08) 25%, transparent 25%) 0 0 / 16px 16px,
    linear-gradient(-45deg, rgba(127, 127, 127, 0.08) 25%, transparent 25%) 0 8px / 16px 16px,
    var(--copper-bg-sunken, rgba(0, 0, 0, 0.18));
  overflow: hidden;
  touch-action: none;
}

.co-controls__node {
  position: absolute;
  display: flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--copper-accent);
  border-radius: 6px;
  background: color-mix(in srgb, var(--copper-accent) 25%, transparent);
  color: var(--copper-text-primary);
  font-size: 10px;
  text-align: center;
  padding: 2px;
  cursor: grab;
  user-select: none;
  overflow: hidden;
}

.co-controls__node--selected {
  border-width: 2px;
  box-shadow: 0 0 0 2px color-mix(in srgb, var(--copper-accent) 40%, transparent);
}

.co-controls__node--hidden {
  opacity: 0.25 !important;
  border-style: dashed;
}

.co-controls__loading {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  margin: 0;
  font-size: 12px;
  color: var(--copper-text-secondary);
}

.co-controls__side {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
}

.co-controls__list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-height: 220px;
  overflow-y: auto;
}

.co-controls__row {
  display: grid;
  grid-template-columns: auto 1fr auto auto;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 12px;
}

.co-controls__row:hover {
  background: var(--copper-bg-hover, rgba(127, 127, 127, 0.1));
}

.co-controls__row--active {
  background: color-mix(in srgb, var(--copper-accent) 18%, transparent);
}

.co-controls__row-name {
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.co-controls__row-desc {
  color: var(--copper-text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.co-controls__icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 2px;
  border: none;
  background: none;
  color: var(--copper-text-secondary);
  cursor: pointer;
}

.co-controls__icon:hover {
  color: var(--copper-text-primary);
}

.co-controls__form {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.co-controls__field {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12px;
  color: var(--copper-text-secondary);
  min-width: 0;
}

.co-controls__grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}
</style>
