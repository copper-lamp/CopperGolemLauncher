// 实例上下文：把路由 `?name=`、版本清单与"上次选中的实例"收敛到一处。
//
// 入口页与各管理页都要回答同一个问题 —— 现在操作的是哪个实例。清单拉取、事件订阅、
// 选中项同步若各页各写一份，切页会重复请求，还容易出现"这边改了那边还是旧的"。
// 因此做成模块级单例：
// - `lastName` 记在模块作用域，跨页面导航（含面包屑跳回入口页）保留选中项；
// - 路由 `?name=` 是深链入口，优先级最高，解析后写回地址栏（刷新 / 分享不丢上下文）；
// - 订阅 `version.installed` / `version.removed` / `versions-changed`：清单变了就地刷新，
//   当前实例被删掉时回落到第一个，而不是继续显示一个已不存在的名字。

import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";

import { homeVersionGet, homeVersionsList, type VersionView } from "../../api/home";
import { onVersionInstalled, onVersionRemoved, onVersionsChanged } from "../../events";

/** 上次选中的实例名；跨页导航时用来还原上下文（不回落到"永远是第一个"）。 */
const lastName = ref("");

export function useVersionInstance() {
  const route = useRoute();
  const router = useRouter();

  const versions = ref<VersionView[]>([]);
  const current = ref<VersionView | null>(null);
  const loading = ref(true);
  const errorText = ref("");

  /** 地址栏里的实例名（深链入口）。 */
  const queryName = computed(() =>
    typeof route.query.name === "string" ? route.query.name : "",
  );

  /** 当前实例名；写入即改路由，路由是唯一事实源。 */
  const currentName = computed({
    get: () => current.value?.name ?? "",
    set: (name: string) => select(name),
  });

  async function fetchOne(name: string) {
    if (current.value?.name === name) return;
    try {
      current.value = await homeVersionGet(name);
      lastName.value = name;
      errorText.value = "";
    } catch (e) {
      // 名字已不在清单里（版本被删 / 深链写错）：回落到清单首项，别卡在坏名字上。
      const fallback = versions.value[0]?.name ?? "";
      if (fallback && fallback !== name) {
        current.value = null;
        await fetchOne(fallback);
        return;
      }
      current.value = null;
      errorText.value = String(e);
    }
  }

  /** 重新拉清单并按"路由 → 上次选中 → 首项"解析当前实例。 */
  async function refresh() {
    try {
      versions.value = await homeVersionsList();
      const preferred = queryName.value || lastName.value;
      const target = versions.value.some((v) => v.name === preferred)
        ? preferred
        : versions.value[0]?.name ?? "";
      if (!target) {
        current.value = null;
        errorText.value = "";
        return;
      }
      // The list response includes freshly revalidated compatibility capabilities.
      current.value = versions.value.find(version => version.name === target) ?? null;
      lastName.value = target;
      errorText.value = '';
      // 地址栏补齐实例名：子页面互相跳转、刷新、复制链接都不会丢上下文。
      if (queryName.value !== target) {
        void router.replace({ query: { ...route.query, name: target } });
      }
    } catch (e) {
      errorText.value = String(e);
    } finally {
      loading.value = false;
    }
  }

  /** 切换实例：只改路由，实际加载由 watch 统一处理。 */
  function select(name: string) {
    if (!name) return;
    if (name === queryName.value) {
      void fetchOne(name);
      return;
    }
    void router.replace({ query: { ...route.query, name } });
  }

  /** 保存设置 / 重命名后同步当前视图与清单里的同名项。 */
  function applyView(updated: VersionView) {
    const previous = current.value?.name ?? updated.name;
    current.value = updated;
    lastName.value = updated.name;
    const index = versions.value.findIndex(
      (v) => v.name === previous || v.name === updated.name,
    );
    if (index >= 0) versions.value[index] = updated;
    else void refresh();
    if (previous !== updated.name) {
      void router.replace({ query: { ...route.query, name: updated.name } });
    }
  }

  watch(queryName, (name) => {
    if (!name) {
      void refresh();
      return;
    }
    void fetchOne(name);
  });

  let dispose: Array<() => void> = [];

  onMounted(async () => {
    await refresh();
    // 游戏下载模块装 / 删版本、用户切游戏目录后清单会变；订阅一次即可覆盖。
    dispose = [
      await onVersionInstalled(() => void refresh()),
      await onVersionRemoved(() => void refresh()),
      await onVersionsChanged(() => void refresh()),
    ];
  });

  onUnmounted(() => {
    dispose.forEach((off) => off());
    dispose = [];
  });

  return {
    versions,
    current,
    currentName,
    loading,
    errorText,
    select,
    refresh,
    applyView,
  };
}
