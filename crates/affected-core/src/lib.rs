//! Which TypeScript entry points does a diff reach?
//!
//! Builds the import graph of each entry on the working tree and, when a git base is
//! given, on the fork point as well, read straight from the object database. A changed
//! file affects an entry when either graph reaches it, so imports that were added or
//! removed count too.

mod git;
mod graph;
mod imports;
mod source;
mod tsconfig;

use std::path::{Component, Path, PathBuf};

use git::Git;
use graph::{Graph, ParseCache};
use source::{Disk, GitTree};

/// Where an entry's import graph starts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Entry {
    /// A tsconfig: its `files` and `include`, resolved with its `paths` and conditions.
    Tsconfig(PathBuf),
    /// Source files, each resolved with its nearest tsconfig.
    Files(Vec<PathBuf>),
}

impl Entry {
    /// A single `.json` path is a tsconfig; anything else is a list of source files.
    #[must_use]
    pub fn from_paths(mut paths: Vec<PathBuf>) -> Self {
        match paths.as_slice() {
            [single] if single.extension().is_some_and(|extension| extension == "json") => {
                Self::Tsconfig(paths.swap_remove(0))
            }
            _ => Self::Files(paths),
        }
    }

    fn absolute(self, cwd: &Path) -> Self {
        match self {
            Self::Tsconfig(path) => Self::Tsconfig(absolute(cwd, &path)),
            Self::Files(files) => Self::Files(files.iter().map(|file| absolute(cwd, file)).collect()),
        }
    }
}

/// What to analyse. Paths are relative to `cwd` unless absolute.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Options {
    pub cwd: PathBuf,
    pub entries: Vec<(String, Entry)>,
    /// Compare against the fork point of this ref; changed files come from git unless given.
    pub base: Option<String>,
    /// The changed files. Required without `base`.
    pub changed: Option<Vec<PathBuf>>,
    /// With `base`, also count uncommitted and untracked changes.
    pub uncommitted: bool,
    /// Follow type-only imports.
    pub types: bool,
}

impl Options {
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self { cwd: cwd.into(), entries: Vec::new(), base: None, changed: None, uncommitted: true, types: true }
    }

    #[must_use]
    pub fn entry(mut self, name: impl Into<String>, entry: Entry) -> Self {
        self.entries.push((name.into(), entry));
        self
    }

    #[must_use]
    pub fn base(mut self, base: impl Into<String>) -> Self {
        self.base = Some(base.into());
        self
    }

    #[must_use]
    pub fn changed(mut self, changed: Vec<PathBuf>) -> Self {
        self.changed = Some(changed);
        self
    }

    #[must_use]
    pub fn uncommitted(mut self, uncommitted: bool) -> Self {
        self.uncommitted = uncommitted;
        self
    }

    #[must_use]
    pub fn types(mut self, types: bool) -> Self {
        self.types = types;
        self
    }
}

/// The outcome for one entry. Paths are relative to `cwd`, with `/` separators.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EntryReport {
    pub name: String,
    /// Every file the entry reaches on either revision, sorted.
    pub files: Vec<String>,
    /// The changed files the entry reaches, sorted.
    pub hits: Vec<String>,
    /// The shortest import chain from an entry root to a changed file; empty when not hit.
    pub chain: Vec<String>,
}

impl EntryReport {
    #[must_use]
    pub fn is_hit(&self) -> bool {
        !self.hits.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Report {
    /// The changed files considered, sorted.
    pub changed: Vec<String>,
    pub entries: Vec<EntryReport>,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("pass `base` or `changed`")]
    NoChanges,
    #[error("`base` needs a git repository at {0}")]
    NotARepository(PathBuf),
    #[error("entry not found: {0}")]
    EntryNotFound(PathBuf),
    #[error("invalid tsconfig {path}: {message}")]
    Tsconfig { path: PathBuf, message: String },
    #[error("git {command} failed: {stderr}")]
    Git { command: String, stderr: String },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

/// Builds each entry's graph and reports which entries the changed files reach.
///
/// # Errors
///
/// Fails when neither `base` nor `changed` is given, an entry or tsconfig is missing or
/// invalid, or a git command fails.
pub fn analyze(options: &Options) -> Result<Report, Error> {
    let cwd = dunce::canonicalize(&options.cwd).map_err(|source| Error::Io { path: options.cwd.clone(), source })?;
    let git = Git::discover(&cwd);
    let root = git.as_ref().map_or_else(|| cwd.clone(), |git| git.root().to_path_buf());
    let entries: Vec<(&str, Entry)> =
        options.entries.iter().map(|(name, entry)| (name.as_str(), entry.clone().absolute(&cwd))).collect();

    let fork_point = match (&options.base, &git) {
        (Some(base), Some(git)) => Some(git.merge_base(base)?),
        (Some(_), None) => return Err(Error::NotARepository(cwd)),
        (None, _) => None,
    };
    let mut changed: Vec<PathBuf> = match (&options.changed, &fork_point, &git) {
        (Some(changed), _, _) => changed.iter().map(|path| absolute(&cwd, path)).collect(),
        (None, Some(commit), Some(git)) => git.changed_since(commit, options.uncommitted)?,
        _ => return Err(Error::NoChanges),
    };
    changed.sort();
    changed.dedup();

    let disk = Disk::new();
    let cache = ParseCache::default();
    let base_tree = match (&fork_point, &git) {
        (Some(commit), Some(git)) => {
            let differs = if options.changed.is_none() && options.uncommitted {
                changed.iter().cloned().collect()
            } else {
                git.changed_since(commit, true)?.into_iter().collect()
            };
            Some(GitTree::load(git, commit, differs)?)
        }
        _ => None,
    };

    let mut reports = Vec::with_capacity(entries.len());
    for (name, entry) in &entries {
        let (head, base) = rayon::join(
            || graph::build(&disk, &root, entry, options.types, &cache),
            || base_tree.as_ref().map(|tree| build_base(tree, &root, entry, options.types, &cache)).transpose(),
        );
        reports.push(report(name, &cwd, &changed, &head?, base?.as_ref()));
    }
    Ok(Report { changed: changed.iter().map(|path| relative(&cwd, path)).collect(), entries: reports })
}

/// An entry that does not exist at the fork point was added by the change: an empty graph.
fn build_base(tree: &GitTree, root: &Path, entry: &Entry, types: bool, cache: &ParseCache) -> Result<Graph, Error> {
    match graph::build(tree, root, entry, types, cache) {
        Err(Error::EntryNotFound(_)) => Ok(Graph::default()),
        other => other,
    }
}

fn report(name: &str, cwd: &Path, changed: &[PathBuf], head: &Graph, base: Option<&Graph>) -> EntryReport {
    let graphs: Vec<&Graph> = std::iter::once(head).chain(base).collect();
    let mut files: Vec<&PathBuf> = graphs.iter().flat_map(|graph| graph.files.iter()).collect();
    files.sort();
    files.dedup();
    let hits: Vec<&PathBuf> = changed.iter().filter(|path| graphs.iter().any(|graph| graph.contains(path))).collect();
    let chain = hits
        .iter()
        .flat_map(|path| graphs.iter().filter_map(|graph| graph.chain(path)))
        .min_by_key(Vec::len)
        .unwrap_or_default();
    EntryReport {
        name: name.to_owned(),
        files: files.iter().map(|path| relative(cwd, path)).collect(),
        hits: hits.iter().map(|path| relative(cwd, path)).collect(),
        chain: chain.iter().map(|path| relative(cwd, path)).collect(),
    }
}

fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    tsconfig::normalize(&cwd.join(path))
}

/// `path` relative to `cwd`, climbing with `..` when it lies outside, `/`-separated.
fn relative(cwd: &Path, path: &Path) -> String {
    let base: Vec<Component<'_>> = cwd.components().collect();
    let target: Vec<Component<'_>> = path.components().collect();
    let shared = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let ups = base.len().saturating_sub(shared);
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), ups).collect();
    parts.extend(target.iter().skip(shared).map(|part| part.as_os_str().to_string_lossy().into_owned()));
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_climb_out_of_cwd() {
        assert_eq!(relative(Path::new("/repo/pkg"), Path::new("/repo/pkg/src/a.ts")), "src/a.ts");
        assert_eq!(relative(Path::new("/repo/pkg"), Path::new("/repo/server/a.ts")), "../server/a.ts");
    }

    #[test]
    fn a_single_json_path_is_a_tsconfig() {
        assert_eq!(Entry::from_paths(vec!["tsconfig.json".into()]), Entry::Tsconfig("tsconfig.json".into()));
        assert_eq!(Entry::from_paths(vec!["a.ts".into()]), Entry::Files(vec!["a.ts".into()]));
    }
}
