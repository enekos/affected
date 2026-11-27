//! Yarn Berry (2+) `yarn.lock`, metadata versions 4 to 8.
//!
//! Each entry is keyed by the descriptors (`name@range`, comma-separated) that resolve to it.
//! Workspaces are entries whose resolution is `name@workspace:<dir>`; their dependencies
//! join the manifest's `dependencies` and `devDependencies`.
//!
//! Peer dependencies are resolved by Yarn at install time and are not recorded, so a package
//! with a peer counts as depending on every locked package of that name.
//!
//! Yarn classic (v1) lockfiles record neither workspaces nor peer dependencies: a peer-only
//! bump cannot be traced to its consumers, so they are not compared.
use std::collections::BTreeMap;

use serde_json::Value;

use super::{Dependency, Importer, Package, Resolution, Target, importer_dir, yaml};

const METADATA: &str = "__metadata";
const WORKSPACE: &str = "@workspace:";
const SUPPORTED: std::ops::RangeInclusive<u64> = 4..=8;

pub(super) fn resolution(text: &str) -> Option<Resolution> {
    let mut documents = yaml::documents(text)?;
    let Value::Object(mut lock) = documents.pop()? else { return None };
    if !documents.is_empty() {
        return None;
    }
    let metadata = lock.remove(METADATA)?;
    let version = metadata.get("version")?;
    let version = version.as_u64().or_else(|| version.as_str()?.parse().ok())?;
    if !SUPPORTED.contains(&version) {
        return None;
    }
    let mut descriptors = BTreeMap::new();
    let mut named: BTreeMap<&str, Vec<Target>> = BTreeMap::new();
    let mut targets = BTreeMap::new();
    for (key, entry) in &lock {
        let resolution = entry.get("resolution")?.as_str()?;
        let target = match resolution.split_once(WORKSPACE) {
            Some((_, dir)) => Target::Importer(importer_dir(dir)?),
            None => Target::Package(resolution.to_owned()),
        };
        if targets.insert(resolution, target.clone()).is_some() {
            return None;
        }
        named.entry(name(resolution)?).or_default().push(target.clone());
        for descriptor in key.split(',').map(str::trim) {
            if descriptors.insert(descriptor, target.clone()).is_some() {
                return None;
            }
        }
    }
    let resolve = |dependency: &str, range: &str| {
        descriptors
            .get(format!("{dependency}@{range}").as_str())
            .or_else(|| descriptors.get(format!("{dependency}@npm:{range}").as_str()))
            .cloned()
    };
    let mut result = Resolution { settings: metadata, ..Resolution::default() };
    for entry in lock.values() {
        let mut record = entry.as_object()?.clone();
        let mut dependencies: BTreeMap<String, Dependency> = BTreeMap::new();
        if let Some(declared) = record.get("dependencies") {
            for (dependency, range) in declared.as_object()? {
                let range = range.as_str()?;
                let joined = dependencies.entry(dependency.clone()).or_default();
                joined.declared.push_str(range);
                joined.targets.push(resolve(dependency, range)?);
            }
        }
        if let Some(declared) = record.get("peerDependencies") {
            for (dependency, range) in declared.as_object()? {
                let joined = dependencies.entry(dependency.clone()).or_default();
                joined.declared.push_str(&format!(" peer:{}", range.as_str()?));
                joined.targets.extend(named.get(dependency.as_str()).into_iter().flatten().cloned());
            }
        }
        match targets.get(entry.get("resolution")?.as_str()?)? {
            Target::Importer(dir) => {
                record.remove("dependencies");
                record.remove("peerDependencies");
                let importer = Importer { settings: Value::Object(record), dependencies };
                if result.importers.insert(dir.clone(), importer).is_some() {
                    return None;
                }
            }
            Target::Package(id) => {
                let dependencies = dependencies.into_values().flat_map(|d| d.targets).collect();
                result.packages.insert(id.clone(), Package { record: Value::Object(record), dependencies });
            }
        }
    }
    Some(result)
}

/// The package name of a locator or descriptor, `@scope/name` or `name`.
fn name(locator: &str) -> Option<&str> {
    let start = usize::from(locator.starts_with('@'));
    let at = locator.get(start..)?.find('@')? + start;
    locator.get(..at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_of_locators() {
        assert_eq!(name("debug@npm:4.3.4"), Some("debug"));
        assert_eq!(name("@ws/a@workspace:packages/a"), Some("@ws/a"));
        assert_eq!(name("resolve@patch:resolve@npm%3A1.22.8#~builtin<compat/resolve>"), Some("resolve"));
        assert_eq!(name("nameless"), None);
    }

    #[test]
    fn classic_and_unknown_lockfiles_are_not_compared() {
        assert!(resolution("# yarn lockfile v1\n\n\ndebug@4.3.4:\n  version \"4.3.4\"\n").is_none());
        assert!(resolution("__metadata:\n  version: 99\n").is_none());
        assert!(resolution("__metadata:\n  version: 8\n").is_some());
    }
}
