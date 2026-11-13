import type { ContentItem } from "./api";

/** LL sources use GitHub identities; never turn arbitrary author text into a URL. */
export function contentAuthorAvatar(item: ContentItem): string | null {
  if (item.contentType !== "ll_mod") return null;
  const project = item.id.replace(/^lip:/, "").replace(/^https?:\/\//, "");
  const projectOwner = /^(?:github\.com\/)([a-z\d](?:[a-z\d-]{0,37}[a-z\d])?)\//i.exec(project)?.[1];
  const author = item.author?.trim() || "";
  const profileOwner = /^https:\/\/github\.com\/([a-z\d](?:[a-z\d-]{0,37}[a-z\d])?)\/?$/i.exec(author)?.[1];
  const catalogOwner = item.source === "lla" && /^[a-z\d](?:[a-z\d-]{0,37}[a-z\d])?$/i.test(author) ? author : null;
  const shortProjectOwner = item.source === "lip" ? /^([a-z\d](?:[a-z\d-]{0,37}[a-z\d])?)\/[a-z\d_.-]+$/i.exec(project)?.[1] : null;
  const owner = projectOwner || shortProjectOwner || profileOwner || catalogOwner;
  return owner ? `https://github.com/${encodeURIComponent(owner)}.png?size=96` : null;
}
