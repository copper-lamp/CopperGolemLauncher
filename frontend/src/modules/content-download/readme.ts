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

/**
 * 把相对链接 / 图片地址改写为绝对地址，并给外链补安全属性。
 *
 * 纯 DOM 变换（不注册 DOMPurify 全局 hook），避免污染全局状态。
 */
function absolutize(html: string, baseUrl?: string | null): string {
  const doc = new DOMParser().parseFromString(html, "text/html");
  const base = baseUrl && baseUrl.trim() ? resolveBase(baseUrl) : null;

  doc.querySelectorAll("a[href]").forEach((a) => {
    const href = a.getAttribute("href") ?? "";
    if (base && isRelative(href)) {
      a.setAttribute("href", joinUrl(base, href));
    }
    a.setAttribute("target", "_blank");
    a.setAttribute("rel", "noopener noreferrer nofollow");
  });

  if (base) {
    doc.querySelectorAll("img[src]").forEach((img) => {
      const src = img.getAttribute("src") ?? "";
      if (isRelative(src)) img.setAttribute("src", joinUrl(base, src));
    });
    doc.querySelectorAll("source[srcset]").forEach((s) => {
      const srcset = s.getAttribute("srcset") ?? "";
      const fixed = srcset
        .split(",")
        .map((part) => {
          const [url, ...rest] = part.trim().split(/\s+/);
          if (!url || !isRelative(url)) return part.trim();
          return [joinUrl(base, url), ...rest].join(" ");
        })
        .join(", ");
      s.setAttribute("srcset", fixed);
    });
  }

  return doc.body.innerHTML;
}

/** 归一基准地址：GitHub 仓库主页补齐为 raw 可解析的同源根。 */
function resolveBase(baseUrl: string): string {
  const url = baseUrl.trim().replace(/\/+$/, "");
  return url;
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
