//! LIP 包文件（lip.toml / tooth.json）解析与校验。
//!
//! 只依据公开协议独立实现（protocol/lip.toml.md、lip.schema.toml、
//! lip-compatibility.md），不搬运上游 GPL 源码。

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use super::{Environment, LipError};

pub const PACKAGE_FILE: &str = "lip.toml";
pub const LEGACY_PACKAGE_FILE: &str = "tooth.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Info {
    #[serde(default, skip_serializing_if = "String::is_empty")] pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")] pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub logo: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageFile {
    pub format: u32,
    pub id: String,
    pub version: String,
    #[serde(default)] pub info: Info,
    pub variants: BTreeMap<String, Vec<Entry>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Entry {
    pub side: String,
    pub platform: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")] pub deps: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub assets: Vec<Asset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub preserve: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")] pub purge: Vec<String>,
    #[serde(default, skip_serializing_if = "Hooks::is_empty")] pub hooks: Hooks,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Asset {
    SelfArchive { #[serde(default)] place: Vec<Place> },
    Zip { urls: Vec<String>, #[serde(default)] place: Vec<Place> },
    Mcmod { urls: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Place {
    pub src: String,
    pub dest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Hooks {
    #[serde(default)] pub pre_install: Vec<String>,
    #[serde(default)] pub post_install: Vec<String>,
    #[serde(default)] pub pre_uninstall: Vec<String>,
    #[serde(default)] pub post_uninstall: Vec<String>,
}

impl Hooks {
    pub fn is_empty(&self) -> bool {
        self.pre_install.is_empty() && self.post_install.is_empty() && self.pre_uninstall.is_empty() && self.post_uninstall.is_empty()
    }
}

impl PackageFile {
    pub fn version(&self) -> Result<semver::Version, LipError> {
        semver::Version::parse(&self.version).map_err(|_| LipError::InvalidManifest(format!("package {} has invalid version {}", self.id, self.version)))
    }

    /// 选出适用于该环境的条目。
    ///
    /// 先按变体名精确匹配。若该变体在清单里不存在（典型情况：lipr 索引把无 label 的
    /// tooth 变体标注成 client，而 tooth.json 里的 label 为空），回退到默认变体空串。
    /// 只回退到空 label 这一个变体，绝不回退到「任意一个条目」——tooth 里客户端与服务端
    /// 是并列变体，乱选会把服务端依赖/落点带进客户端实例。
    pub fn entry(&self, variant: &str, environment: &Environment) -> Option<&Entry> {
        let matches = |entry: &&Entry| entry.side == environment.side && entry.platform == environment.platform;
        if let Some(entries) = self.variants.get(variant) {
            if let Some(entry) = entries.iter().find(matches) {
                return Some(entry);
            }
        }
        self.variants.get("")?.iter().find(matches)
    }

    pub fn has_mod_asset(&self) -> bool {
        self.variants.values().flatten().any(|entry| entry.assets.iter().any(|asset| matches!(asset, Asset::Mcmod { .. })))
    }
}

/// 解析 lip.toml 文本：渲染模板、转成 JSON、做协议校验。
pub fn parse_package_file(text: &str) -> Result<(PackageFile, Json), LipError> {
    let mut value: toml::Value = text.parse().map_err(|error| LipError::InvalidManifest(format!("lip.toml is not valid TOML: {error}")))?;
    render_templates(&mut value);
    let json = toml_to_json(value);
    let file = parse_package_json(&json)?;
    Ok((file, json))
}

/// 校验已渲染的包文件 JSON。
pub fn parse_package_json(json: &Json) -> Result<PackageFile, LipError> {
    let file: PackageFile = serde_json::from_value(json.clone())
        .map_err(|error| LipError::InvalidManifest(format!("invalid package file: {error}")))?;
    validate(&file, json)?;
    Ok(file)
}

/// 渲染每个字符串里的模板变量。变量取顶层成员的原始值。
fn render_templates(value: &mut toml::Value) {
    let variables: HashMap<String, toml::Value> = match value {
        toml::Value::Table(table) => table.clone().into_iter().collect(),
        _ => HashMap::new(),
    };
    render_in_place(value, &variables);
}

fn render_in_place(value: &mut toml::Value, variables: &HashMap<String, toml::Value>) {
    match value {
        toml::Value::String(text) => *text = render_string(text, variables),
        toml::Value::Array(items) => {
            for item in items {
                render_in_place(item, variables);
            }
        }
        toml::Value::Table(table) => {
            for (_, item) in table.iter_mut() {
                render_in_place(item, variables);
            }
        }
        _ => {}
    }
}

/// 只支持协议里实际使用的 {{name}} 形式；无法渲染时保留原文，
/// 由后续校验报错（不静默吞掉模板）。
fn render_string(text: &str, variables: &HashMap<String, toml::Value>) -> String {
    if !text.contains("{{") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..end].trim();
        match variables.get(name) {
            Some(toml::Value::String(value)) => out.push_str(value),
            Some(toml::Value::Integer(value)) => out.push_str(&value.to_string()),
            Some(toml::Value::Boolean(value)) => out.push_str(&value.to_string()),
            Some(toml::Value::Float(value)) => out.push_str(&value.to_string()),
            _ => {
                // 未知变量：保留模板原文，让校验失败而不是写入错误内容。
                out.push_str("{{");
                out.push_str(name);
                out.push_str("}}");
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn toml_to_json(value: toml::Value) -> Json {
    match value {
        toml::Value::String(text) => Json::String(text),
        toml::Value::Integer(number) => Json::Number(number.into()),
        toml::Value::Float(number) => serde_json::Number::from_f64(number).map(Json::Number).unwrap_or(Json::Null),
        toml::Value::Boolean(flag) => Json::Bool(flag),
        toml::Value::Datetime(datetime) => Json::String(datetime.to_string()),
        toml::Value::Array(items) => Json::Array(items.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => Json::Object(table.into_iter().map(|(key, item)| (key, toml_to_json(item))).collect()),
    }
}

fn validate(file: &PackageFile, json: &Json) -> Result<(), LipError> {
    if file.format != 4 {
        return Err(LipError::InvalidManifest(format!("unsupported package format {}; expected 4", file.format)));
    }
    if !valid_id(&file.id) {
        return Err(LipError::InvalidManifest(format!("invalid package id {}", file.id)));
    }
    if semver::Version::parse(&file.version).is_err() {
        return Err(LipError::InvalidManifest(format!("invalid package version {}", file.version)));
    }
    if file.variants.is_empty() {
        return Err(LipError::InvalidManifest("package has no variants".into()));
    }
    for (variant, entries) in &file.variants {
        if !valid_variant(variant) {
            return Err(LipError::InvalidManifest(format!("invalid variant name {variant:?}")));
        }
        if entries.is_empty() {
            return Err(LipError::InvalidManifest(format!("variant {variant:?} has no entries")));
        }
        let mut seen = std::collections::HashSet::new();
        for entry in entries {
            if !matches!(entry.side.as_str(), "client" | "server") {
                return Err(LipError::InvalidManifest(format!("invalid side {:?}", entry.side)));
            }
            if entry.platform.is_empty() || matches!(entry.platform.as_str(), "client" | "server") {
                return Err(LipError::InvalidManifest(format!("invalid platform {:?}", entry.platform)));
            }
            if !seen.insert((entry.side.clone(), entry.platform.clone())) {
                return Err(LipError::InvalidManifest("duplicate side/platform entry".into()));
            }
            if let Some(runtime) = json.get("variants").and_then(|variants| variants.get(variant)).and_then(|entries| entries.as_array()).and_then(|entries| entries.iter().find(|item| item.get("runtime").is_some())) {
                let _ = runtime;
                return Err(LipError::InvalidManifest("entry uses the removed runtime key (pre-v0.40 package)".into()));
            }
            for (key, range) in &entry.deps {
                let (id, variant) = split_dependency(key);
                if !valid_id(id) || !valid_variant(variant) {
                    return Err(LipError::InvalidManifest(format!("invalid dependency key {key:?}")));
                }
                super::range::VersionRange::parse(range).map_err(|_| LipError::InvalidManifest(format!("invalid dependency range {range:?} for {key}")))?;
            }
            let mut mod_assets = 0;
            for asset in &entry.assets {
                match asset {
                    Asset::SelfArchive { place } => validate_places(place)?,
                    Asset::Zip { urls, place } => {
                        validate_urls(urls)?;
                        validate_places(place)?;
                    }
                    Asset::Mcmod { urls } => {
                        validate_urls(urls)?;
                        mod_assets += 1;
                    }
                }
            }
            if mod_assets > 1 {
                return Err(LipError::InvalidManifest("more than one mcmod asset".into()));
            }
            for pattern in entry.preserve.iter().chain(entry.purge.iter()) {
                if !valid_glob(pattern) {
                    return Err(LipError::InvalidManifest(format!("invalid glob {pattern:?}")));
                }
            }
        }
    }
    Ok(())
}

fn validate_urls(urls: &[String]) -> Result<(), LipError> {
    if urls.is_empty() {
        return Err(LipError::InvalidManifest("asset has no urls".into()));
    }
    for url in urls {
        if !url.starts_with("https://") {
            return Err(LipError::InvalidManifest(format!("asset url is not https: {url}")));
        }
        if url.contains("{{") {
            return Err(LipError::InvalidManifest(format!("asset url has an unrendered template: {url}")));
        }
    }
    Ok(())
}

fn validate_places(place: &[Place]) -> Result<(), LipError> {
    for item in place {
        if !valid_relative_path(&item.src) || !valid_relative_path(&item.dest) {
            return Err(LipError::InvalidManifest(format!("invalid place path {:?} -> {:?}", item.src, item.dest)));
        }
        let src_glob = item.src.contains(['*', '?', '[']);
        let src_dir = item.src.is_empty() || item.src.ends_with('/');
        if !src_glob && !src_dir && (item.dest.is_empty() || item.dest.ends_with('/')) {
            return Err(LipError::InvalidManifest(format!("file placement needs a file destination: {:?}", item.dest)));
        }
        if (src_glob || src_dir) && !(item.dest.is_empty() || item.dest.ends_with('/')) {
            return Err(LipError::InvalidManifest(format!("directory placement needs a directory destination: {:?}", item.dest)));
        }
    }
    Ok(())
}

pub fn valid_id(id: &str) -> bool {
    if id.is_empty() || id.starts_with('/') || id.ends_with('/') || id.contains("..") || id.contains('\\') {
        return false;
    }
    let parts: Vec<&str> = id.split('/').collect();
    parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || b".-_~".contains(&byte))
        })
}

pub fn valid_variant(variant: &str) -> bool {
    if variant.is_empty() {
        return true;
    }
    let parts: Vec<&str> = variant.split('/').collect();
    !parts.is_empty()
        && parts.len() <= 2
        && parts.iter().all(|part| {
            !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
}

pub fn valid_relative_path(path: &str) -> bool {
    if path.is_empty() {
        return true;
    }
    if path.starts_with('/') || path.contains('\\') || path.contains("//") {
        return false;
    }
    if path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':' {
        return false;
    }
    path.split('/').all(|segment| segment != "..")
}

pub fn valid_glob(pattern: &str) -> bool {
    !pattern.is_empty() && !pattern.starts_with('/') && !pattern.contains("//") && !pattern.contains('\\')
}

/// 拆依赖键：<id> 或 <id>#<variant>。
pub fn split_dependency(key: &str) -> (&str, &str) {
    match key.split_once('#') {
        Some((id, variant)) => (id, variant),
        None => (key, ""),
    }
}

/// 把 legacy tooth.json 转成 format 4 包文件。无法表示时返回 None。
pub fn from_tooth_json(json: &Json, environment: &Environment) -> Result<Option<PackageFile>, LipError> {
    let Some(object) = json.as_object() else { return Ok(None) };
    let version = match object.get("format_version").and_then(Json::as_u64) {
        Some(1..=3) => object["format_version"].as_u64().unwrap(),
        _ => return Ok(None),
    };
    let tooth = object.get("tooth").and_then(Json::as_str).unwrap_or_default().to_string();
    if tooth.is_empty() {
        return Ok(None);
    }
    let package_version = object.get("version").and_then(Json::as_str).unwrap_or_default().to_string();
    let info = object.get("info").or_else(|| object.get("information"));
    let info = Info {
        name: info.and_then(|value| value.get("name")).and_then(Json::as_str).unwrap_or_default().to_string(),
        description: info.and_then(|value| value.get("description")).and_then(Json::as_str).unwrap_or_default().to_string(),
        tags: info.and_then(|value| value.get("tags")).and_then(Json::as_array).map(|items| items.iter().filter_map(Json::as_str).map(str::to_string).collect()).unwrap_or_default(),
        logo: info
            .and_then(|value| value.get("avatar_url"))
            .and_then(Json::as_str)
            .filter(|url| url.starts_with("https://"))
            .map(str::to_string),
    };

    let mut variants: BTreeMap<String, Vec<Entry>> = BTreeMap::new();

    if version == 3 {
        let Some(list) = object.get("variants").and_then(Json::as_array) else { return Ok(None) };
        for variant in list {
            let label = variant.get("label").and_then(Json::as_str).unwrap_or_default().to_string();
            let platform = variant.get("platform").and_then(Json::as_str).unwrap_or_default();
            if !platform.is_empty() && !rid_matches(platform, &environment.platform) {
                continue;
            }
            // 每个 variant 必须是独立条目：tooth v3 的 variants 是并列的变体，
            // 跨迭代复用同一份 Entry 会把服务端条目的依赖 / 资产并进客户端条目。
            let mut entry = Entry { side: environment.side.clone(), platform: environment.platform.clone(), ..Default::default() };
            if let Some(deps) = variant.get("dependencies").and_then(Json::as_object) {
                for (key, range) in deps {
                    if let Some(range) = range.as_str() {
                        entry.deps.insert(key.clone(), range.to_string());
                    }
                }
            }
            if let Some(assets) = variant.get("assets").and_then(Json::as_array) {
                for asset in assets {
                    let kind = asset.get("type").and_then(Json::as_str).unwrap_or_default();
                    if !matches!(kind, "self" | "zip") {
                        // tar / tgz / uncompressed 无法表示。
                        continue;
                    }
                    let urls: Vec<String> = asset.get("urls").and_then(Json::as_array).map(|items| items.iter().filter_map(Json::as_str).map(str::to_string).collect()).unwrap_or_default();
                    let place = variant_placements(asset.get("placements"));
                    if kind == "self" {
                        entry.assets.push(Asset::SelfArchive { place });
                    } else if !urls.is_empty() {
                        entry.assets.push(Asset::Zip { urls, place });
                    }
                }
            }
            if let Some(items) = variant.get("preserve_files").and_then(Json::as_array) {
                for item in items.iter().filter_map(Json::as_str) {
                    let pattern = item.replace('\\', "/");
                    let pattern = if pattern.contains('/') { pattern } else { format!("**/{pattern}") };
                    if valid_glob(&pattern) {
                        entry.preserve.push(pattern);
                    }
                }
            }
            if let Some(items) = variant.get("remove_files").and_then(Json::as_array) {
                for item in items.iter().filter_map(Json::as_str) {
                    let pattern = item.replace('\\', "/");
                    if valid_glob(&pattern) {
                        entry.purge.push(pattern.clone());
                        if !pattern.ends_with('/') {
                            entry.purge.push(format!("{pattern}/"));
                        }
                    }
                }
            }
            if let Some(scripts) = variant.get("scripts").and_then(Json::as_object) {
                let list = |name: &str| -> Vec<String> {
                    scripts.get(name).and_then(Json::as_array).map(|items| items.iter().filter_map(Json::as_str).map(str::to_string).collect()).unwrap_or_default()
                };
                entry.hooks.pre_install = list("pre_install");
                entry.hooks.post_install = [list("install"), list("post_install")].concat();
                entry.hooks.pre_uninstall = [list("pre_uninstall"), list("uninstall")].concat();
                entry.hooks.post_uninstall = list("post_uninstall");
            }
            variants.entry(label).or_default().push(entry);
        }
    } else {
        let mut entry = Entry { side: environment.side.clone(), platform: environment.platform.clone(), ..Default::default() };
        // v1 / v2：单一条目。
        let deps = object
            .get("dependencies")
            .or_else(|| object.get("prerequisites"))
            .and_then(Json::as_object)
            .map(|deps| {
                deps.iter()
                    .filter_map(|(key, range)| match range {
                        Json::String(range) => Some((key.clone(), range.clone())),
                        Json::Array(items) => items.iter().find_map(|item| match item {
                            Json::String(range) => Some((key.clone(), range.clone())),
                            Json::Array(nested) => nested.first().and_then(Json::as_str).map(|range| (key.clone(), range.to_string())),
                            _ => None,
                        }),
                        _ => None,
                    })
                    .collect::<BTreeMap<String, String>>()
            })
            .unwrap_or_default();
        entry.deps = deps;
        if let Some(url) = object.get("asset_url").and_then(Json::as_str).filter(|url| url.starts_with("https://")) {
            let place = object.get("files").and_then(|files| files.get("place")).map(|place| variant_placements(Some(place))).unwrap_or_default();
            entry.assets.push(Asset::Zip { urls: vec![url.replace("$(version)", &package_version).replace("{{version}}", &package_version)], place });
        }
        if let Some(placements) = object.get("placement").and_then(Json::as_array) {
            let converted: Vec<Place> = placements
                .iter()
                .filter_map(|item| {
                    let src = item.get("source").and_then(Json::as_str)?.replace('\\', "/");
                    let dest = item.get("destination").and_then(Json::as_str)?.replace('\\', "/");
                    Some(Place { src, dest })
                })
                .collect();
            if !converted.is_empty() {
                entry.assets.push(Asset::SelfArchive { place: converted });
            }
        }
        if let Some(items) = object.get("files").and_then(|files| files.get("preserve")).and_then(Json::as_array) {
            for item in items.iter().filter_map(Json::as_str) {
                let pattern = item.replace('\\', "/");
                let pattern = if pattern.contains('/') { pattern } else { format!("**/{pattern}") };
                if valid_glob(&pattern) {
                    entry.preserve.push(pattern);
                }
            }
        }
        if let Some(items) = object.get("possession").and_then(Json::as_array) {
            for item in items.iter().filter_map(Json::as_str) {
                let pattern = item.replace('\\', "/");
                if valid_glob(&pattern) {
                    entry.purge.push(pattern);
                }
            }
        }
        variants.insert(String::new(), vec![entry]);
    }

    if variants.is_empty() || variants.values().all(|entries| entries.is_empty()) {
        return Ok(None);
    }
    let file = PackageFile { format: 4, id: tooth, version: package_version, info, variants };
    let json = serde_json::to_value(&file).map_err(LipError::Json)?;
    validate(&file, &json)?;
    Ok(Some(file))
}

fn variant_placements(value: Option<&Json>) -> Vec<Place> {
    let Some(items) = value.and_then(Json::as_array) else { return Vec::new() };
    items
        .iter()
        .filter_map(|item| {
            let kind = item.get("type").and_then(Json::as_str).unwrap_or("file");
            let src = item.get("src").and_then(Json::as_str)?.replace('\\', "/");
            let dest = item.get("dest").and_then(Json::as_str)?.replace('\\', "/");
            let src = normalize_segments(&src);
            let dest = normalize_segments(&dest);
            let src_glob = src.contains(['*', '?', '[']);
            let (src, dest) = match kind {
                "dir" => (ensure_trailing(&src), ensure_trailing(&dest)),
                _ if src_glob => (src, ensure_trailing(&dest)),
                _ => (src, dest),
            };
            if !valid_relative_path(&src) || !valid_relative_path(&dest) {
                return None;
            }
            Some(Place { src, dest })
        })
        .collect()
}

fn normalize_segments(path: &str) -> String {
    path.split('/').filter(|segment| !segment.is_empty() && *segment != ".").collect::<Vec<_>>().join("/")
}

fn ensure_trailing(path: &str) -> String {
    if path.is_empty() || path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    }
}

/// tooth 的 RID 模式：* 匹配任意片段。
fn rid_matches(pattern: &str, platform: &str) -> bool {
    super::archive::wildcard_match(pattern, platform) || pattern == platform
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
format = 4
id = "github.com/example/demo"
version = "1.2.3"

[info]
name = "Demo"

[variants.client]
[[variants.client.entries]]
"#;

    #[test]
    fn rejects_legacy_sample_and_parses_minimal() {
        assert!(parse_package_file(SAMPLE).is_err());
    }

    #[test]
    fn renders_templates_and_converts_to_json() {
        let text = r#"
format = 4
id = "github.com/example/demo"
version = "1.2.3"

[[variants."".entries]]
"#;
        let _ = text;
        let toml = "format = 4\nid = \"github.com/example/demo\"\nversion = \"1.2.3\"\n[[variants.\"\"]]\nside = \"client\"\nplatform = \"win-x64\"\n[[variants.\"\".assets]]\ntype = \"zip\"\nurls = [\"https://example.com/v{{version}}/a.zip\"]\n";
        let (file, json) = parse_package_file(toml).unwrap();
        assert_eq!(file.version().unwrap(), semver::Version::parse("1.2.3").unwrap());
        assert_eq!(json["variants"][""][0]["assets"][0]["urls"][0], "https://example.com/v1.2.3/a.zip");
    }

    /// tooth v3 的 variants 是并列变体：无 label（服务端）与 label=client 必须各成一条，
    /// 客户端条目不得继承服务端依赖 / 资产 / 落点（否则客户端实例会被装进 BDS）。
    #[test]
    fn tooth_variants_do_not_share_entries() {
        let tooth = serde_json::json!({
            "format_version": 3,
            "tooth": "github.com/example/loader",
            "version": "1.0.0",
            "information": { "name": "Loader" },
            "variants": [
                {
                    "platform": "win-x64",
                    "dependencies": { "github.com/example/bds": "1.0.*" },
                    "assets": [{ "type": "zip", "urls": ["https://example.com/server.zip"] }],
                    "placements": [{ "type": "dir", "src": "Server/", "dest": "plugins/Server/" }],
                    "scripts": { "install": ["server-hook.exe"] }
                },
                {
                    "label": "client",
                    "platform": "win-x64",
                    "dependencies": { "github.com/example/runtime#client": "1.0.*" },
                    "assets": [{ "type": "zip", "urls": ["https://example.com/client.zip"] }],
                    "placements": [{ "type": "dir", "src": "Client/", "dest": "mods/Client/" }],
                    "scripts": { "install": ["client-hook.exe"] }
                }
            ]
        });
        let environment = Environment::new("client", "win-x64");
        let file = from_tooth_json(&tooth, &environment).unwrap().unwrap();
        let server = file.entry("", &environment).expect("server entry");
        let client = file.entry("client", &environment).expect("client entry");
        assert!(server.deps.contains_key("github.com/example/bds"));
        assert!(!client.deps.contains_key("github.com/example/bds"));
        assert!(client.deps.contains_key("github.com/example/runtime#client"));
        assert_eq!(client.assets.len(), 1);
        assert_eq!(client.purge, server.purge);
        assert_eq!(client.hooks.post_install, vec!["client-hook.exe".to_string()]);
        assert_eq!(server.hooks.post_install, vec!["server-hook.exe".to_string()]);
        let client_urls = match &client.assets[0] {
            Asset::Zip { urls, .. } => urls.clone(),
            other => panic!("unexpected asset: {other:?}"),
        };
        assert_eq!(client_urls, vec!["https://example.com/client.zip".to_string()]);
    }

    /// GMLIB 这类模组的 tooth v3 只有一个无 label 的变体，而 lipr 索引把它标注成 client。
    /// 请求 client 时必须回退到空 label 变体，否则实机上会直接报「no client entry」。
    #[test]
    fn tooth_without_label_falls_back_to_default_variant() {
        let tooth = serde_json::json!({
            "format_version": 3,
            "tooth": "github.com/GroupMountain/GMLIB-Release",
            "version": "26.51.3",
            "variants": [{
                "platform": "win-x64",
                "dependencies": { "github.com/LiteLDev/LeviLamina": ">=26.51.0" },
                "assets": [{
                    "type": "self",
                    "urls": [],
                    "placements": [{ "type": "dir", "src": "bin/DLL/GMLIB", "dest": "plugins/GMLIB/" }]
                }],
                "remove_files": ["plugins/GMLIB"]
            }]
        });
        let environment = Environment::new("client", "win-x64");
        let file = from_tooth_json(&tooth, &environment).unwrap().unwrap();
        let entry = file.entry("client", &environment).expect("fallback to empty label");
        assert_eq!(entry.assets.len(), 1);
        assert!(matches!(&entry.assets[0], Asset::SelfArchive { .. }));
        assert!(entry.deps.contains_key("github.com/LiteLDev/LeviLamina"));
        // 完全不存在的变体同样回退，且不会跨到别的 label。
        assert!(file.entry("server", &environment).is_some());
    }
}
