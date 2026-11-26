use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use oxc_resolver::{
    ResolveError, ResolveOptions, ResolverGeneric, TsconfigDiscovery, TsconfigOptions, TsconfigReferences,
};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::imports::{self, Kind, Specifier};
use crate::manifest::{self, Manifests};
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

/// Every node an entry reaches, with the breadth-first parent of each so the shortest
/// import chain to any node can be read back. A node is a file, or a dependency declared
/// in a `package.json` (see [`manifest::dependency_node`]).
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

/// Frontiers smaller than this are walked on the calling thread.
const PARALLEL_FRONTIER: usize = 64;

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
pub(crate) fn is_followed(root: &Path, path: &Path) -> bool {
    path.starts_with(root) && !path.components().any(|part| part.as_os_str() == "node_modules")
}

/// A bare specifier's package name: `@scope/pkg/sub` → `@scope/pkg`, `pkg/sub` → `pkg`.
/// `None` for relative, absolute, `#imports`, URL and `node:` requests.
fn package_name(request: &str) -> Option<&str> {
    if request.starts_with(['.', '/', '#']) || request.contains(':') {
        return None;
    }
    let mut parts = request.splitn(3, '/');
    let first = parts.next()?;
    let end = if first.starts_with('@') { first.len() + 1 + parts.next()?.len() } else { first.len() };
    request.get(..end)
}

/// One way of resolving requests: an entry tsconfig (its `paths`, `baseUrl` and conditions),
/// or `None` for each file's nearest tsconfig.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Context {
    tsconfig: Option<PathBuf>,
    conditions: Vec<String>,
}

/// A file under one resolution context.
type EdgeKey = (Arc<Context>, PathBuf);

/// Builds entry graphs over one revision. Resolvers and each file's resolved dependencies
/// are kept for the whole analysis, so entries that share files resolve them once; both
/// maps are bounded by the files and tsconfigs of the repository.
pub(crate) struct Walker<'c, S: Source> {
    source: S,
    root: PathBuf,
    types: bool,
    parses: &'c ParseCache,
    manifests: Manifests,
    resolvers: Mutex<FxHashMap<Context, Arc<ResolverGeneric<S>>>>,
    edges: Mutex<FxHashMap<EdgeKey, Arc<[PathBuf]>>>,
}

impl<'c, S: Source> Walker<'c, S> {
    pub(crate) fn new(source: S, root: &Path, types: bool, parses: &'c ParseCache) -> Self {
        Self {
            source,
            root: root.to_path_buf(),
            types,
            parses,
            manifests: Manifests::default(),
            resolvers: Mutex::default(),
            edges: Mutex::default(),
        }
    }

    pub(crate) fn source(&self) -> &S {
        &self.source
    }

    fn resolver(&self, context: &Context) -> Arc<ResolverGeneric<S>> {
        let mut resolvers = self.resolvers.lock().unwrap_or_else(PoisonError::into_inner);
        let resolver = resolvers.entry(context.clone()).or_insert_with(|| {
            Arc::new(ResolverGeneric::new_with_file_system(
                self.source.clone(),
                resolve_options(&self.root, context.tsconfig.as_deref(), &context.conditions),
            ))
        });
        Arc::clone(resolver)
    }

    /// The graph of one entry. Roots missing from this revision are skipped: the caller
    /// has checked that each exists on at least one side of the diff.
    pub(crate) fn build(&self, entry: &Entry) -> Result<Graph, Error> {
        let mut graph = Graph::default();
        let mut frontier: Vec<(usize, Arc<Context>)> = Vec::new();
        match entry {
            Entry::Tsconfig(path) => {
                if self.source.metadata(path).is_err() {
                    return Ok(graph);
                }
                for project in tsconfig::load_projects(&self.source, path)? {
                    for config in project.configs {
                        graph.insert(config, None);
                    }
                    let context = Arc::new(Context { tsconfig: Some(project.path), conditions: project.conditions });
                    for file in project.roots {
                        if let Some(id) = graph.insert(file, None) {
                            frontier.push((id, Arc::clone(&context)));
                        }
                    }
                }
            }
            Entry::Files(files) => {
                let context = Arc::new(Context { tsconfig: None, conditions: Vec::new() });
                for file in files.iter().filter(|file| self.source.metadata(file).is_ok()) {
                    if let Some(id) = graph.insert(file.clone(), None) {
                        frontier.push((id, Arc::clone(&context)));
                    }
                }
            }
        }

        while !frontier.is_empty() {
            let visit = |(id, context): &(usize, Arc<Context>)| {
                let file = graph.files.get(*id)?;
                Some((*id, Arc::clone(context), self.dependencies(context, file)))
            };
            // Fork-join costs more than it saves on a small frontier, or once its files are cached.
            let found: Vec<(usize, Arc<Context>, Arc<[PathBuf]>)> = if frontier.len() < PARALLEL_FRONTIER {
                frontier.iter().filter_map(visit).collect()
            } else {
                frontier.par_iter().filter_map(visit).collect()
            };
            frontier.clear();
            for (parent, context, dependencies) in found {
                for dependency in dependencies.iter() {
                    if let Some(id) = graph.insert(dependency.clone(), Some(parent)) {
                        frontier.push((id, Arc::clone(&context)));
                    }
                }
            }
        }
        Ok(graph)
    }

    fn dependencies(&self, context: &Arc<Context>, file: &Path) -> Arc<[PathBuf]> {
        let key = (Arc::clone(context), file.to_path_buf());
        if let Some(cached) = self.edges.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return Arc::clone(cached);
        }
        let resolved: Arc<[PathBuf]> = self.resolve_all(context, file).into();
        self.edges.lock().unwrap_or_else(PoisonError::into_inner).insert(key, Arc::clone(&resolved));
        resolved
    }

    fn resolve_all(&self, context: &Context, file: &Path) -> Vec<PathBuf> {
        let specifiers = self.parses.specifiers(&self.source, file);
        if specifiers.is_empty() {
            return Vec::new();
        }
        let resolver = self.resolver(context);
        let dir = file.parent().unwrap_or(&self.root);
        let mut found = Vec::new();
        for specifier in specifiers.iter().filter(|specifier| self.types || !specifier.type_only) {
            match specifier.kind {
                Kind::Module => self.resolve_module(&resolver, file, &specifier.request, &mut found),
                Kind::File => self.resolve_path(file, &tsconfig::normalize(&dir.join(&*specifier.request)), &mut found),
                Kind::Glob => self.resolve_glob(dir, &specifier.request, &mut found),
            }
        }
        found.retain(|path| path != file);
        found
    }

    fn resolve_module(&self, resolver: &ResolverGeneric<S>, file: &Path, request: &str, found: &mut Vec<PathBuf>) {
        let resolution = resolver.resolve_file(file, request);
        if let Err(ResolveError::Builtin { .. }) = resolution {
            return;
        }
        let bare = package_name(request);
        if let Ok(resolution) = &resolution
            && is_followed(&self.root, resolution.path())
        {
            found.push(resolution.path().to_path_buf());
            // A workspace package reached through `node_modules`: its manifest decides where the
            // name points. Path aliases also land in the repository but never pass a manifest.
            if bare.is_some()
                && let Some(package) = resolution.package_json()
                && !is_followed(&self.root, package.path())
                && is_followed(&self.root, package.realpath())
            {
                found.push(package.realpath().to_path_buf());
            }
            return;
        }
        // An installed package, or one that does not resolve: the dependency its nearest manifest declares.
        if let Some(name) = bare
            && let Some(manifest) = self.manifests.declaring(&self.source, &self.root, file, name)
        {
            found.push(manifest::dependency_node(&manifest, name));
        }
    }

    /// A file, or every file below a directory. A directory holding the importing file is a
    /// base for further path building (`resolve(__dirname, '../..')`), not an asset.
    fn resolve_path(&self, file: &Path, path: &Path, found: &mut Vec<PathBuf>) {
        match self.source.metadata(path) {
            Ok(metadata) if metadata.is_file() => found.push(path.to_path_buf()),
            Ok(metadata) if metadata.is_dir() && is_followed(&self.root, path) && !file.starts_with(path) => {
                found.extend(self.source.files_under(path));
            }
            _ => {}
        }
    }

    fn resolve_glob(&self, dir: &Path, pattern: &str, found: &mut Vec<PathBuf>) {
        let patterns = [pattern.to_owned()];
        let Ok(globs) = tsconfig::glob_set(dir, &patterns, false) else { return };
        let base = tsconfig::static_prefix(dir, &patterns);
        if !is_followed(&self.root, &base) {
            return;
        }
        found.extend(self.source.files_under(&base).into_iter().filter(|path| globs.is_match(path)));
    }
}

/// Changed dependency nodes: for each changed `package.json`, the dependencies whose
/// version range differs between the two revisions. Without a base revision every
/// dependency of a changed manifest counts.
pub(crate) fn changed_dependencies<H: Source, B: Source>(
    head: &H,
    base: Option<&B>,
    changed: &[PathBuf],
) -> Vec<PathBuf> {
    let mut nodes = Vec::new();
    for path in changed.iter().filter(|path| path.file_name().is_some_and(|name| name == "package.json")) {
        let after = manifest::dependencies(head, path);
        let names: FxHashSet<String> = match base {
            Some(base) => {
                let before = manifest::dependencies(base, path);
                let mut names: FxHashSet<String> = after
                    .iter()
                    .filter(|(name, range)| before.get(*name) != Some(*range))
                    .map(|(name, _)| name.clone())
                    .collect();
                names.extend(before.keys().filter(|name| !after.contains_key(*name)).cloned());
                names
            }
            None => after.into_keys().collect(),
        };
        nodes.extend(names.iter().map(|name| manifest::dependency_node(path, name)));
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_of_bare_specifiers() {
        assert_eq!(package_name("zod"), Some("zod"));
        assert_eq!(package_name("lodash/fp"), Some("lodash"));
        assert_eq!(package_name("@scope/pkg/sub/path"), Some("@scope/pkg"));
        assert_eq!(package_name("./a"), None);
        assert_eq!(package_name("#internal"), None);
        assert_eq!(package_name("node:fs"), None);
        assert_eq!(package_name("@scope"), None);
    }
}
