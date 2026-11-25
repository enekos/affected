use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use oxc_resolver::{ResolveOptions, ResolverGeneric, TsconfigDiscovery, TsconfigOptions, TsconfigReferences};
use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::imports::{self, Specifier};
use crate::source::Source;
use crate::tsconfig;
use crate::{Entry, Error};

/// Parsed module requests per file. Files identical to the working tree share one entry
/// across revisions; the rest are cached per revision.
#[derive(Default)]
pub(crate) struct ParseCache {
    disk: Mutex<FxHashMap<PathBuf, Arc<[Specifier]>>>,
    revision: Mutex<FxHashMap<PathBuf, Arc<[Specifier]>>>,
}

impl ParseCache {
    fn specifiers<S: Source>(&self, source: &S, path: &Path) -> Arc<[Specifier]> {
        let map = if source.matches_disk(path) { &self.disk } else { &self.revision };
        // Poison means another parser panicked; the map still only holds complete entries.
        if let Some(cached) = map.lock().unwrap_or_else(PoisonError::into_inner).get(path) {
            return Arc::clone(cached);
        }
        let parsed: Arc<[Specifier]> = match source.read_to_string(path) {
            Ok(text) if imports::is_parseable(path) => imports::extract(path, &text).into(),
            _ => Arc::from([]),
        };
        map.lock().unwrap_or_else(PoisonError::into_inner).insert(path.to_path_buf(), Arc::clone(&parsed));
        parsed
    }
}

/// Every file an entry reaches, with the breadth-first parent of each so the shortest
/// import chain to any file can be read back.
#[derive(Debug, Default)]
pub(crate) struct Graph {
    pub(crate) files: Vec<PathBuf>,
    parents: Vec<Option<usize>>,
    index: FxHashMap<PathBuf, usize>,
}

impl Graph {
    fn insert(&mut self, path: PathBuf, parent: Option<usize>) -> Option<usize> {
        if self.index.contains_key(&path) {
            return None;
        }
        let id = self.files.len();
        self.index.insert(path.clone(), id);
        self.files.push(path);
        self.parents.push(parent);
        Some(id)
    }

    pub(crate) fn contains(&self, path: &Path) -> bool {
        self.index.contains_key(path)
    }

    /// The import chain from an entry root to `path`, root first.
    pub(crate) fn chain(&self, path: &Path) -> Option<Vec<PathBuf>> {
        let mut current = *self.index.get(path)?;
        let mut chain = vec![self.files.get(current)?.clone()];
        while let Some(parent) = self.parents.get(current).copied().flatten() {
            chain.push(self.files.get(parent)?.clone());
            current = parent;
        }
        chain.reverse();
        Some(chain)
    }
}

const EXTENSIONS: [&str; 10] = [".ts", ".tsx", ".d.ts", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".json"];
const CONDITIONS: [&str; 5] = ["types", "import", "require", "node", "default"];

fn resolve_options(root: &Path, tsconfig: Option<&Path>, custom_conditions: &[String]) -> ResolveOptions {
    let owned = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect::<Vec<_>>();
    let mut condition_names = custom_conditions.to_vec();
    condition_names.extend(owned(&CONDITIONS));
    ResolveOptions {
        cwd: Some(root.to_path_buf()),
        tsconfig: Some(match tsconfig {
            Some(config_file) => TsconfigDiscovery::Manual(TsconfigOptions {
                config_file: config_file.to_path_buf(),
                references: TsconfigReferences::Auto,
            }),
            None => TsconfigDiscovery::Auto,
        }),
        condition_names,
        extensions: owned(&EXTENSIONS),
        extension_alias: vec![
            (".js".into(), owned(&[".ts", ".tsx", ".d.ts", ".js"])),
            (".jsx".into(), owned(&[".tsx", ".jsx"])),
            (".mjs".into(), owned(&[".mts", ".d.mts", ".mjs"])),
            (".cjs".into(), owned(&[".cts", ".d.cts", ".cjs"])),
        ],
        main_fields: owned(&["types", "typings", "module", "main"]),
        builtin_modules: true,
        ..ResolveOptions::default()
    }
}

/// Files inside the repository and outside `node_modules` are followed; packages are not.
fn is_followed(root: &Path, path: &Path) -> bool {
    path.starts_with(root) && !path.components().any(|part| part.as_os_str() == "node_modules")
}

pub(crate) fn build<S: Source>(
    source: &S,
    root: &Path,
    entry: &Entry,
    include_types: bool,
    cache: &ParseCache,
) -> Result<Graph, Error> {
    let mut graph = Graph::default();
    let (tsconfig_path, roots, conditions) = match entry {
        Entry::Tsconfig(path) => {
            let config = tsconfig::load(source, path)?;
            for file in config.configs {
                graph.insert(file, None);
            }
            (Some(path.as_path()), config.roots, config.conditions)
        }
        Entry::Files(files) => {
            for file in files {
                if source.metadata(file).is_err() {
                    return Err(Error::EntryNotFound(file.clone()));
                }
            }
            (None, files.clone(), Vec::new())
        }
    };
    let resolver =
        ResolverGeneric::new_with_file_system(source.clone(), resolve_options(root, tsconfig_path, &conditions));

    let mut frontier: Vec<usize> = roots.into_iter().filter_map(|file| graph.insert(file, None)).collect();
    while !frontier.is_empty() {
        let found: Vec<(usize, Vec<PathBuf>)> = frontier
            .par_iter()
            .filter_map(|&id| {
                let file = graph.files.get(id)?;
                let dependencies = cache
                    .specifiers(source, file)
                    .iter()
                    .filter(|specifier| include_types || !specifier.type_only)
                    .filter_map(|specifier| resolver.resolve_file(file, &specifier.request).ok())
                    .map(|resolution| resolution.into_path_buf())
                    .filter(|path| is_followed(root, path))
                    .collect();
                Some((id, dependencies))
            })
            .collect();
        frontier.clear();
        for (parent, dependencies) in found {
            for dependency in dependencies {
                if let Some(id) = graph.insert(dependency, Some(parent)) {
                    frontier.push(id);
                }
            }
        }
    }
    Ok(graph)
}
