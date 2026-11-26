//! `package.json` dependencies as graph nodes.
//!
//! A file importing an installed package depends on the version range its nearest
//! manifest declares for it. That range is a node of its own, `<manifest>#<name>`, which
//! a diff hits when the range changes, so a dependency bump reaches exactly the files
//! that import the bumped package.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use rustc_hash::FxHashMap;
use serde_json::Value;

use crate::source::Source;

const SECTIONS: [&str; 4] = ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"];
const SEPARATOR: &str = "#";

/// The node standing for dependency `name` of `manifest`.
pub(crate) fn dependency_node(manifest: &Path, name: &str) -> PathBuf {
    let mut node = manifest.as_os_str().to_owned();
    node.push(SEPARATOR);
    node.push(name);
    PathBuf::from(node)
}

/// Whether a graph node is a dependency rather than a file.
pub(crate) fn is_dependency_node(node: &Path) -> bool {
    node.to_string_lossy().contains(concat!("package.json", "#"))
}

/// Every declared dependency and its version range; a name declared in several sections
/// joins their ranges. Empty when the manifest is missing or invalid.
pub(crate) fn dependencies<S: Source>(source: &S, path: &Path) -> FxHashMap<String, String> {
    let mut ranges: FxHashMap<String, String> = FxHashMap::default();
    let Ok(text) = source.read_to_string(path) else { return ranges };
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return ranges };
    for section in SECTIONS {
        let Some(entries) = value.get(section).and_then(Value::as_object) else { continue };
        for (name, range) in entries {
            let range = range.as_str().map_or_else(|| range.to_string(), str::to_owned);
            ranges
                .entry(name.clone())
                .and_modify(|joined| joined.push_str(&format!(" {section}:{range}")))
                .or_insert(range);
        }
    }
    ranges
}

/// Parsed manifests of one revision, bounded by the `package.json` files in the repository.
#[derive(Default)]
pub(crate) struct Manifests {
    parsed: Mutex<FxHashMap<PathBuf, Arc<FxHashMap<String, String>>>>,
}

impl Manifests {
    fn get<S: Source>(&self, source: &S, path: &Path) -> Arc<FxHashMap<String, String>> {
        // Poison means another reader panicked; the map still only holds complete entries.
        if let Some(cached) = self.parsed.lock().unwrap_or_else(PoisonError::into_inner).get(path) {
            return Arc::clone(cached);
        }
        let parsed = Arc::new(dependencies(source, path));
        self.parsed.lock().unwrap_or_else(PoisonError::into_inner).insert(path.to_path_buf(), Arc::clone(&parsed));
        parsed
    }

    /// The nearest manifest at or above `file`, inside `root`, that declares `name`.
    pub(crate) fn declaring<S: Source>(&self, source: &S, root: &Path, file: &Path, name: &str) -> Option<PathBuf> {
        file.ancestors().skip(1).take_while(|dir| dir.starts_with(root)).find_map(|dir| {
            let manifest = dir.join("package.json");
            self.get(source, &manifest).contains_key(name).then_some(manifest)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_nodes_are_told_apart_from_files() {
        let node = dependency_node(Path::new("/repo/package.json"), "@scope/pkg");
        assert_eq!(node, PathBuf::from("/repo/package.json#@scope/pkg"));
        assert!(is_dependency_node(&node));
        assert!(!is_dependency_node(Path::new("/repo/package.json")));
    }
}
