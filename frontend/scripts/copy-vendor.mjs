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

import { copyFile, mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "vite";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const vendor = resolve(root, "public/vendor");
await mkdir(vendor, { recursive: true });
await copyFile(
  resolve(root, "node_modules/vue/dist/vue.esm-browser.prod.js"),
  resolve(vendor, "vue.esm-browser.prod.js"),
);
console.log("[vendor] copied standalone production Vue browser runtime");

// vue-router's browser ESM still imports @vue/devtools-api. Bundle that
// transitive dependency, but NEVER Vue: host, router and addons must all
// resolve the one bare `vue` specifier through the launcher import map.
// configFile:false prevents recursively loading our host build config.
const result = await build({
  configFile: false,
  root,
  logLevel: "warn",
  build: {
    write: false,
    minify: true,
    lib: {
      entry: resolve(root, "node_modules/vue-router/dist/vue-router.esm-browser.js"),
      formats: ["es"],
      fileName: () => "vue-router.esm-browser.js",
    },
    rolldownOptions: { external: ["vue"] },
  },
});
const bundles = Array.isArray(result) ? result : [result];
const outputs = bundles.flatMap((bundle) => bundle.output);
if (outputs.length !== 1 || outputs[0].type !== "chunk") {
  throw new Error("Router vendor build must produce exactly one ESM chunk");
}
const router = outputs[0];
if (router.imports.some((id) => id !== "vue") || router.dynamicImports.length > 0) {
  throw new Error("Router vendor has an unmapped runtime dependency");
}
if (Object.keys(router.modules).some((id) => /[/\\]node_modules[/\\](?:vue|@vue[/\\](?:runtime-core|runtime-dom|reactivity|shared))[/\\]/.test(id))) {
  throw new Error("Router vendor must not inline a second Vue runtime");
}
await writeFile(resolve(vendor, "vue-router.esm-browser.js"), router.code);
console.log("[vendor] bundled Vue Router dependency closure; only external: vue");
