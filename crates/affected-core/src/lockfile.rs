//! Map changed npm install locations back to the dependencies that each importer actually loads.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::manifest::dependency_node;
use crate::source::Source;

#[derive(Default)]
pub(crate) struct Impact {
    pub broad: BTreeSet<PathBuf>,
    /// Pairs of (dependency node, changed lockfile).
    pub dependencies: BTreeSet<(PathBuf, PathBuf)>,
}

type Packages = Map<String, Value>;

fn packages(source: &impl Source, path: &Path) -> Option<Packages> {
    let value: Value = serde_json::from_slice(&source.read(path).ok()?).ok()?;
    if !matches!(value.get("lockfileVersion")?.as_u64()?, 2 | 3) {
        return None;
    }
    let packages = value.get("packages")?.as_object()?;
    packages.get("")?.as_object()?;
    Some(packages.clone())
}

/// npm first looks beside the importer, then climbs to each enclosing node_modules.
fn installed(packages: &Packages, from: &str, name: &str) -> Option<String> {
    let name = Path::new(name);
    if name.is_absolute() || name.components().any(|p| !matches!(p, std::path::Component::Normal(_))) {
        return None;
    }
    for dir in Path::new(from).ancestors() {
        if dir.ends_with("node_modules") {
            continue;
        }
        let candidate = dir.join("node_modules").join(name).to_string_lossy().replace('\\', "/");
        if packages.contains_key(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn in_modules(path: &str) -> bool {
    path.split('/').any(|part| part == "node_modules")
}

fn consumers(packages: &Packages, other: &Packages, lock: &Path) -> Option<BTreeSet<PathBuf>> {
    let root = lock.parent()?;
    let mut parents: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut direct: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let mut changed = Vec::new();
    let mut nodes = BTreeSet::new();
    for (location, record) in packages {
        let record = record.as_object()?;
        if in_modules(location) && other.get(location).and_then(Value::as_object) != Some(record) {
            changed.push(location.clone());
        }
        if record.get("link").and_then(Value::as_bool) == Some(true) {
            let target = record.get("resolved")?.as_str()?;
            if !packages.contains_key(target) {
                return None;
            }
            parents.entry(target.to_owned()).or_default().push(location);
        }
        for section in ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"] {
            let Some(declared) = record.get(section) else { continue };
            if section == "devDependencies" && in_modules(location) {
                continue;
            }
            for (name, range) in declared.as_object()? {
                range.as_str()?;
                let Some(target) = installed(packages, location, name) else {
                    if section == "optionalDependencies"
                        || (section == "peerDependencies"
                            && record
                                .get("peerDependenciesMeta")
                                .and_then(|m| m.get(name))
                                .and_then(|v| v.get("optional"))
                                .and_then(Value::as_bool)
                                == Some(true))
                    {
                        continue;
                    }
                    return None;
                };
                parents.entry(target.clone()).or_default().push(location);
                if !in_modules(location) {
                    let node = dependency_node(&root.join(location).join("package.json"), name);
                    let prior = other.get(location).and_then(|r| r.get(section)).and_then(|v| v.get(name));
                    if prior != Some(range) || installed(other, location, name).as_ref() != Some(&target) {
                        nodes.insert(node.clone());
                        changed.push(location.clone());
                    }
                    direct.entry(target).or_default().push(node);
                }
            }
        }
    }
    let mut visited = BTreeSet::new();
    while let Some(location) = changed.pop() {
        if !visited.insert(location.clone()) {
            continue;
        }
        if let Some(users) = direct.get(&location) {
            nodes.extend(users.iter().cloned());
        }
        if let Some(users) = parents.get(&location) {
            changed.extend(users.iter().map(|p| (*p).to_owned()));
        }
    }
    Some(nodes)
}

pub(crate) fn compare(head: &impl Source, base: Option<&impl Source>, changed: &[PathBuf]) -> Impact {
    let mut result = Impact::default();
    for path in changed {
        match path.file_name().and_then(|p| p.to_str()) {
            Some("package-lock.json" | "npm-shrinkwrap.json") => {
                let comparison =
                    base.and_then(|b| packages(b, path)).zip(packages(head, path)).and_then(|(old, new)| {
                        let mut nodes = consumers(&old, &new, path)?;
                        nodes.extend(consumers(&new, &old, path)?);
                        Some(nodes)
                    });
                if let Some(nodes) = comparison {
                    result.dependencies.extend(nodes.into_iter().map(|n| (n, path.clone())));
                } else {
                    result.broad.insert(path.clone());
                }
            }
            Some("pnpm-lock.yaml" | "pnpm-lock.yml" | "yarn.lock" | "bun.lock" | "bun.lockb") => {
                result.broad.insert(path.clone());
            }
            _ => {}
        }
    }
    result
}
