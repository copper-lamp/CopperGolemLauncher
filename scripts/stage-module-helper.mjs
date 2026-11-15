#!/usr/bin/env node
// 把附加模块 helper 放进 Tauri sidecar 目录（src-tauri/binaries），
// 让 NSIS / MSI 安装包与后续更新都带上 copper-module-helper.exe。
//
// 为什么需要这一步：helper 是内核派生出去跑不受信模块的独立进程。
// 安装包只带主程序时，用户机器上 locate_helper_program() 找不到它，
// 「设置 → 模块」和需要模块能力的页面会直接报「找不到 helper」。
//
// 用法：
//   node scripts/stage-module-helper.mjs            # release 产物
//   node scripts/stage-module-helper.mjs --debug    # debug 产物（本地联调）
//
// 产物名必须带目标三元组后缀：Tauri 的 bundle.externalBin 按
// <name>-<target-triple><.exe> 查找 sidecar，打包时会去掉三元组再落到安装目录，
// 因此安装后的文件名恰好是 helper_backend::locate_helper_program 查找的名字。

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const debug = process.argv.includes("--debug");
const profile = debug ? "debug" : "release";

function run(command, args) {
  return execFileSync(command, args, {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  }).trim();
}

const triple = (() => {
  const verbose = run("rustc", ["-vV"]);
  const host = verbose
    .split(/\r?\n/)
    .find((line) => line.startsWith("host: "));
  if (!host) {
    throw new Error("无法从 rustc -vV 读取 host 三元组");
  }
  return host.slice("host: ".length).trim();
})();

console.log(`[helper] 目标三元组：${triple}，profile：${profile}`);
const args = ["build", "-p", "copper-module-abi", "--bin", "copper-module-helper"];
if (!debug) args.push("--release");
run("cargo", args);

const exe = process.platform === "win32" ? ".exe" : "";
const targetDir = process.env.CARGO_TARGET_DIR
  ? resolve(root, process.env.CARGO_TARGET_DIR)
  : join(root, "target");
const built = join(targetDir, profile, `copper-module-helper${exe}`);
if (!existsSync(built)) {
  throw new Error(`helper 构建完成但找不到产物：${built}`);
}

const destDir = join(root, "src-tauri", "binaries");
mkdirSync(destDir, { recursive: true });
const dest = join(destDir, `copper-module-helper-${triple}${exe}`);
copyFileSync(built, dest);
console.log(`[helper] 已就位：${dest}（${statSync(dest).size} 字节）`);
