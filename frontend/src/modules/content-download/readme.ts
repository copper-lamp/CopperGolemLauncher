// readme 渲染：把来源原文安全地转成可插入 DOM 的 HTML。
//
// 两个来源格式不同，必须分别处理，否则会出现「源码直接显示」的不兼容现象：
// - CurseForge 项目描述为 HTML 片段；
// - LIP（LL 模组）的 readme 取自 GitHub 仓库，为 Markdown 原文。
//
// 统一流程：Markdown → marked 转 HTML；HTML → 直接使用；随后一律经 DOMPurify
// 白名单净化（防脚本注入），最后把相对链接 / 图片绝对化（GitHub README 大量
// 使用相对路径，不处理会 404）。

import DOMPurify from "dompurify";
import { marked } from "marked";

/** 原文格式。 */
export type ReadmeFormat = "markdown" | "html";

/** 净化白名单：仅保留文档类标签，剔除表单 / 嵌入 / 样式等可执行或影响布局的节点。 */
const ALLOWED_TAGS = [
  "a", "b", "blockquote", "br", "code", "dd", "del", "details", "div", "dl", "dt",
  "em", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "i", "img", "input", "kbd", "li",
  "ol", "p", "picture", "pre", "s", "samp", "source", "span", "strong", "sub",
  "summary", "sup", "table", "tbody", "td", "tfoot", "th", "thead", "tr", "ul", "var",
];

const ALLOWED_ATTR = [
  "align", "alt", "checked", "class", "colspan", "disabled", "height", "href",
  "rowspan", "src", "srcset", "start", "title", "type", "width",
];

/**
 * 渲染 readme 原文为可安全插入的 HTML。
 *
 * @param raw     来源返回的原文；空值返回空串。
 * @param format  `markdown`（lip）或 `html`（CurseForge）。
 * @param baseUrl 相对链接 / 图片的基准地址（如仓库主页），缺省则不转换。
 */
export function renderReadme(
  raw: string | null | undefined,
  format: ReadmeFormat,
  baseUrl?: string | null,
): string {
  if (!raw || !raw.trim()) return "";

  let html: string;
  if (format === "markdown") {
    html = marked.parse(raw, { async: false, gfm: true, breaks: false });
    html = convertGithubAlerts(html);
  } else {
    html = raw;
  }

  const clean = DOMPurify.sanitize(html, {
    ALLOWED_TAGS,
    ALLOWED_ATTR,
    // 禁止 data: / javascript: 等可执行或内嵌载荷的 URL。
    ALLOWED_URI_REGEXP: /^(?:https?|mailto|tel|#|\/(?!\/)|\.\/|\.\.\/)/i,
  }) as string;

  return absolutize(clean, baseUrl);
}

/** GitHub 告警块类型（`> [!TYPE]`），与 GitHub 官方语法一致。 */
const ALERT_TYPES = ["note", "tip", "important", "warning", "caution"] as const;
const ALERT_PATTERN = new RegExp(`^\\[!(${ALERT_TYPES.join("|")})\\][ \\t]*\\r?\\n?`, "i");

/**
 * 把 GitHub 告警语法 `> [!WARNING]` 转成带类型的引用块。
 *
 * `marked` 不认识该扩展语法，不转换会在正文里原样显示 `[!WARNING]` 文本
 * （用户反馈的「不兼容显示」之一）。转换后由 `.cd-doc blockquote.cd-alert--*`
 * 按类型着色。
 *
 * 标记一定位于引用块首个段落的首个文本节点起始处，因此只裁切该节点，
 * 不跨元素边界（首行后常紧跟 `<strong>` 等行内元素，跨节点裁切会损坏正文）。
 */
function convertGithubAlerts(html: string): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  doc.querySelectorAll("blockquote").forEach((quote) => {
    const first = quote.querySelector("p");
    const firstText = first?.firstChild;
    if (!first || !firstText || firstText.nodeType !== Node.TEXT_NODE) return;

    const text = firstText.textContent ?? "";
    const marker = ALERT_PATTERN.exec(text);
    if (!marker) return;
    const kind = marker[1].toLowerCase();

    firstText.textContent = text.slice(marker[0].length);
    if (!(first.textContent ?? "").trim()) first.remove();

    const title = doc.createElement("p");
    title.className = "cd-alert__title";
    title.textContent = kind.toUpperCase();
    quote.classList.add("cd-alert", `cd-alert--${kind}`);
    quote.prepend(title);
  });
  return doc.body.innerHTML;
}

/**
 * 把相对链接 / 图片地址改写为绝对地址，并给外链补安全属性。
 *
 * 纯 DOM 变换（不注册 DOMPurify 全局 hook），避免污染全局状态。
 */
function absolutize(html: string, baseUrl?: string | null): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  if (!baseUrl || !baseUrl.trim()) return doc.body.innerHTML;

  // GitHub 仓库主页下，链接应落在 `blob/<ref>/`、图片应落在 `raw/<ref>/`，
  // 直接按主页拼接会 404（README 大量使用相对路径）。非 GitHub 则回落为同址拼接。
  const repo = parseGithubRepo(baseUrl);
  const base = baseUrl.trim().replace(/\/+$/, "");
  const linkBase = repo ? `${repo.host}/${repo.owner}/${repo.repo}/blob/HEAD` : base;
  const assetBase = repo ? `${repo.host}/${repo.owner}/${repo.repo}/raw/HEAD` : base;

  doc.querySelectorAll("a[href]").forEach((a) => {
    const href = a.getAttribute("href") ?? "";
    // 纯页内锚点保持原页跳转，不新开窗口。
    if (href.startsWith("#")) return;
    if (isRelative(href)) {
      a.setAttribute("href", joinUrl(linkBase, href));
    }
    a.setAttribute("target", "_blank");
    a.setAttribute("rel", "noopener noreferrer nofollow");
  });

  doc.querySelectorAll("img[src]").forEach((img) => {
    const src = img.getAttribute("src") ?? "";
    if (isRelative(src)) img.setAttribute("src", joinUrl(assetBase, src));
  });

  doc.querySelectorAll("source[srcset]").forEach((s) => {
    const srcset = s.getAttribute("srcset") ?? "";
    const fixed = srcset
      .split(",")
      .map((part) => {
        const [url, ...rest] = part.trim().split(/\s+/);
        if (!url || !isRelative(url)) return part.trim();
        return [joinUrl(assetBase, url), ...rest].join(" ");
      })
      .join(", ");
    s.setAttribute("srcset", fixed);
  });

  return doc.body.innerHTML;
}

/** 识别 GitHub 仓库主页，返回归一后的三要素；非 GitHub 返回 null。 */
function parseGithubRepo(
  url: string,
): { host: string; owner: string; repo: string } | null {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  const host = parsed.hostname.toLowerCase();
  if (host !== "github.com" && host !== "www.github.com") return null;
  const segs = parsed.pathname.split("/").filter(Boolean);
  const owner = segs[0];
  const repo = segs[1]?.replace(/\.git$/, "");
  if (!owner || !repo) return null;
  return { host: "https://github.com", owner, repo };
}

/** 判断是否为相对路径（含锚点与协议相对写法）。 */
function isRelative(url: string): boolean {
  if (!url) return false;
  if (url.startsWith("#")) return false;
  if (url.startsWith("//")) return false;
  return !/^[a-z][a-z0-9+.-]*:/i.test(url);
}

/** 拼接基准与相对路径，处理 `./`、`../` 与根路径。 */
function joinUrl(base: string, rel: string): string {
  if (rel.startsWith("/")) {
    // 根路径：取基准的 origin（+ 仓库层级由调用方保证）。
    try {
      return new URL(rel, base).toString();
    } catch {
      return rel;
    }
  }
  try {
    return new URL(rel, `${base}/`).toString();
  } catch {
    return rel;
  }
}
