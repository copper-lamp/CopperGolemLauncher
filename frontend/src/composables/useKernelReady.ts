// 内核就绪状态：Shell 据此决定显示启动加载页还是主界面。
//
// 为什么单独成一个模块：`main.ts` 负责初始化内核能力，`App.vue` 负责渲染，
// 两者都需要读写「是否就绪」，但又不应互相 import（会造成循环依赖）。
// 此处只放一个极薄的响应式开关，谁都能依赖。

import { ref } from "vue";

/** 内核能力是否已就绪（i18n / 主题可用，首屏可以安全渲染）。 */
const kernelReady = ref(false);

/** 就绪状态（响应式，只读语义：外部经 `markKernelReady` 置位）。 */
export const isKernelReady = kernelReady;

/**
 * 标记内核就绪。
 *
 * 刻意设计为**幂等且不可回退**：初始化失败也要调用它，否则内核不可用
 * （浏览器调试、后端异常）时用户会永远停在加载页，看不到任何错误反馈。
 */
export function markKernelReady(): void {
  kernelReady.value = true;
}

/** 可组合式入口。 */
export function useKernelReady() {
  return { isKernelReady, markKernelReady };
}
