use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use oxc_resolver::{
    ResolveError, ResolveOptions, ResolverGeneric, TsconfigDiscovery, TsconfigOptions, TsconfigReferences,
};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::bindings::{Binding, Changes};
use crate::imports::{self, Kind, Specifier};
use crate::manifest::{self, Manifests};
use crate::source::Source;
use crate::tsconfig;
use crate::{Entry, Error, TraceEdge};

/// Parsed module requests per file. Files identical to the working tree share one entry
/// across revisions; the rest are cached per revision.
#[derive(Default)]
pub(crate) struct ParseCache {
    bindings: bool,
    disk: Mutex<FxHashMap<PathBuf, Arc<[Specifier]>>>,
    revision: Mutex<FxHashMap<PathBuf, Arc<[Specifier]>>>,
}

impl ParseCache {
    pub(crate) fn new(bindings: bool) -> Self {
        Self { bindings, ..Self::default() }
    }
    fn specifiers<S: Source>(&self, source: &S, path: &Path) -> Arc<[Specifier]> {
        let map = if source.matches_disk(path) { &self.disk } else { &self.revision };
        // Poison means another parser panicked; the map still only holds complete entries.
        if let Some(cached) = map.lock().unwrap_or_else(PoisonError::into_inner).get(path) {
            return Arc::clone(cached);
        }
        let parsed: Arc<[Specifier]> = match source.read_to_string(path) {
            Ok(text) if imports::is_parseable(path) => imports::extract(path, &text, self.bindings).into(),
            Err(error) if imports::is_parseable(path) => Arc::from([Specifier {
                request: format!("cannot read source: {error}").into(),
                type_only: false,
                kind: Kind::Unknown,
            }]),
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
    precise: FxHashMap<PathBuf, Option<Vec<PathBuf>>>,
    index: FxHashMap<PathBuf, usize>,
    pub(crate) unknown: Vec<(PathBuf, String)>,
    /// Target node of each trace edge chosen by the breadth-first walk.
    pub(crate) traced: FxHashSet<PathBuf>,
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

    /// The import chain from an entry root to `path`, root first.
    pub(crate) fn chain(&self, path: &Path) -> Option<Vec<PathBuf>> {
        if let Some(chain) = self.precise.get(path) {
            return chain.clone();
        }
        self.file_chain(path)
    }

    fn file_chain(&self, path: &Path) -> Option<Vec<PathBuf>> {
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

type Consumers = FxHashMap<PathBuf, Vec<(usize, Option<Arc<Binding>>)>>;

impl Graph {
    /// Walk *every* incoming edge of the changed export, including paths through
    /// different barrels and aliases. Opaque consumers retain ordinary reachability.
    fn binding_chain(&self, changed: &Path, names: &[String], consumers: &Consumers) -> Option<Vec<PathBuf>> {
        use std::collections::VecDeque;
        let mut queue: VecDeque<_> = names.iter().map(|name| (name.clone(), vec![changed.to_owned()])).collect();
        let mut seen = FxHashSet::default();
        let mut best = None;
        while let Some((symbol, suffix)) = queue.pop_front() {
            let file = suffix.last()?;
            if !seen.insert((file.clone(), symbol.clone())) {
                continue;
            }
            if self.index.get(file).and_then(|i| self.parents.get(*i)).is_some_and(Option::is_none) {
                let path: Vec<_> = suffix.into_iter().rev().collect();
                if best.as_ref().is_none_or(|old: &Vec<_>| path.len() < old.len()) {
                    best = Some(path);
                }
                continue;
            }
            for (id, binding) in consumers.get(file).into_iter().flatten() {
                let parent = self.files.get(*id)?;
                let mut forward = |name: &str| {
                    let mut path = suffix.clone();
                    path.push(parent.clone());
                    queue.push_back((name.to_owned(), path));
                };
                match binding.as_deref() {
                    Some(Binding::Star) => {
                        if symbol != "default" {
                            forward(&symbol);
                        }
                    }
                    Some(Binding::Aliases(pairs)) => {
                        for (_, alias) in pairs.iter().filter(|(name, _)| name == &symbol) {
                            forward(alias);
                        }
                    }
                    Some(Binding::Names(names)) if !names.contains(&symbol) => {}
                    _ => {
                        let mut path = self.file_chain(parent)?;
                        path.extend(suffix.iter().rev().cloned());
                        if best.as_ref().is_none_or(|old| path.len() < old.len()) {
                            best = Some(path);
                        }
                    }
                }
            }
        }
        best
    }
}

/// Frontiers smaller than this are walked on the calling thread.
const PARALLEL_FRONTIER: usize = 64;

const EXTENSIONS: [&str; 10] = [".ts", ".tsx", ".d.ts", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".json"];
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Mode {
    require: bool,
    types: bool,
}

fn resolve_options(root: &Path, tsconfig: Option<&Path>, custom_conditions: &[String], mode: Mode) -> ResolveOptions {
    let owned = |items: &[&str]| items.iter().map(|item| (*item).to_owned()).collect::<Vec<_>>();
    let mut condition_names = custom_conditions.to_vec();
    condition_names.extend(owned(&[if mode.require { "require" } else { "import" }, "node", "default"]));
    if mode.types {
        condition_names.push("types".to_owned());
    }
    let runtime = if mode.types && !mode.require { vec!["module", "main"] } else { vec!["main", "module"] };
    let main_fields = if mode.types { ["types", "typings"].into_iter().chain(runtime).collect() } else { runtime };
    let mut options = ResolveOptions {
        cwd: Some(root.to_path_buf()),
        tsconfig: Some(match tsconfig {
            Some(config_file) => TsconfigDiscovery::Manual(TsconfigOptions {
                config_file: config_file.to_path_buf(),
                references: TsconfigReferences::Auto,
            }),
            None => TsconfigDiscovery::Auto,
        }),
        condition_names,
        extensions: EXTENSIONS.iter().filter(|s| mode.types || **s != ".d.ts").map(|s| (*s).into()).collect(),
        extension_alias: vec![
            (".js".into(), owned(&[".ts", ".tsx", ".d.ts", ".js"])),
            (".jsx".into(), owned(&[".tsx", ".jsx"])),
            (".mjs".into(), owned(&[".mts", ".d.mts", ".mjs"])),
            (".cjs".into(), owned(&[".cts", ".d.cts", ".cjs"])),
        ],
        main_fields: owned(&main_fields),
        builtin_modules: true,
        ..ResolveOptions::default()
    };
    if !mode.types {
        for (extension, aliases) in &mut options.extension_alias {
            // The emitted file is the runtime input when it exists alongside the TS source.
            aliases.sort_by_key(|ext| ext != extension);
            aliases.retain(|ext| !ext.starts_with(".d."));
        }
    }
    options
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

#[derive(Default)]
struct Dependencies {
    bindings: FxHashMap<PathBuf, Arc<Binding>>,
    files: Vec<PathBuf>,
    unknown: Vec<String>,
}

#[derive(Default)]
struct FileConfig {
    paths: Vec<PathBuf>,
    conditions: Vec<String>,
    error: Option<String>,
}

type ResolverCache<S> = FxHashMap<(Context, Mode), Arc<ResolverGeneric<S>>>;

/// Builds entry graphs over one revision. Resolvers and each file's resolved dependencies
/// are kept for the whole analysis, so entries that share files resolve them once; both
/// maps are bounded by the files and tsconfigs of the repository.
pub(crate) struct Walker<'c, S: Source> {
    source: S,
    root: PathBuf,
    types: bool,
    parses: &'c ParseCache,
    manifests: Manifests,
    resolvers: Mutex<ResolverCache<S>>,
    configs: Mutex<FxHashMap<PathBuf, Arc<FileConfig>>>,
    trace: FxHashMap<PathBuf, Vec<PathBuf>>,
    edges: Mutex<FxHashMap<EdgeKey, Arc<Dependencies>>>,
}

impl<'c, S: Source> Walker<'c, S> {
    pub(crate) fn new(
        source: S,
        root: &Path,
        types: bool,
        parses: &'c ParseCache,
        trace: &[TraceEdge],
        cwd: &Path,
        revision: &str,
    ) -> Self {
        let mut traced: FxHashMap<PathBuf, Vec<PathBuf>> = FxHashMap::default();
        for edge in trace.iter().filter(|e| e.revision.as_deref().is_none_or(|side| side == revision)) {
            traced.entry(crate::absolute(cwd, &edge.from)).or_default().push(crate::absolute(cwd, &edge.to));
        }
        Self {
            source,
            root: root.to_path_buf(),
            types,
            parses,
            manifests: Manifests::default(),
            resolvers: Mutex::default(),
            configs: Mutex::default(),
            trace: traced,
            edges: Mutex::default(),
        }
    }

    pub(crate) fn source(&self) -> &S {
        &self.source
    }

    fn resolver(&self, context: &Context, mode: Mode) -> Arc<ResolverGeneric<S>> {
        let mut resolvers = self.resolvers.lock().unwrap_or_else(PoisonError::into_inner);
        let resolver = resolvers.entry((context.clone(), mode)).or_insert_with(|| {
            Arc::new(ResolverGeneric::new_with_file_system(
                self.source.clone(),
                resolve_options(&self.root, context.tsconfig.as_deref(), &context.conditions, mode),
            ))
        });
        Arc::clone(resolver)
    }

    /// The graph of one entry. Roots missing from this revision are skipped: the caller
    /// has checked that each exists on at least one side of the diff.
    pub(crate) fn build(&self, entry: &Entry, changes: &Changes) -> Result<Graph, Error> {
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

        let refine = !changes.is_empty() && matches!(entry, Entry::Files(_));
        let mut consumers = Consumers::default();
        while !frontier.is_empty() {
            let visit = |(id, context): &(usize, Arc<Context>)| {
                let file = graph.files.get(*id)?;
                Some((*id, Arc::clone(context), self.dependencies(context, file)))
            };
            // Fork-join costs more than it saves on a small frontier, or once its files are cached.
            let found: Vec<_> = if frontier.len() < PARALLEL_FRONTIER {
                frontier.iter().filter_map(visit).collect()
            } else {
                frontier.par_iter().filter_map(visit).collect()
            };
            frontier.clear();
            for (parent, context, dependencies) in found {
                let Some(file) = graph.files.get(parent) else { continue };
                for expr in &dependencies.unknown {
                    graph.unknown.push((file.clone(), expr.clone()));
                }
                let traced = self.trace.get(file).cloned().unwrap_or_default();
                for dependency in &dependencies.files {
                    if refine {
                        consumers
                            .entry(dependency.clone())
                            .or_default()
                            .push((parent, dependencies.bindings.get(dependency).cloned()));
                    }
                    if let Some(id) = graph.insert(dependency.clone(), Some(parent)) {
                        frontier.push((id, Arc::clone(&context)));
                    }
                }
                for dependency in traced {
                    if refine {
                        consumers.entry(dependency.clone()).or_default().push((parent, None));
                    }
                    if let Some(id) = graph.insert(dependency.clone(), Some(parent)) {
                        graph.traced.insert(dependency);
                        frontier.push((id, Arc::clone(&context)));
                    }
                }
            }
        }
        if refine {
            for (path, names) in changes.iter().filter(|(p, _)| graph.index.contains_key(*p)) {
                let chain = graph.binding_chain(path, names, &consumers);
                graph.precise.insert(path.clone(), chain);
            }
        }
        Ok(graph)
    }

    fn dependencies(&self, context: &Arc<Context>, file: &Path) -> Arc<Dependencies> {
        let key = (Arc::clone(context), file.to_path_buf());
        if let Some(cached) = self.edges.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
            return Arc::clone(cached);
        }
        let resolved = Arc::new(self.resolve_all(context, file));
        self.edges.lock().unwrap_or_else(PoisonError::into_inner).insert(key, Arc::clone(&resolved));
        resolved
    }

    fn file_config(&self, dir: &Path) -> Arc<FileConfig> {
        let mut configs = self.configs.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(configs.entry(dir.into()).or_insert_with(|| {
            let mut config = FileConfig::default();
            if let Some(path) = dir
                .ancestors()
                .take_while(|p| p.starts_with(&self.root))
                .map(|p| p.join("tsconfig.json"))
                .find(|p| self.source.metadata(p).is_ok())
            {
                match tsconfig::resolution_configs(&self.source, &path) {
                    Ok((paths, conditions)) => {
                        config.paths = paths;
                        config.conditions = conditions;
                    }
                    Err(error) => {
                        config.paths.push(path);
                        config.error = Some(error.to_string());
                    }
                }
            }
            Arc::new(config)
        }))
    }

    fn resolve_all(&self, context: &Context, file: &Path) -> Dependencies {
        let mut deps = Dependencies::default();
        if !imports::is_parseable(file) || !is_followed(&self.root, file) {
            return deps;
        }
        if self.source.symlink_metadata(file).is_ok_and(|m| m.is_symlink()) {
            if let Ok(target) = self.source.canonicalize(file)
                && is_followed(&self.root, &target)
            {
                deps.files.push(target);
            }
            return deps;
        }
        let dir = file.parent().unwrap_or(&self.root);
        let local;
        let context = if context.tsconfig.is_none() {
            let config = self.file_config(dir);
            deps.files.extend(config.paths.iter().cloned());
            deps.unknown.extend(config.error.iter().cloned());
            local = Context { tsconfig: config.paths.first().cloned(), conditions: config.conditions.clone() };
            &local
        } else {
            context
        };
        let mut broad: FxHashSet<PathBuf> = deps.files.iter().cloned().collect();
        for spec in self.parses.specifiers(&self.source, file).iter().filter(|s| self.types || !s.type_only) {
            let start = deps.files.len();
            let request = &spec.request;
            let static_import = matches!(spec.kind, Kind::Module | Kind::Bound(_));
            match &spec.kind {
                Kind::Module | Kind::Bound(_) | Kind::Require | Kind::DynamicImport => {
                    let ext = file.extension().and_then(|s| s.to_str());
                    let require = spec.kind == Kind::Require || (static_import && matches!(ext, Some("cts" | "cjs")));
                    // A TS static import can emit either module kind. Keep both runtime branches
                    // for package specifiers when the file extension does not determine its emit.
                    let ambiguous = static_import && matches!(ext, Some("ts" | "tsx")) && !request.starts_with('.');
                    let modes: &[bool] = if ambiguous { &[false, true] } else { std::slice::from_ref(&require) };
                    let mut resolved = false;
                    for &require in modes {
                        if !spec.type_only {
                            let resolver = self.resolver(context, Mode { require, types: false });
                            resolved |= self.resolve_module(&resolver, file, request, &mut deps.files);
                        }
                        if self.types {
                            let resolver = self.resolver(context, Mode { require, types: true });
                            let typed = self.resolve_module(&resolver, file, request, &mut deps.files);
                            if spec.type_only {
                                resolved |= typed;
                            }
                        }
                    }
                    if !resolved {
                        deps.unknown.push(format!("unresolved module {request}"));
                    }
                    // Retain the alias as a graph input when a tracked symlink itself changes.
                    if request.starts_with('.') {
                        let path = tsconfig::normalize(&dir.join(&**request));
                        deps.files.extend(
                            path.ancestors()
                                .take_while(|p| p.starts_with(&self.root))
                                .filter(|p| self.source.symlink_metadata(p).is_ok_and(|m| m.is_symlink()))
                                .map(Path::to_owned),
                        );
                    }
                }
                Kind::Unknown => deps.unknown.push(request.to_string()),
                Kind::File => self.resolve_path(file, &tsconfig::normalize(&dir.join(&**request)), &mut deps.files),
                Kind::Glob => self.resolve_glob(dir, request, &[], &mut deps.files),
                Kind::GlobExcept(excluded) => self.resolve_glob(dir, request, excluded, &mut deps.files),
            }
            if !self.parses.bindings {
                continue;
            }
            for path in deps.files.get(start..).unwrap_or_default() {
                if let Kind::Bound(binding) = &spec.kind
                    && !broad.contains(path)
                {
                    let prior = deps.bindings.entry(path.clone()).or_insert_with(|| Arc::new(binding.clone()));
                    if prior.as_ref() == binding {
                        continue;
                    }
                }
                broad.insert(path.clone());
                deps.bindings.remove(path);
            }
        }
        deps.files.retain(|path| path != file);
        deps.files.sort();
        deps.files.dedup();
        deps
    }

    fn resolve_module(
        &self,
        resolver: &ResolverGeneric<S>,
        file: &Path,
        request: &str,
        found: &mut Vec<PathBuf>,
    ) -> bool {
        let resolution = resolver.resolve_file(file, request);
        if let Err(ResolveError::Builtin { .. }) = resolution {
            return true;
        }
        let bare = package_name(request);
        if let Ok(resolution) = &resolution
            && is_followed(&self.root, resolution.path())
        {
            found.push(resolution.path().to_path_buf());
            // Package exports and package imports depend on the manifest selecting the implementation.
            if (request.starts_with('#')
                || resolution.package_json().is_some_and(|pkg| bare.is_some() && pkg.name() == bare))
                && let Some(package) = resolution.package_json()
                && is_followed(&self.root, package.realpath())
            {
                found.push(package.realpath().to_path_buf());
            }
        }
        // An installed package, or one that does not resolve: the dependency its nearest manifest declares.
        if let Some(name) = bare
            && let Some(manifest) = self.manifests.declaring(&self.source, &self.root, file, name)
        {
            found.push(manifest::dependency_node(&manifest, name));
            return true;
        }
        resolution.is_ok()
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

    fn resolve_glob(&self, dir: &Path, pattern: &str, excluded: &[String], found: &mut Vec<PathBuf>) {
        let patterns = [pattern.to_owned()];
        let Ok(globs) = tsconfig::glob_set(dir, &patterns, false) else { return };
        let base = tsconfig::static_prefix(dir, &patterns);
        if !is_followed(&self.root, &base) {
            return;
        }
        let Ok(exclude) = tsconfig::glob_set(dir, excluded, false) else { return };
        found.extend(
            self.source.files_under(&base).into_iter().filter(|path| globs.is_match(path) && !exclude.is_match(path)),
        );
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
