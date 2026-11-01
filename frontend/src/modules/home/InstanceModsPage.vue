<script setup lang="ts">
// 实例 · 模组页（路由 `/version-settings/mods`）：左侧列表，右侧搜索 + 启停筛选。
//
// 列表与导入 / 启停 / 编辑清单 / 删除仍由 `ModTab` 承担（它订阅 `mods.changed`
// 自行刷新），本页只提供分类过滤与搜索词 —— 页面不再复制一套文件操作，
// 避免两处各自刷新后状态打架。
//
// 分类不显示计数：清单由 `ModTab` 内部持有，硬凑计数要么复制请求、要么把组件拆成
// 「取数 + 渲染」两半，收益不抵复杂度。

import { computed, ref } from "vue";
import { CircleCheck, CircleSlash, Layers } from "@lucide/vue";

import { useI18n } from "../../i18n";
import InstanceFilterList from "./InstanceFilterList.vue";
import InstanceManageShell from "./InstanceManageShell.vue";
import InstanceSearchBox from "./InstanceSearchBox.vue";
import ModTab from "./ModTab.vue";
import { useVersionInstance } from "./useVersionInstance";

type StatusFilter = "all" | "enabled" | "disabled";

const { t } = useI18n();
const { currentName } = useVersionInstance();

const category = ref("all");
const search = ref("");
const statusFilter = computed(() => category.value as StatusFilter);

const categories = computed(() => [
  { value: "all", label: t("module.home.category.all"), icon: Layers },
  { value: "enabled", label: t("module.home.category.enabled"), icon: CircleCheck },
  { value: "disabled", label: t("module.home.category.disabled"), icon: CircleSlash },
]);
</script>

<template>
  <InstanceManageShell :title="t('module.home.instance.page.mods')">
    <ModTab :version-name="currentName" :search="search" :filter="statusFilter" />

    <template #search>
      <InstanceSearchBox
        v-model="search"
        :placeholder="t('module.home.mods.search_placeholder')"
      />
    </template>

    <template #aside>
      <InstanceFilterList
        v-model:active="category"
        :title="t('module.home.instance.filter')"
        :items="categories"
      />
    </template>
  </InstanceManageShell>
</template>
