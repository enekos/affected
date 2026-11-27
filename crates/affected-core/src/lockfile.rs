//! Map changed lockfile resolutions back to the dependencies that each importer actually loads.
//!
//! Each supported lockfile is read on both revisions. A dependency an importer declares is
//! changed when its own resolution differs, or when anything it transitively reaches does.
//! A lockfile that cannot be read or understood on either side selects every entry.
mod npm;
mod pnpm;
mod yaml;
mod yarn;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::manifest::dependency_node;
use crate::source::Source;

#[derive(Default)]
pub(crate) struct Impact {
    pub broad: BTreeSet<PathBuf>,
    /// Pairs of (dependency node, changed lockfile).
    pub dependencies: BTreeSet<(PathBuf, PathBuf)>,
}

/// A lockfile's resolution graph in the shape pnpm and Yarn Berry share.
#[derive(Debug, Default)]
struct Resolution {
    /// Top-level fields that may change any resolution; a difference selects every entry.
    settings: Value,
    /// Workspace directory relative to the lockfile, `""` for its own directory.
    importers: BTreeMap<String, Importer>,
    /// Package id to its recorded resolution.
    packages: BTreeMap<String, Package>,
}

#[derive(Debug, Default)]
struct Importer {
    /// Fields besides the dependencies; a difference counts as a change to every dependency.
    settings: Value,
    dependencies: BTreeMap<String, Dependency>,
}

#[derive(Debug, Default, PartialEq)]
struct Dependency {
    /// The declared range and the recorded resolution, as the lockfile writes them.
    declared: String,
    targets: Vec<Target>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Target {
    Package(String),
    /// A workspace package, by importer directory.
    Importer(String),
}

#[derive(Debug)]
struct Package {
    /// Everything recorded for the package; any difference is a change.
    record: Value,
    dependencies: Vec<Target>,
}

impl Resolution {
    /// The declared dependencies, as (importer, name), whose resolution here differs from
    /// `other` directly or through anything they reach. A changed workspace package also
    /// changes every dependency linking to it, as npm links do.
    fn changed_against<'a>(&'a self, other: &Self) -> BTreeSet<(&'a str, &'a str)> {
        let mut parents: BTreeMap<Target, Vec<Target>> = BTreeMap::new();
        let mut users: BTreeMap<&Target, Vec<(&str, &str)>> = BTreeMap::new();
        let mut changed = Vec::new();
        let mut direct = BTreeSet::new();
        for (id, package) in &self.packages {
            if other.packages.get(id).is_none_or(|prior| prior.record != package.record) {
                changed.push(Target::Package(id.clone()));
            }
            for target in &package.dependencies {
                parents.entry(target.clone()).or_default().push(Target::Package(id.clone()));
            }
        }
        for (dir, importer) in &self.importers {
            let prior = other.importers.get(dir);
            let rewired = prior.is_none_or(|prior| prior.settings != importer.settings);
            for (name, dependency) in &importer.dependencies {
                for target in &dependency.targets {
                    parents.entry(target.clone()).or_default().push(Target::Importer(dir.clone()));
                    users.entry(target).or_default().push((dir, name));
                }
                if rewired || prior.and_then(|prior| prior.dependencies.get(name)) != Some(dependency) {
                    direct.insert((dir.as_str(), name.as_str()));
                    changed.push(Target::Importer(dir.clone()));
                }
            }
        }
        let mut visited = BTreeSet::new();
        while let Some(target) = changed.pop() {
            if visited.contains(&target) {
                continue;
            }
            if let Some(found) = users.get(&target) {
                direct.extend(found.iter().copied());
            }
            if let Some(found) = parents.get(&target) {
                changed.extend(found.iter().filter(|parent| !visited.contains(*parent)).cloned());
            }
            visited.insert(target);
        }
        direct
    }
}

/// A workspace directory as the lockfile names it, `.` or a relative path inside the
/// lockfile's directory, normalised to `/`-separated segments with `""` for the root.
fn importer_dir(dir: &str) -> Option<String> {
    join_relative("", dir)
}

/// `relative` resolved against the lockfile-relative directory `base`; `None` when it
/// leaves the lockfile's directory or is absolute.
fn join_relative(base: &str, relative: &str) -> Option<String> {
    if relative.starts_with('/') || relative.contains('\\') || relative.contains(':') {
        return None;
    }
    let mut segments: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    Some(segments.join("/"))
}

fn manifest(root: &Path, dir: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(dir.split('/').filter(|s| !s.is_empty()));
    path.push("package.json");
    path
}

fn precise(
    head: &impl Source,
    base: &impl Source,
    path: &Path,
    parse: fn(&str) -> Option<Resolution>,
) -> Option<BTreeSet<PathBuf>> {
    let root = path.parent()?;
    let old = parse(&base.read_to_string(path).ok()?)?;
    let new = parse(&head.read_to_string(path).ok()?)?;
    if old.settings != new.settings {
        return None;
    }
    let mut changed = old.changed_against(&new);
    changed.extend(new.changed_against(&old));
    Some(changed.into_iter().map(|(dir, name)| dependency_node(&manifest(root, dir), name)).collect())
}

pub(crate) fn compare(head: &impl Source, base: Option<&impl Source>, changed: &[PathBuf]) -> Impact {
    let mut result = Impact::default();
    for path in changed {
        let comparison = match path.file_name().and_then(|p| p.to_str()) {
            Some("package-lock.json" | "npm-shrinkwrap.json") => base.and_then(|b| npm::changed(head, b, path)),
            Some("pnpm-lock.yaml" | "pnpm-lock.yml") => base.and_then(|b| precise(head, b, path, pnpm::resolution)),
            Some("yarn.lock") => base.and_then(|b| precise(head, b, path, yarn::resolution)),
            Some("bun.lock" | "bun.lockb") => None,
            _ => continue,
        };
        match comparison {
            Some(nodes) => result.dependencies.extend(nodes.into_iter().map(|n| (n, path.clone()))),
            None => {
                result.broad.insert(path.clone());
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_stay_inside_the_lockfile_directory() {
        assert_eq!(importer_dir(".").as_deref(), Some(""));
        assert_eq!(importer_dir("packages/a").as_deref(), Some("packages/a"));
        assert_eq!(join_relative("packages/a", "../b").as_deref(), Some("packages/b"));
        assert_eq!(join_relative("", "../outside"), None);
        assert_eq!(join_relative("", "/abs"), None);
        assert_eq!(join_relative("", "C:/abs"), None);
    }

    #[test]
    fn a_change_reaches_importers_through_packages_and_links() {
        let package = |record: &str, dependencies: Vec<Target>| Package { record: Value::from(record), dependencies };
        let dependency =
            |declared: &str, target: Target| Dependency { declared: declared.into(), targets: vec![target] };
        let build = |leaf: &str| Resolution {
            settings: Value::Null,
            importers: BTreeMap::from([
                (
                    "lib".to_owned(),
                    Importer {
                        settings: Value::Null,
                        dependencies: BTreeMap::from([
                            ("a".to_owned(), dependency("1", Target::Package("a@1".into()))),
                            ("b".to_owned(), dependency("1", Target::Package("b@1".into()))),
                        ]),
                    },
                ),
                (
                    "app".to_owned(),
                    Importer {
                        settings: Value::Null,
                        dependencies: BTreeMap::from([(
                            "lib".to_owned(),
                            dependency("link:../lib", Target::Importer("lib".into())),
                        )]),
                    },
                ),
            ]),
            packages: BTreeMap::from([
                ("a@1".to_owned(), package("a", vec![Target::Package("leaf@1".into())])),
                ("b@1".to_owned(), package("b", vec![])),
                ("leaf@1".to_owned(), package(leaf, vec![])),
            ]),
        };
        let (old, new) = (build("v1"), build("v2"));
        assert_eq!(new.changed_against(&old), BTreeSet::from([("app", "lib"), ("lib", "a")]));
        assert!(old.changed_against(&build("v1")).is_empty());
    }
}
