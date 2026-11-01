// 详情页面包屑末级的标题来源。
//
// 为什么不用 `route.meta.breadcrumb` 里的字面量：
// - 路由记录里的 meta 数组在多次导航之间是**同一个引用**，上一页写进去的内容名会留到
//   下一页，切换详情时先看到旧名字（实测：打开 LeviLamina 时面包屑还显示上一个条目）；
// - `onUnmounted` 里再删也赶不上新页面首帧。
//
// 这个 ref 由 register.ts 的 getter 读取、由 DetailView 写入，
// 导航时先清空、拿到数据后再填 —— 任何时刻面包屑都不会出现"上一个内容"。
import { ref } from "vue";

export const detailCrumbTitle = ref("");
