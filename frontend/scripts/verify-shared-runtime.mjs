import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import { resolve } from "node:path";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { parseAst } from "rolldown/parseAst";

const dist = fileURLToPath(new URL("../dist/", import.meta.url));
const html = await readFile(resolve(dist, "index.html"), "utf8");
const map = JSON.parse(html.match(/<script type="importmap">([\s\S]*?)<\/script>/)[1]).imports;
assert.deepEqual(map, {
  vue: "/vendor/vue.esm-browser.prod.js",
  "vue-router": "/vendor/vue-router.esm-browser.js",
});
const bare = new Map();
let count = 0;
async function inspect(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const file = resolve(directory, entry.name);
    if (entry.isDirectory()) { await inspect(file); continue; }
    if (!entry.name.endsWith(".js")) continue;
    count++;
    const code = await readFile(file, "utf8");
    const ast = parseAst(code);
    function walk(node) {
      if (!node || typeof node !== "object") return;
      let specifier;
      if (node.type === "ImportDeclaration" || node.type === "ExportNamedDeclaration" || node.type === "ExportAllDeclaration") specifier = node.source?.value;
      if (node.type === "ImportExpression" && node.source?.type === "Literal") specifier = node.source.value;
      if (typeof specifier === "string" && !specifier.startsWith(".") && !specifier.startsWith("/")) {
        assert.ok(Object.hasOwn(map, specifier), `${file}: unmapped bare import ${specifier}`);
        bare.set(specifier, (bare.get(specifier) ?? 0) + 1);
      }
      for (const value of Object.values(node)) {
        if (Array.isArray(value)) value.forEach(walk);
        else if (value && typeof value === "object") walk(value);
      }
    }
    walk(ast);
  }
}
await inspect(resolve(dist, "assets"));
const router = await readFile(resolve(dist, "vendor/vue-router.esm-browser.js"), "utf8");
const routerImports = parseAst(router).body.filter((node) => node.type === "ImportDeclaration").map((node) => node.source.value);
assert.deepEqual(routerImports, ["vue"]);
assert.ok(bare.has("vue") && bare.has("vue-router"), "Host must use both shared bare imports");
assert.equal(await readFile(resolve(dist, "vendor/vue.esm-browser.prod.js"), "utf8"), await readFile(fileURLToPath(new URL("../node_modules/vue/dist/vue.esm-browser.prod.js", import.meta.url)), "utf8"));
console.log(JSON.stringify({ jsChunks: count, hostBareImports: Object.fromEntries(bare), importMap: map, routerImports, vueSha256: createHash("sha256").update(await readFile(resolve(dist, "vendor/vue.esm-browser.prod.js"))).digest("hex"), routerSha256: createHash("sha256").update(router).digest("hex") }, null, 2));