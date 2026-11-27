//! pnpm `pnpm-lock.yaml`, lockfile versions 6.x and 9.x.
//!
//! Version 9 splits each package into its registry record (`packages`, keyed `name@version`)
//! and its resolved dependencies (`snapshots`, keyed with peer suffixes such as
//! `name@1.0.0(react@18.2.0)`). Version 6 keeps both under `packages`, keyed `/name@version`.
use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{Dependency, Importer, Package, Resolution, Target, importer_dir, join_relative, yaml};

const IMPORTER_SECTIONS: [&str; 3] = ["dependencies", "devDependencies", "optionalDependencies"];
const PACKAGE_SECTIONS: [&str; 2] = ["dependencies", "optionalDependencies"];
/// Top-level fields that restate what importers and packages already resolve. Everything
/// else (settings, overrides, patches, hooks) may change any resolution.
const DERIVED: [&str; 5] = ["importers", "packages", "snapshots", "catalogs", "time"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    V6,
    V9,
}

pub(super) fn resolution(text: &str) -> Option<Resolution> {
    let mut documents = yaml::documents(text)?;
    // pnpm 10 may write its own config dependencies as a leading document.
    let Value::Object(mut lock) = documents.pop()? else { return None };
    let format = match lock.get("lockfileVersion")?.as_str()?.split_once('.')? {
        ("6", minor) if is_number(minor) => Format::V6,
        ("9", minor) if is_number(minor) => Format::V9,
        _ => return None,
    };
    let importers = match lock.remove("importers") {
        Some(Value::Object(importers)) => importers,
        Some(_) => return None,
        // A single-project version 6 lockfile lists the root's dependencies at the top.
        None if format == Format::V6 => {
            let mut root = Map::new();
            for section in IMPORTER_SECTIONS.iter().chain(&["dependenciesMeta"]) {
                if let Some(entries) = lock.remove(*section) {
                    root.insert((*section).to_owned(), entries);
                }
            }
            Map::from_iter([(".".to_owned(), Value::Object(root))])
        }
        None => return None,
    };
    let empty = Map::new();
    let records = section(&lock, "packages", &empty)?;
    let snapshots = section(&lock, "snapshots", &empty)?;
    if format == Format::V6 && lock.contains_key("snapshots") {
        return None;
    }
    let mut dirs = BTreeMap::new();
    for dir in importers.keys() {
        if dirs.insert(importer_dir(dir)?, dir.as_str()).is_some() {
            return None;
        }
    }
    let mut packages = BTreeMap::new();
    let nodes = if format == Format::V9 { snapshots } else { records };
    for (id, value) in nodes {
        let node = value.as_object()?;
        let (record, registry) = match format {
            Format::V9 => {
                // The registry record is keyed without peer or patch suffixes.
                let registry = records.get(id.split_once('(').map_or(id.as_str(), |(base, _)| base))?;
                (Value::Array(vec![registry.clone(), Value::Object(node.clone())]), registry)
            }
            Format::V6 => {
                let mut record = node.clone();
                // Whether only dev dependencies reach a package does not change what it is.
                record.remove("dev");
                (Value::Object(record), value)
            }
        };
        let mut dependencies = Vec::new();
        // An injected workspace package is a copy of that importer.
        if let Some(directory) = registry.get("resolution").and_then(|r| r.get("directory"))
            && let Some(dir) = join_relative("", directory.as_str()?)
            && dirs.contains_key(&dir)
        {
            dependencies.push(Target::Importer(dir));
        }
        for section in PACKAGE_SECTIONS {
            let Some(declared) = node.get(section) else { continue };
            for (name, reference) in declared.as_object()? {
                let reference = reference.as_str()?;
                match target(format, nodes, &dirs, "", name, reference) {
                    Some(Some(target)) => dependencies.push(target),
                    Some(None) => {}
                    None if section == "optionalDependencies" => {}
                    None => return None,
                }
            }
        }
        packages.insert(id.clone(), Package { record, dependencies });
    }
    let mut resolved = BTreeMap::new();
    for (dir, name) in &dirs {
        let mut importer = importers.get(*name)?.as_object()?.clone();
        let mut dependencies: BTreeMap<String, Dependency> = BTreeMap::new();
        for section in IMPORTER_SECTIONS {
            let Some(declared) = importer.remove(section) else { continue };
            for (dependency, entry) in declared.as_object()? {
                let specifier = entry.get("specifier")?.as_str()?;
                let version = entry.get("version")?.as_str()?;
                let found = match target(format, nodes, &dirs, dir, dependency, version) {
                    Some(found) => found,
                    None if section == "optionalDependencies" => None,
                    None => return None,
                };
                let joined = dependencies.entry(dependency.clone()).or_default();
                joined.declared.push_str(&format!("{section}:{specifier}:{version} "));
                joined.targets.extend(found);
            }
        }
        resolved.insert(dir.clone(), Importer { settings: Value::Object(importer), dependencies });
    }
    for key in DERIVED {
        lock.remove(key);
    }
    documents.push(Value::Object(lock));
    Some(Resolution { settings: Value::Array(documents), importers: resolved, packages })
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// A top-level map; a lockfile without any packages omits it.
fn section<'a>(
    lock: &'a Map<String, Value>,
    name: &str,
    empty: &'a Map<String, Value>,
) -> Option<&'a Map<String, Value>> {
    match lock.get(name) {
        None | Some(Value::Null) => Some(empty),
        Some(value) => value.as_object(),
    }
}

/// What a dependency reference resolves to: `Some(None)` for a link outside the workspace,
/// `None` when the lockfile does not record it.
fn target(
    format: Format,
    nodes: &Map<String, Value>,
    dirs: &BTreeMap<String, &str>,
    from: &str,
    name: &str,
    reference: &str,
) -> Option<Option<Target>> {
    if let Some(path) = reference.strip_prefix("link:") {
        let dir = join_relative(from, path);
        return Some(dir.filter(|dir| dirs.contains_key(dir)).map(Target::Importer));
    }
    let id = match format {
        Format::V9 => v9_id(name, reference),
        Format::V6 => v6_id(name, reference),
    };
    nodes.contains_key(&id).then_some(Some(Target::Package(id)))
}

/// pnpm 9's `refToRelative`: a reference that already names its package (an alias such as
/// `string-width@4.2.3`) is the id; a bare version is prefixed with the dependency name.
fn v9_id(name: &str, reference: &str) -> String {
    if reference.starts_with('@') {
        return reference.to_owned();
    }
    let Some(at) = reference.find('@') else { return format!("{name}@{reference}") };
    let before = |c| reference.find(c).is_none_or(|i| at < i);
    if before(':') && before('(') { reference.to_owned() } else { format!("{name}@{reference}") }
}

/// pnpm 8's `refToRelative`: registry versions become `/name@version`; paths such as
/// `/alias@1.0.0` or `github.com/…` are ids already.
fn v6_id(name: &str, reference: &str) -> String {
    let version = reference.split_once('(').map_or(reference, |(version, _)| version);
    if reference.starts_with("file:") || version.contains('/') {
        reference.to_owned()
    } else {
        format!("/{name}@{reference}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_map_to_package_ids() {
        assert_eq!(v9_id("react", "18.2.0"), "react@18.2.0");
        assert_eq!(v9_id("use-sync", "1.2.0(react@18.2.0)"), "use-sync@1.2.0(react@18.2.0)");
        assert_eq!(v9_id("sw-cjs", "string-width@4.2.3"), "string-width@4.2.3");
        assert_eq!(v9_id("t", "@types/node@20.0.0"), "@types/node@20.0.0");
        assert_eq!(v9_id("x", "https://example.com/x.tgz"), "x@https://example.com/x.tgz");
        assert_eq!(v6_id("react", "18.2.0"), "/react@18.2.0");
        assert_eq!(v6_id("a", "1.0.0(@types/react@18.0.0)"), "/a@1.0.0(@types/react@18.0.0)");
        assert_eq!(v6_id("sw-cjs", "/string-width@4.2.3"), "/string-width@4.2.3");
        assert_eq!(v6_id("x", "file:packages/x"), "file:packages/x");
    }
}
