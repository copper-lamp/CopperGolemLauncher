//! 依赖求解与安装计划。
//!
//! 语义对齐 protocol/lip-install.md：请求汇合、依赖范围取交集、已安装版本优先、
//! 依赖在前安装、更新等于卸载后安装。求解只读取远端清单，不改工作区。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use semver::Version;

use super::manifest::Entry;
use super::range::VersionRange;
use super::remote::{RemoteClient, RemotePackage};
use super::source::{GitRef, PackageSource, Source};
use super::{Cancel, Environment, InstalledPackage, LipError, State};

/// 一个待安装的包（已带解析结果）。
pub struct InstallItem {
    pub identity: String,
    pub variant: String,
    pub source: PackageSource,
    pub package: RemotePackage,
    pub entry: Entry,
    pub explicit: bool,
    /// 工作区中已有同 identity 的旧版本，需要先卸载。
    pub replaces: bool,
}

/// 保留下来的已安装包（可能只改 explicit 标记）。
pub struct KeptItem {
    pub identity: String,
    pub package: InstalledPackage,
    pub explicit: bool,
}

pub struct Plan {
    pub installs: Vec<InstallItem>,
    pub removals: Vec<InstalledPackage>,
    pub kept: Vec<KeptItem>,
}

struct Requirement {
    source: PackageSource,
    explicit: bool,
    ranges: BTreeSet<(String, String)>,
}

struct Selection {
    identity: String,
    variant: String,
    source: PackageSource,
    package: RemotePackage,
    entry: Entry,
    explicit: bool,
}

impl Plan {
    pub fn final_count(&self) -> usize {
        self.installs.len() + self.kept.len()
    }
}

/// 计算 install / update 计划。refresh=true 时忽略已安装版本优先，重新取最新。
///
/// dependencies=false 时只安装请求里点名的包：不展开依赖闭包、不把状态里其它
/// 显式包当成根重新求解、也不卸载它们。内容下载「装这个模组」走这条路径——
/// 用户点一个模组就只该拿到该模组的文件，而不是顺带重装 LeviLamina 的运行时依赖。
pub async fn plan_install(
    client: &RemoteClient,
    environment: &Environment,
    state: &State,
    requests: &[PackageSource],
    refresh: bool,
    dependencies: bool,
    cancel: &Cancel,
    sink: &super::Sink,
) -> Result<Plan, LipError> {
    let installed: HashMap<String, InstalledPackage> =
        state.packages.iter().map(|package| (identity(&package.manifest.id, &package.variant).to_ascii_lowercase(), package.clone())).collect();

    // 请求集：本次显式请求 + 状态里其它显式包（install 不应顺手删掉它们）。
    let mut roots: Vec<PackageSource> = requests.to_vec();
    for package in state.packages.iter().filter(|_| dependencies) {
        if !package.explicit {
            continue;
        }
        let Ok(source) = PackageSource::parse(&package.source_ref) else { continue };
        if requests.iter().any(|request| request.identity() == source.identity()) {
            continue;
        }
        roots.push(source);
    }
    if roots.is_empty() {
        return Ok(Plan { installs: Vec::new(), removals: Vec::new(), kept: Vec::new() });
    }

    let mut requirements: BTreeMap<String, Requirement> = BTreeMap::new();
    for source in &roots {
        let key = source.identity();
        let entry = requirements.entry(key).or_insert_with(|| Requirement { source: source.clone(), explicit: true, ranges: BTreeSet::new() });
        entry.explicit = true;
        if let Some(GitRef::Version(version)) = &source.reference {
            entry.ranges.insert(("(requested)".into(), version.to_string()));
        }
    }

    let mut selections: BTreeMap<String, Selection> = BTreeMap::new();
    let mut previous: BTreeMap<String, String> = BTreeMap::new();

    for _round in 0..64 {
        // 先把依赖闭包扩到不动点（依赖的依赖）。
        for _expand in 0..(if dependencies { 64 } else { 0 }) {
            let mut added = false;
            let snapshot: Vec<(String, BTreeMap<String, String>)> = selections
                .iter()
                .map(|(key, selection)| (key.clone(), selection.entry.deps.clone()))
                .collect();
            for (dependant, deps) in snapshot {
                for (key, range) in deps {
                    let (id, variant) = super::manifest::split_dependency(&key);
                    let dep_identity = identity(id, variant);
                    let dep_key = dep_identity.to_ascii_lowercase();
                    let source = source_for(id, variant, &dep_key, &requirements, &installed)?;
                    let entry = requirements.entry(dep_key).or_insert_with(|| Requirement {
                        source: source.clone(),
                        explicit: false,
                        ranges: BTreeSet::new(),
                    });
                    if entry.ranges.insert((dependant.clone(), range)) {
                        added = true;
                    }
                }
            }
            if !added {
                break;
            }
        }

        // 为每个需求选版本。
        let keys: Vec<String> = requirements.keys().cloned().collect();
        let mut changed = false;
        for key in keys {
            let requirement = &requirements[&key];
            let ranges: Vec<VersionRange> = requirement
                .ranges
                .iter()
                .map(|(_, range)| VersionRange::parse(range))
                .collect::<Result<_, _>>()?;
            let source = requirement.source.clone();
            let selected = select(client, environment, &installed, &key, &source, &ranges, requirement.explicit, refresh, cancel, sink).await?;
            let signature = format!("{}@{}", selected.package.file.version, selected.package.git_ref.clone().unwrap_or_default());
            if previous.get(&key) != Some(&signature) {
                previous.insert(key.clone(), signature);
                selections.insert(key.clone(), selected);
                changed = true;
            } else if let Some(existing) = selections.get_mut(&key) {
                existing.explicit = requirement.explicit;
            }
        }
        if !changed {
            break;
        }
    }

    // 只保留显式包的可达闭包。
    let mut reachable: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = if dependencies {
        requirements.iter().filter(|(_, requirement)| requirement.explicit).map(|(key, _)| key.clone()).collect()
    } else {
        // 只认请求本身：依赖不进入计划，也就不会触发依赖的远端查询与下载。
        requirements.keys().cloned().collect()
    };
    while let Some(key) = queue.pop() {
        if !reachable.insert(key.clone()) {
            continue;
        }
        if let Some(selection) = selections.get(&key) {
            for dep in selection.entry.deps.keys() {
                let (id, variant) = super::manifest::split_dependency(dep);
                queue.push(identity(id, variant).to_ascii_lowercase());
            }
        }
    }

    let mut final_selections: Vec<&Selection> = selections.iter().filter(|(key, _)| reachable.contains(*key)).map(|(_, selection)| selection).collect();
    sort_installs(&mut final_selections);

    let mut installs = Vec::new();
    let mut kept = Vec::new();
    for selection in final_selections {
        let existing = installed.get(&selection.identity.to_ascii_lowercase());
        let keep_existing = existing.filter(|package| {
            !refresh
                && package.manifest.version == selection.package.file.version
                && package.commit == selection.package.git_ref
                && package.variant == selection.variant
        });
        match keep_existing {
            Some(package) => kept.push(KeptItem { identity: selection.identity.clone(), package: package.clone(), explicit: selection.explicit }),
            None => installs.push(InstallItem {
                identity: selection.identity.clone(),
                variant: selection.variant.clone(),
                source: selection.source.clone(),
                package: RemotePackage {
                    file: selection.package.file.clone(),
                    json: selection.package.json.clone(),
                    git_ref: selection.package.git_ref.clone(),
                    archive: None,
                },
                entry: selection.entry.clone(),
                explicit: selection.explicit,
                replaces: existing.is_some(),
            }),
        }
    }

    if !dependencies {
        // 只装请求的包：未参与本次解析的已安装包原样保留，既不改状态也不删文件。
        for package in &state.packages {
            let key = identity(&package.manifest.id, &package.variant).to_ascii_lowercase();
            if installs.iter().any(|item| item.identity.to_ascii_lowercase() == key)
                || kept.iter().any(|item| item.identity.to_ascii_lowercase() == key)
            {
                continue;
            }
            kept.push(KeptItem {
                identity: identity(&package.manifest.id, &package.variant),
                package: package.clone(),
                explicit: package.explicit,
            });
        }
    }

    // 被新版本替换的旧包先卸载。
    let keep_keys: HashSet<String> = kept.iter().map(|item| item.identity.to_ascii_lowercase()).collect();
    let mut removals: Vec<InstalledPackage> = state
        .packages
        .iter()
        .filter(|package| {
            let key = identity(&package.manifest.id, &package.variant).to_ascii_lowercase();
            if keep_keys.contains(&key) {
                return false;
            }
            // 关闭依赖求解时，只有被同名新版本替换的包才卸载。
            dependencies || installs.iter().any(|item| item.identity.to_ascii_lowercase() == key)
        })
        .cloned()
        .collect();
    sort_removals(&mut removals);

    Ok(Plan { installs, removals, kept })
}

#[allow(clippy::too_many_arguments)]
async fn select(
    client: &RemoteClient,
    environment: &Environment,
    installed: &HashMap<String, InstalledPackage>,
    key: &str,
    source: &PackageSource,
    ranges: &[VersionRange],
    explicit: bool,
    refresh: bool,
    cancel: &Cancel,
    sink: &super::Sink,
) -> Result<Selection, LipError> {
    let pinned = match &source.reference {
        Some(GitRef::Version(version)) => Some(version.clone()),
        _ => None,
    };

    // 已安装版本优先：满足所有范围就不再联网。
    if let Some(package) = installed.get(key) {
        if !refresh {
            if let Ok(version) = Version::parse(&package.manifest.version) {
                if ranges.iter().all(|range| range.matches(&version)) && pinned.as_ref().map(|pin| pin == &version).unwrap_or(true) {
                    let (result, entry) = selection_from_installed(package, environment, source)?;
                    sink.log(format!("保留已安装的 {key} {}", package.manifest.version));
                    return Ok(Selection { identity: identity(&result.file.id, &package.variant), variant: package.variant.clone(), source: source.clone(), package: result, entry, explicit });
                }
            }
        }
    }

    let chosen = match &pinned {
        Some(version) => version.clone(),
        None => {
            sink.log(format!("解析 {key} 的可用版本"));
            // 解析阶段没有字节进度，但必须把「在做什么」推到界面上，否则网络慢时
            // 用户只能看到一个不动的转圈。
            sink.progress(&format!("解析 {key} 的可用版本"), None);
            let versions = client.versions(source, cancel).await?;
            ranges
                .iter()
                .fold(Some(versions), |candidates, range| {
                    candidates.map(|versions| versions.into_iter().filter(|version| range.matches(version)).collect())
                })
                .and_then(|versions: Vec<Version>| versions.into_iter().max())
                .ok_or_else(|| conflict(key, ranges))?
        }
    };

    let ok = ranges.iter().all(|range| range.matches(&chosen));
    if !ok {
        return Err(conflict(key, ranges));
    }

    let mut request = source.clone();
    request.reference = Some(GitRef::Version(chosen.clone()));
    sink.progress(&format!("获取 {key} {chosen} 的清单"), None);
    // 解析阶段也会走网络（ls-refs / 文件树 / 清单 blob），把字节进度接到界面上，
    // 否则慢网下用户只看到一行静止的「正在安装」。
    let reporter = |done: u64, total: Option<u64>| {
        let text = match total {
            Some(total) if total > 0 => format!("获取 {key} {chosen} 清单 {:.1}/{:.1} MB", done as f64 / 1048576.0, total as f64 / 1048576.0),
            _ => format!("获取 {key} {chosen} 清单 {:.1} MB", done as f64 / 1048576.0),
        };
        sink.progress(&text, None);
    };
    let package = client.fetch_package(&request, environment, cancel, Some(&reporter)).await?;
    let variant = if request.variant.is_empty() { String::new() } else { request.variant.clone() };
    let entry = package.file.entry(&variant, environment).cloned().ok_or_else(|| {
        LipError::PackageNotFound(format!("{key} {} has no {} entry for {}", package.file.version, environment.side, environment.platform))
    })?;
    Ok(Selection {
        identity: identity(&package.file.id, &variant),
        variant,
        source: request,
        package,
        entry,
        explicit,
    })
}

fn selection_from_installed(package: &InstalledPackage, environment: &Environment, _source: &PackageSource) -> Result<(RemotePackage, Entry), LipError> {
    let variant = package.variant.clone();
    // 严禁回退到「任意一个条目」：tooth 里服务端与客户端是并列变体，
    // 拿错条目会把服务端依赖/落点带进客户端实例（客户端模组装出 BDS 的根因之一）。
    let entry = package
        .manifest
        .entry(&variant, environment)
        .cloned()
        .ok_or_else(|| LipError::InvalidState(format!(
            "installed package {}#{} has no {} entry for {}",
            package.manifest.id, variant, environment.side, environment.platform
        )))?;
    let json = serde_json::to_value(&package.manifest)?;
    Ok((RemotePackage { file: package.manifest.clone(), json, git_ref: package.commit.clone(), archive: None }, entry))
}

fn conflict(key: &str, ranges: &[VersionRange]) -> LipError {
    let text = ranges.iter().map(|range| range.as_str()).collect::<Vec<_>>().join(", ");
    LipError::Conflict(format!("no version of {key} satisfies {text}"))
}

fn source_for(id: &str, variant: &str, key: &str, requirements: &BTreeMap<String, Requirement>, installed: &HashMap<String, InstalledPackage>) -> Result<PackageSource, LipError> {
    if let Some(requirement) = requirements.get(key) {
        return Ok(requirement.source.clone());
    }
    if let Some(package) = installed.get(key) {
        if let Ok(mut source) = PackageSource::parse(&package.source_ref) {
            if source.variant.is_empty() {
                source.variant = variant.to_string();
            }
            return Ok(source);
        }
    }
    let Some(rest) = id.strip_prefix("github.com/") else {
        return Err(LipError::Unsupported(format!("dependency {id} is not a GitHub package")));
    };
    let mut parts = rest.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(LipError::InvalidReference(format!("invalid dependency id {id}")));
    };
    Ok(PackageSource {
        source: Source::Github { owner: owner.to_string(), repo: repo.to_string() },
        variant: variant.to_string(),
        reference: None,
    })
}

pub fn identity(id: &str, variant: &str) -> String {
    if variant.is_empty() {
        id.to_string()
    } else {
        format!("{id}#{variant}")
    }
}

/// 依赖在前；同层按 identity 排序；环按 identity 升序追加。
fn sort_installs(items: &mut [&Selection]) {
    let keys: HashSet<String> = items.iter().map(|item| item.identity.to_ascii_lowercase()).collect();
    let mut edges: HashMap<String, BTreeSet<String>> = HashMap::new(); // dep -> dependants
    let mut indegree: HashMap<String, usize> = HashMap::new();
    for item in items.iter() {
        let key = item.identity.to_ascii_lowercase();
        indegree.entry(key.clone()).or_insert(0);
        for dep in item.entry.deps.keys() {
            let (id, variant) = super::manifest::split_dependency(dep);
            let dep_key = identity(id, variant).to_ascii_lowercase();
            if dep_key == key || !keys.contains(&dep_key) {
                continue;
            }
            if edges.entry(dep_key).or_default().insert(key.clone()) {
                *indegree.entry(key.clone()).or_insert(0) += 1;
            }
        }
    }
    let mut ready: BTreeSet<String> = indegree.iter().filter(|(_, degree)| **degree == 0).map(|(key, _)| key.clone()).collect();
    let mut ordered: Vec<String> = Vec::new();
    let mut remaining = indegree.clone();
    while let Some(key) = ready.iter().next().cloned() {
        ready.remove(&key);
        if remaining.remove(&key).is_none() {
            continue;
        }
        ordered.push(key.clone());
        if let Some(dependants) = edges.get(&key) {
            for dependant in dependants {
                if let Some(degree) = remaining.get_mut(dependant) {
                    *degree -= 1;
                    if *degree == 0 {
                        ready.insert(dependant.clone());
                    }
                }
            }
        }
    }
    // 环：按 identity 追加。
    for item in items.iter() {
        let key = item.identity.to_ascii_lowercase();
        if !ordered.contains(&key) {
            ordered.push(key);
        }
    }
    items.sort_by_key(|item| ordered.iter().position(|key| key == &item.identity.to_ascii_lowercase()).unwrap_or(usize::MAX));
}

pub fn sort_removals(packages: &mut Vec<InstalledPackage>) {
    packages.sort_by(|a, b| {
        let a_key = identity(&a.manifest.id, &a.variant).to_ascii_lowercase();
        let b_key = identity(&b.manifest.id, &b.variant).to_ascii_lowercase();
        let a_depends_on_b = a.manifest.variants.values().flatten().any(|entry| entry.deps.keys().any(|key| identity_of_key(key) == b_key));
        let b_depends_on_a = b.manifest.variants.values().flatten().any(|entry| entry.deps.keys().any(|key| identity_of_key(key) == a_key));
        match (a_depends_on_b, b_depends_on_a) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a_key.cmp(&b_key),
        }
    });
}

fn identity_of_key(key: &str) -> String {
    let (id, variant) = super::manifest::split_dependency(key);
    identity(id, variant).to_ascii_lowercase()
}

/// 卸载计划：默认同时移除因此不再被需要的隐式依赖。
pub fn plan_uninstall(state: &State, targets: &[String], no_dependencies: bool) -> Result<Plan, LipError> {
    let mut wanted: HashSet<String> = targets.iter().map(|target| target.to_ascii_lowercase()).collect();
    // 允许只写 id：展开到具体 variant。
    let mut exact: HashSet<String> = HashSet::new();
    for package in &state.packages {
        let key = identity(&package.manifest.id, &package.variant).to_ascii_lowercase();
        if wanted.contains(&key) || wanted.contains(&package.manifest.id.to_ascii_lowercase()) {
            exact.insert(key);
        }
    }
    if exact.is_empty() {
        return Err(LipError::PackageNotFound(targets.join(", ")));
    }
    wanted = exact;

    if !no_dependencies {
        // 级联移除依赖被卸载包的包。
        loop {
            let mut added = false;
            for package in &state.packages {
                let key = identity(&package.manifest.id, &package.variant).to_ascii_lowercase();
                if wanted.contains(&key) {
                    continue;
                }
                let depends = package.manifest.variants.values().flatten().any(|entry| entry.deps.keys().any(|dep| wanted.contains(&identity_of_key(dep))));
                if depends {
                    wanted.insert(key);
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
    }

    let removals: Vec<InstalledPackage> = state
        .packages
        .iter()
        .filter(|package| wanted.contains(&identity(&package.manifest.id, &package.variant).to_ascii_lowercase()))
        .cloned()
        .collect();

    // 剩余包中丢弃不再可达的隐式包。
    let mut kept: Vec<KeptItem> = state
        .packages
        .iter()
        .filter(|package| !wanted.contains(&identity(&package.manifest.id, &package.variant).to_ascii_lowercase()))
        .map(|package| KeptItem { identity: identity(&package.manifest.id, &package.variant), package: package.clone(), explicit: package.explicit })
        .collect();

    if !no_dependencies {
        let explicit_keys: HashSet<String> = kept.iter().filter(|item| item.explicit).map(|item| item.identity.to_ascii_lowercase()).collect();
        let mut reachable: HashSet<String> = HashSet::new();
        let mut queue: Vec<String> = explicit_keys.iter().cloned().collect();
        while let Some(key) = queue.pop() {
            if !reachable.insert(key.clone()) {
                continue;
            }
            if let Some(item) = kept.iter().find(|item| item.identity.to_ascii_lowercase() == key) {
                for dep in item.package.manifest.variants.values().flatten().flat_map(|entry| entry.deps.keys()) {
                    queue.push(identity_of_key(dep));
                }
            }
        }
        kept.retain(|item| reachable.contains(&item.identity.to_ascii_lowercase()));
    }

    Ok(Plan { installs: Vec::new(), removals, kept })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(id: &str, version: &str, deps: &[(&str, &str)]) -> InstalledPackage {
        let mut entry = Entry { side: "client".into(), platform: "win-x64".into(), ..Default::default() };
        for (key, range) in deps {
            entry.deps.insert((*key).into(), (*range).into());
        }
        InstalledPackage {
            manifest: super::super::manifest::PackageFile {
                format: 4,
                id: id.into(),
                version: version.into(),
                info: Default::default(),
                variants: BTreeMap::from([("".to_string(), vec![entry])]),
            },
            variant: String::new(),
            explicit: true,
            source_ref: id.into(),
            commit: None,
            files: Vec::new(),
        }
    }

    #[test]
    fn uninstall_cascades_dependants_and_prunes_implicit() {
        let state = State {
            format: 4,
            side: "client".into(),
            platform: "win-x64".into(),
            packages: vec![package("github.com/a/core", "1.0.0", &[]), package("github.com/b/mod", "1.0.0", &[("github.com/a/core", "1.0.0")])],
        };
        let plan = plan_uninstall(&state, &["github.com/a/core".into()], false).unwrap();
        assert_eq!(plan.removals.len(), 2);
        assert!(plan.kept.is_empty());
    }

    #[test]
    fn uninstall_keeps_others_explicit_packages() {
        let state = State {
            format: 4,
            side: "client".into(),
            platform: "win-x64".into(),
            packages: vec![package("github.com/a/core", "1.0.0", &[]), package("github.com/b/mod", "1.0.0", &[])],
        };
        let plan = plan_uninstall(&state, &["github.com/b/mod".into()], false).unwrap();
        assert_eq!(plan.removals.len(), 1);
        assert_eq!(plan.kept.len(), 1);
    }
}
