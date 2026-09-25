// 把 Vue / Vue Router 的浏览器 ESM 构建复制到 public/vendor。
//
// # 为什么必须这样做
//
// 附加模块前端被构建为 ESM，并把 `vue` / `vue-router` 标记为外部依赖。只有这样
// 模块才能与宿主**共用同一个 Vue 实例**——各带一份 Vue 会导致组件无法互操作。
// 浏览器不认识裸模块说明符，必须由 `index.html` 里的 import map 指向真实文件，
// 因此这些文件必须随宿主一起分发。
//
// 由 dev / build 脚本自动执行，无需手工维护（见 package.json 的 scripts）。

import { copyFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const targets = [
  [
    "node_modules/vue/dist/vue.esm-browser.prod.js",
    "public/vendor/vue.esm-browser.prod.js",
  ],
  [
    "node_modules/vue-router/dist/vue-router.esm-browser.js",
    "public/vendor/vue-router.esm-browser.js",
  ],
];

for (const [source, destination] of targets) {
  const from = resolve(root, source);
  const to = resolve(root, destination);
  await mkdir(dirname(to), { recursive: true });
  await copyFile(from, to);
  console.log(`[vendor] ${source} -> ${destination}`);
}
