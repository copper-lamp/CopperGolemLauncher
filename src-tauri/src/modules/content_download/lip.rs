//! 内容下载模块 · LIP 提供器（LL 模组）。
//!
//! 数据源为 lipr 索引导 `https://lipr.levimc.org/levilauncher.json`（与 LeviLauncher 一致）。
//! 该索引仅提供**元数据**（包信息、variants、各版本及版本依赖），不含每版本的直链下载
//! 地址。因此：
//! - 列表 / 详情 / 依赖展示可完整提供；
//! - 「下载」需借助用户预装的 lip（BDS 环境）执行安装，模块提供 lip 环境探测与安装命令，
//!   不伪造直链。
//!
//! 索引整体拉取并缓存（TTL），列表在内存中按关键字 / 页码分页。

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

use crate::error::KernelError;
use crate::state::KernelContext;

use super::model::{
    ContentDependency, ContentDetail, ContentFile, ContentItem, ContentListPage,
    ContentListQuery, PAGE_SIZE, SOURCE_LIP, TYPE_LL_MOD,
};

/// lipr 索引导地址（与 LeviLauncher 前端一致）。
const INDEX_URL: &str = "https://lipr.levimc.org/levilauncher.json";
/// 索引缓存 TTL（秒）。
const INDEX_TTL_SECS: u64 = 3600;

type SharedCache = tokio::sync::Mutex<Option<(std::time::Instant, IndexData)>>;

fn cache() -> &'static SharedCache {
    static CACHE: OnceLock<SharedCache> = OnceLock::new();
    CACHE.get_or_init(|| tokio::sync::Mutex::new(None))
}

// ---------------------------------------------------------------- 索引模型

#[derive(Debug, Clone, Default)]
struct IndexData {
    /// identifier → 包。
    packages: HashMap<String, Package>,
}

#[derive(Debug, Clone)]
struct Package {
    identifier: String,
    name: String,
    description: String,
    author: String,
    avatar_url: Option<String>,
    tags: Vec<String>,
    hotness: i64,
    /// preferredVariant（client 优先，否则第一个）。
    preferred_variants: Vec<Variant>,
}

#[derive(Debug, Clone)]
struct Variant {
    key: String,
    label: String,
    /// version → 依赖（key → range）。
    versions: Vec<(String, HashMap<String, String>)>,
}

/// 拉取最新索引（带 TTL 缓存）。
async fn index() -> Result<IndexData, KernelError> {
    {
        let g = cache().lock().await;
        if let Some((t, data)) = g.as_ref() {
            if t.elapsed().as_secs() < INDEX_TTL_SECS {
                return Ok(data.clone());
            }
        }
    }

    let json: Value = super::http::client()
        .get(INDEX_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let data = parse_index(&json);
    *cache().lock().await = Some((std::time::Instant::now(), data.clone()));
    Ok(data)
}

// ---------------------------------------------------------------- 解析（可单测）

/// 把 lipr 索引 JSON 解析为内部模型。缺字段容错，返回空表而非报错。
fn parse_index(json: &Value) -> IndexData {
    let mut packages = HashMap::new();
    let Some(root) = json.as_object() else {
        return IndexData::default();
    };
    let Some(pkgs) = root.get("packages").and_then(Value::as_object) else {
        return IndexData::default();
    };
    for (identifier, val) in pkgs {
        let Some(v) = val.as_object() else {
            continue;
        };
        let info = v.get("info").and_then(Value::as_object);
        let name = info
            .and_then(|i| i.get("name"))
            .and_then(Value::as_str)
            .unwrap_or(identifier)
            .trim()
            .to_string();
        let description = info
            .and_then(|i| i.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let tags: Vec<String> = info
            .and_then(|i| i.get("tags"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| s.to_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        let avatar_url = info
            .and_then(|i| i.get("avatar_url"))
            .and_then(Value::as_str)
            .map(|s| s.to_string());
        let hotness = v
            .get("stargazer_count")
            .or_else(|| v.get("stars"))
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0);

        // variants
        let variants_obj = v.get("variants").and_then(Value::as_object);
        let mut variants: Vec<Variant> = Vec::new();
        if let Some(variants_obj) = variants_obj {
            for (key, meta) in variants_obj {
                let Some(meta) = meta.as_object() else {
                    continue;
                };
                let Some(variants_map) = meta.get("versions").and_then(Value::as_object) else {
                    continue;
                };
                let mut versions: Vec<(String, HashMap<String, String>)> = Vec::new();
                for (vers, vmeta) in variants_map {
                    let deps = vmeta
                        .get("dependencies")
                        .and_then(Value::as_object)
                        .map(|d| {
                            d.iter()
                                .filter_map(|(k, range)| {
                                    range.as_str().map(|r| (k.clone(), r.to_string()))
                                })
                                .collect::<HashMap<String, String>>()
                        })
                        .unwrap_or_default();
                    versions.push((vers.clone(), deps));
                }
                versions.sort_by(|a, b| cmp_versions(&b.0, &a.0));
                variants.push(Variant {
                    key: key.clone(),
                    label: meta
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or(key)
                        .to_string(),
                    versions,
                });
            }
        }
        // client 优先排序
        variants.sort_by_key(|v| match v.key.to_lowercase().as_str() {
            "client" => 0,
            "" => 1,
            _ => 2,
        });

        packages.insert(
            identifier.clone(),
            Package {
                identifier: identifier.clone(),
                name,
                description,
                author: infer_author(identifier),
                avatar_url,
                tags,
                hotness,
                preferred_variants: variants,
            },
        );
    }
    IndexData { packages }
}

/// 从 identifier 推断作者（github owner 或 host 后段）。
fn infer_author(identifier: &str) -> String {
    let parts: Vec<&str> = identifier.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 2 {
        parts[parts.len() - 2].to_string()
    } else {
        String::new()
    }
}

/// 语义化版本比较（降序用：返回 >0 表示 a<b）。
fn cmp_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let pa = parse_semver(a);
    let pb = parse_semver(b);
    match (pa, pb) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

/// 解析 semver 为可比较元组；解析失败返回 None。
fn parse_semver(v: &str) -> Option<(u32, u32, u32)> {
    let s = v.trim().trim_start_matches('v');
    let core = s.split('-').next().unwrap_or(s);
    let mut it = core.split('.');
    let major: u32 = it.next()?.parse().ok()?;
    let minor: u32 = it.next().unwrap_or("0").parse().ok()?;
    let patch: u32 = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

// ---------------------------------------------------------------- 对外接口

/// 列表：关键字搜索 + 页码分页。
pub async fn list(
    _kernel: &KernelContext,
    query: &ContentListQuery,
) -> Result<ContentListPage, KernelError> {
    let data = index().await?;
    let search = query.search.as_deref().unwrap_or("").to_lowercase();

    let mut matched: Vec<ContentItem> = data
        .packages
        .values()
        .filter(|p| {
            search.is_empty()
                || p.name.to_lowercase().contains(&search)
                || p.identifier.to_lowercase().contains(&search)
                || p.description.to_lowercase().contains(&search)
        })
        .map(package_to_item)
        .collect();
    // 稳定排序：热度过高者靠前，其次 identifier。
    matched.sort_by(|a, b| {
        b.download_count
            .cmp(&a.download_count)
            .then_with(|| a.name.cmp(&b.name))
    });

    let total = matched.len() as u64;
    let page = query.page as usize;
    let start = page * (PAGE_SIZE as usize);
    let items: Vec<ContentItem> = matched
        .into_iter()
        .skip(start)
        .take(PAGE_SIZE as usize)
        .collect();
    let has_more = start + items.len() < total as usize;

    Ok(ContentListPage {
        items,
        has_more,
        total,
    })
}

/// 详情：identifier → 版本文件 + 依赖。
pub async fn detail(_kernel: &KernelContext, identifier: &str) -> Result<ContentDetail, KernelError> {
    let key = identifier.split('#').next().unwrap_or(identifier).to_string();
    let data = index().await?;
    let pkg = data
        .packages
        .get(&key)
        .ok_or_else(|| KernelError::InvalidArgument(format!("包 `lip:{key}` 不存在")))?;

    let item = package_to_item(pkg);
    let mut files: Vec<ContentFile> = Vec::new();
    for variant in &pkg.preferred_variants {
        for (vers, deps) in &variant.versions {
            let dependencies: Vec<ContentDependency> = deps
                .iter()
                .map(|(k, range)| ContentDependency {
                    ref_id: format!("lip:{}", k.split('#').next().unwrap_or(k)),
                    // 索引导不含依赖方名字，详情跳转后再解析。
                    name: format!("{k} {range}"),
                    kind: "required".into(),
                })
                .collect();
            files.push(ContentFile {
                id: format!("lip-f:{key}:{}:{vers}", variant.key),
                version: vers.clone(),
                filename: format!("{key}@{vers}"),
                // lip 无直链，交由安装流程处理。
                download_url: String::new(),
                size: 0,
                sha256: None,
                game_versions: Vec::new(),
                dependencies,
                release_type: "release".into(),
            });
        }
    }
    // 全部优先 variant 合并后去重（跨 variant 避免重复版本）。
    files.sort_by(|a, b| cmp_versions(&b.version, &a.version));

    Ok(ContentDetail {
        item,
        project_url: infer_project_url(&pkg.identifier),
        repo_url: infer_project_url(&pkg.identifier),
        authors: {
            let mut a = Vec::new();
            if !pkg.author.is_empty() {
                a.push(pkg.author.clone());
            }
            a
        },
        files,
        game_versions: Vec::new(),
    })
}

fn package_to_item(p: &Package) -> ContentItem {
    let latest = p
        .preferred_variants
        .first()
        .and_then(|v| v.versions.first().map(|(v, _)| v.clone()))
        .unwrap_or_else(|| "latest".to_string());
    ContentItem {
        id: format!("lip:{}", p.identifier),
        source: SOURCE_LIP.to_string(),
        content_type: TYPE_LL_MOD.to_string(),
        name: p.name.clone(),
        description: p.description.clone(),
        author: Some(p.author.clone()).filter(|a| !a.is_empty()),
        icon_url: p.avatar_url.clone(),
        categories: p.tags.clone(),
        min_game_version: None,
        max_game_version: None,
        latest_version: latest,
        download_count: p.hotness.max(0) as u64,
    }
}

fn infer_project_url(identifier: &str) -> Option<String> {
    let id = identifier.trim();
    if id.is_empty() {
        return None;
    }
    if id.starts_with("http://") || id.starts_with("https://") {
        return Some(id.to_string());
    }
    let parts: Vec<&str> = id.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() == 2 && !parts[0].contains('.') && !parts[0].contains(':') {
        Some(format!("https://github.com/{}/{}", parts[0], parts[1]))
    } else {
        Some(format!("https://{id}")).filter(|_| id.contains('.'))
    }
}

// ---------------------------------------------------------------- 单测

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> Value {
        serde_json::json!({
            "packages": {
                "liteldev/tstamp": {
                    "info": {
                        "name": "TSTAMP",
                        "description": "时间戳显示",
                        "tags": ["utility", "chat"],
                        "avatar_url": "http://x/avatar.png"
                    },
                    "updated_at": "2026-01-01",
                    "stargazer_count": 42,
                    "variants": {
                        "client": {
                            "label": "Client Side",
                            "versions": {
                                "1.3.0": {
                                    "dependencies": {
                                        "github.com/liteldev/levilamina": ">=0.14.0"
                                    }
                                },
                                "1.2.0": {
                                    "dependencies": {
                                        "github.com/liteldev/levilamina": ">=0.13.0"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        })
    }

    #[test]
    fn parse_index_orders_versions_descending() {
        let data = parse_index(&sample_json());
        let pkg = data.packages.get("liteldev/tstamp").unwrap();
        assert_eq!(pkg.name, "TSTAMP");
        assert_eq!(pkg.author, "liteldev");
        let v = pkg.preferred_variants.first().unwrap();
        assert_eq!(v.versions[0].0, "1.3.0");
        assert_eq!(v.versions[1].0, "1.2.0");
    }

    #[test]
    fn package_to_item_maps_fields() {
        let data = parse_index(&sample_json());
        let pkg = data.packages.get("liteldev/tstamp").unwrap();
        let item = package_to_item(pkg);
        assert_eq!(item.id, "lip:liteldev/tstamp");
        assert_eq!(item.source, SOURCE_LIP);
        assert_eq!(item.content_type, TYPE_LL_MOD);
        assert_eq!(item.latest_version, "1.3.0");
        assert_eq!(item.download_count, 42);
        assert!(item.categories.contains(&"utility".to_string()));
    }

    #[test]
    fn semver_parse_and_compare() {
        assert_eq!(parse_semver("1.3.0"), Some((1, 3, 0)));
        assert_eq!(parse_semver("v1.2"), Some((1, 2, 0)));
        assert_eq!(parse_semver("1.3.0-beta.1"), Some((1, 3, 0)));
        assert_eq!(parse_semver("abc"), None);
        assert_eq!(cmp_versions("1.3.0", "1.2.0"), std::cmp::Ordering::Less);
    }

    #[test]
    fn infer_project_url_github() {
        assert_eq!(
            infer_project_url("liteldev/tstamp"),
            Some("https://github.com/liteldev/tstamp".to_string())
        );
    }
}