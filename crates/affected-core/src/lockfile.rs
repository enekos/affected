//! Map changed lockfile resolutions back to the dependencies that each importer actually loads.
//!
//! Each supported lockfile is read on both revisions. A lockfile that cannot be read or
//! understood on either side selects every entry.
mod npm;

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::source::Source;

#[derive(Default)]
pub(crate) struct Impact {
    pub broad: BTreeSet<PathBuf>,
    /// Pairs of (dependency node, changed lockfile).
    pub dependencies: BTreeSet<(PathBuf, PathBuf)>,
}

pub(crate) fn compare(head: &impl Source, base: Option<&impl Source>, changed: &[PathBuf]) -> Impact {
    let mut result = Impact::default();
    for path in changed {
        let comparison = match path.file_name().and_then(|p| p.to_str()) {
            Some("package-lock.json" | "npm-shrinkwrap.json") => base.and_then(|b| npm::changed(head, b, path)),
            Some("pnpm-lock.yaml" | "pnpm-lock.yml" | "yarn.lock" | "bun.lock" | "bun.lockb") => None,
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
