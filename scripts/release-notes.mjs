#!/usr/bin/env node
// 从 CHANGELOG.md 提取指定版本的发布说明，供 release 工作流生成 Release 正文。
//
// 为什么单独放一个脚本：Release 正文必须和 CHANGELOG.md 是同一份事实源。
// 在工作流里内联一段 YAML / PowerShell 既难读也难测；这里用 Node 实现，
// 本地可以直接跑，CI 与本地处理同一条路径。
//
// 用法：
//   node scripts/release-notes.mjs --version 0.0.2
//   node scripts/release-notes.mjs --version v0.0.2 --out release-notes.md
//
// 退出码：
//   0  成功
//   2  参数错误 / 缺少版本段 / 版本号与 tauri.conf.json 不一致

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const VERSION_HEADING = /^## \[([^\]]+)\](.*)$/gm;

function fail(message) {
  process.stderr.write(`[release-notes] ${message}\n`);
  process.exit(2);
}

function readJson(relativePath) {
  return JSON.parse(readFileSync(resolve(repoRoot, relativePath), "utf8"));
}

function parseArgs(argv) {
  const options = { version: null, out: null, changelog: "CHANGELOG.md", checkVersion: true };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    switch (arg) {
      case "--version":
        options.version = argv[++i] ?? null;
        if (!options.version) fail("--version 需要一个值");
        break;
      case "--out":
        options.out = argv[++i] ?? null;
        if (!options.out) fail("--out 需要一个路径");
        break;
      case "--changelog":
        options.changelog = argv[++i] ?? null;
        if (!options.changelog) fail("--changelog 需要一个路径");
        break;
      case "--no-version-check":
        options.checkVersion = false;
        break;
      case "--help":
      case "-h":
        process.stdout.write(
          "用法：node scripts/release-notes.mjs [--version x.y.z] [--out 文件] [--changelog 文件] [--no-version-check]\n",
        );
        process.exit(0);
        break;
      default:
        fail(`未知参数：${arg}`);
    }
  }
  return options;
}

function normalizeVersion(value) {
  return String(value).trim().replace(/^v/i, "");
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function listVersions(text) {
  return [...text.matchAll(VERSION_HEADING)].map((match) => match[1]);
}

function extractSection(text, version) {
  const heading = new RegExp(`^## \\[${escapeRegExp(version)}\\](?:\\s|$).*$`, "m");
  const match = heading.exec(text);
  if (!match) {
    fail(
      `CHANGELOG.md 里找不到版本 ${version} 的条目。现有版本：${
        listVersions(text).join("、") || "（无）"
      }`,
    );
  }

  const rest = text.slice(match.index + match[0].length);
  const nextHeading = rest.search(/^## \[/m);
  let body = (nextHeading === -1 ? rest : rest.slice(0, nextHeading)).trim();

  // 版本段之间用 `---` 分隔；分隔线属于排版，不属于本版本的发布说明。
  body = body.replace(/(?:\r?\n)?-{3,}\s*$/, "").trim();
  if (!body) {
    fail(`CHANGELOG.md 里版本 ${version} 的条目是空的`);
  }
  return body;
}

const options = parseArgs(process.argv.slice(2));
const changelogPath = resolve(repoRoot, options.changelog);
let changelog;
try {
  changelog = readFileSync(changelogPath, "utf8");
} catch (error) {
  fail(`读取 ${options.changelog} 失败：${error.message}`);
}

const version = normalizeVersion(
  options.version ?? readJson("src-tauri/tauri.conf.json").version,
);
if (!/^\d+\.\d+\.\d+$/.test(version)) {
  fail(`版本号格式不是 x.y.z：${version}`);
}

if (options.checkVersion) {
  const configVersion = normalizeVersion(readJson("src-tauri/tauri.conf.json").version);
  if (configVersion !== version) {
    fail(
      `发布版本 ${version} 与 src-tauri/tauri.conf.json 的 ${configVersion} 不一致；` +
        "请先同步版本号，或显式传 --no-version-check 跳过这项检查",
    );
  }
}

const body = `${extractSection(changelog, version)}\n`;

if (options.out) {
  const outPath = resolve(process.cwd(), options.out);
  writeFileSync(outPath, body, "utf8");
  process.stdout.write(`${outPath}\n`);
} else {
  process.stdout.write(body);
}
