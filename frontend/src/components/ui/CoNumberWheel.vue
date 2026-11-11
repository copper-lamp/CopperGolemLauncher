<script setup lang="ts">
// 数字滚轮：把整数渲染成「一位一列」的竖向数字带，值变化时每一列滚到目标位。
//
// 移植自 rare-ui animated-counter（React + motion/react），改为 Vue + rAF + 手写弹簧，零依赖：
// - 每列是 0..9 + 再补一个 0 共 11 个面，9 -> 0 落到同一字面，不会出现空档；
// - 上下用 mask 渐隐遮罩（线性渐变的透明度曲线经过缓动，直来直去会看到硬边），
//   静止时数字是实心的，滚动时上下沿自然淡出；
// - 滚动方向跟着数值的增减走：递增一律向上进位、递减一律向下退位，
//   否则 9 -> 0 会绕九格；
// - **直接写 transform，不把滚动位置放进响应式**：扫页时每帧都会变值，
//   走 Vue 的 diff 会抖，而且每帧重排 11 个面毫无必要；
// - 尊重 prefers-reduced-motion：开启减动效时直接落位，不做补间。
//
// 宽度用 1ch + tabular-nums 对齐，位数变化时整体宽度自然伸缩，不需要量字体。

import { onBeforeUnmount, ref, watch } from "vue";
import { motionEnabled } from "../../composables/useVisualPreferences";

const props = withDefaults(
  defineProps<{
    /** 显示的值，按绝对值取整渲染。 */
    value: number;
    /** 最少位数，不足左侧补 0（固定宽度用）。0 表示按实际位数。 */
    padStart?: number;
    /** 一列滚到目标位的时长（秒）。 */
    duration?: number;
  }>(),
  { padStart: 0, duration: 0.5 },
);

const FACES = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
/** 末尾补一个 0：第 10 格与第 0 格字面相同，9 进位到 10 时视觉无断点。 */
const WHEEL = [...FACES, 0];
const STEPS = WHEEL.length;

/** 阻尼比：略小于 1，收尾有一点点回弹，像真的轮子停住。 */
const ZETA = 0.82;
const MIN_DT = 1 / 240;
const MAX_DT = 1 / 30;

const mod = (n: number, m: number) => ((n % m) + m) % m;

type Col = {
  /** 连续位置（以「格」为单位，可以带小数，滚动中）。 */
  pos: number;
  target: number;
  vel: number;
  el: HTMLElement | null;
};

/** 列的 key：从右往左的第几位。位数增减时低位列保持同一 DOM，不会整体重挂。 */
const keys = ref<number[]>([]);
const cols = new Map<number, Col>();
/** 无障碍文案（视觉上隐藏）。 */
const text = ref("0");

watch(motionEnabled, enabled => { if (!enabled) { if (raf) cancelAnimationFrame(raf); raf = 0; for (const col of cols.values()) { col.pos = col.target; col.vel = 0; write(col); } } });

let raf = 0;
let lastFrame = 0;
let lastValue = NaN;
let inited = false;

function digitsOf(v: number): number[] {
  const n = Math.abs(Math.trunc(Number.isFinite(v) ? v : 0));
  return String(n)
    .padStart(Math.max(props.padStart, 1), "0")
    .split("")
    .map(Number);
}

function write(col: Col) {
  if (!col.el) return;
  col.el.style.transform = `translate3d(0, ${(-mod(col.pos, 10) * 100) / STEPS}%, 0)`;
}

function bind(key: number, el: unknown) {
  const col = cols.get(key);
  if (!col) return;
  col.el = (el as HTMLElement | null) ?? null;
  if (col.el) write(col);
}

/** 从当前位置出发，按方向落到最近的、个位等于 digit 的那一格。 */
function aim(from: number, digit: number, dir: number) {
  return dir < 0 ? from - mod(from - digit, 10) : from + mod(digit - from, 10);
}

function tick(now: number) {
  raf = 0;
  const dt = Math.min(MAX_DT, Math.max(MIN_DT, (now - lastFrame) / 1000));
  lastFrame = now;

  const omega = (2 * Math.PI) / Math.max(0.06, props.duration);
  let moving = false;

  for (const col of cols.values()) {
    const delta = col.target - col.pos;
    if (Math.abs(delta) < 0.0005 && Math.abs(col.vel) < 0.01) {
      if (col.pos !== col.target) {
        col.pos = col.target;
        write(col);
      }
      col.vel = 0;
      continue;
    }
    // 弹簧：向目标加速，速度带来阻尼，不用缓动曲线也能有惯性和收尾。
    const accel = omega * omega * delta - 2 * ZETA * omega * col.vel;
    col.vel += accel * dt;
    col.pos += col.vel * dt;
    write(col);
    moving = true;
  }

  if (moving) raf = requestAnimationFrame(tick);
}

function start() {
  if (!motionEnabled.value) {
    for (const col of cols.values()) {
      col.pos = col.target;
      col.vel = 0;
      write(col);
    }
    return;
  }
  if (raf) return;
  lastFrame = performance.now();
  raf = requestAnimationFrame(tick);
}

function sync(v: number, animate: boolean) {
  const digits = digitsOf(v);
  const width = digits.length;
  text.value = digits.join("");

  const dir = !Number.isFinite(lastValue) || v >= lastValue ? 1 : -1;
  lastValue = v;

  const nextKeys: number[] = [];
  for (let i = 0; i < width; i += 1) {
    const key = width - 1 - i;
    const digit = digits[i];
    nextKeys.push(key);

    const existing = cols.get(key);
    if (existing) {
      // 以「目标位」而不是「当前位置」为基准重新瞄准：快速连续变值时
      // 不会因为还在半路而算错方向。
      if (mod(Math.round(existing.target), 10) !== digit) {
        existing.target = aim(existing.target, digit, dir);
      }
      continue;
    }
    // 新出现的高位：从反方向滚进来（原位-10 格），而不是凭空跳出来。
    cols.set(key, { pos: digit - dir * 10, target: digit, vel: 0, el: null });
  }

  // 位数变少时删掉高位列。低位列的 key 会变，重建后从 0 起步也可以接受。
  for (const key of [...cols.keys()]) {
    if (!nextKeys.includes(key)) cols.delete(key);
  }

  keys.value = nextKeys;

  if (!animate || !motionEnabled.value || !inited) {
    for (const col of cols.values()) {
      col.pos = col.target;
      col.vel = 0;
      write(col);
    }
    inited = true;
    return;
  }
  start();
}

watch(
  () => props.value,
  (v) => sync(v, true),
  { immediate: true },
);

onBeforeUnmount(() => {
  if (raf) cancelAnimationFrame(raf);
  raf = 0;
});
</script>

<template>
  <span class="co-numwheel">
    <span class="co-numwheel__sr">{{ text }}</span>
    <span class="co-numwheel__row" aria-hidden="true">
      <span v-for="key in keys" :key="key" class="co-numwheel__digit">
        <span :ref="(el) => bind(key, el)" class="co-numwheel__stack">
          <span v-for="(face, i) in WHEEL" :key="i" class="co-numwheel__face">{{ face }}</span>
        </span>
      </span>
    </span>
  </span>
</template>

<style scoped>
.co-numwheel {
  display: inline-flex;
  align-items: center;
  font-variant-numeric: tabular-nums;
}

.co-numwheel__row {
  display: inline-flex;
  align-items: center;
}

/* 一列：只露一个面，上下靠遮罩淡出。em 跟着父级字号走，控件放进哪里都不会错位。 */
.co-numwheel__digit {
  position: relative;
  display: inline-block;
  width: 1ch;
  height: 1.5em;
  overflow: hidden;
  line-height: 1.5em;
  text-align: center;
  mask-image: linear-gradient(
    to bottom,
    rgba(0, 0, 0, 0) 0%,
    rgba(0, 0, 0, 0.06) 5.5%,
    rgba(0, 0, 0, 0.5) 11%,
    rgba(0, 0, 0, 0.94) 16.5%,
    #000 22%,
    #000 78%,
    rgba(0, 0, 0, 0.94) 83.5%,
    rgba(0, 0, 0, 0.5) 89%,
    rgba(0, 0, 0, 0.06) 94.5%,
    rgba(0, 0, 0, 0) 100%
  );
  -webkit-mask-image: linear-gradient(
    to bottom,
    rgba(0, 0, 0, 0) 0%,
    rgba(0, 0, 0, 0.06) 5.5%,
    rgba(0, 0, 0, 0.5) 11%,
    rgba(0, 0, 0, 0.94) 16.5%,
    #000 22%,
    #000 78%,
    rgba(0, 0, 0, 0.94) 83.5%,
    rgba(0, 0, 0, 0.5) 89%,
    rgba(0, 0, 0, 0.06) 94.5%,
    rgba(0, 0, 0, 0) 100%
  );
}

.co-numwheel__stack {
  position: absolute;
  inset-inline: 0;
  top: 0;
  display: block;
  will-change: transform;
}

.co-numwheel__face {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 1.5em;
}

.co-numwheel__sr {
  position: absolute;
  width: 1px;
  height: 1px;
  margin: -1px;
  padding: 0;
  overflow: hidden;
  clip-path: inset(50%);
  white-space: nowrap;
  border: 0;
}
</style>
