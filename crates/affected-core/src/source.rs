use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oxc_resolver::{FileMetadata, FileSystem, FileSystemOs, ResolveError};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::git::{CatFile, Git};

/// A file tree the graph is built from: the working tree on disk or a git revision.
pub(crate) trait Source: FileSystem + Clone + 'static {
    /// Every file below `dir`, recursively, skipping `node_modules`.
    fn files_under(&self, dir: &Path) -> Vec<PathBuf>;

    /// Whether `path` has the same contents here as in the working tree.
    fn matches_disk(&self, path: &Path) -> bool;
}

/// The working tree.
#[derive(Clone)]
pub(crate) struct Disk(FileSystemOs);

impl Disk {
    pub(crate) fn new() -> Self {
        Self(<FileSystemOs as FileSystem>::new())
    }
}

impl FileSystem for Disk {
    fn new() -> Self {
        Self::new()
    }
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.0.read(path)
    }
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.0.read_to_string(path)
    }
    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        self.0.metadata(path)
    }
    fn symlink_metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        self.0.symlink_metadata(path)
    }
    fn read_link(&self, path: &Path) -> Result<PathBuf, ResolveError> {
        self.0.read_link(path)
    }
    /// Without the `\\?\` verbatim prefix on Windows, so resolved paths compare equal to git's.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.0.canonicalize(path).map(|canonical| dunce::simplified(&canonical).to_path_buf())
    }
}

impl Source for Disk {
    fn files_under(&self, dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(current) = pending.pop() {
            let Ok(entries) = fs::read_dir(&current) else { continue };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else { continue };
                let path = entry.path();
                if kind.is_dir() {
                    if entry.file_name() != "node_modules" && entry.file_name() != ".git" {
                        pending.push(path);
                    }
                } else if kind.is_file() {
                    files.push(path);
                }
            }
        }
        files
    }

    fn matches_disk(&self, _path: &Path) -> bool {
        true
    }
}

/// A committed tree, read straight from the object database without a checkout.
#[derive(Clone)]
pub(crate) struct GitTree(Arc<TreeIndex>);

struct TreeIndex {
    root: PathBuf,
    blobs: FxHashMap<PathBuf, Box<str>>,
    dirs: FxHashSet<PathBuf>,
    /// Paths whose working-tree contents differ from this revision; the rest are read from disk.
    differs: FxHashSet<PathBuf>,
    objects: Option<CatFile>,
}

impl GitTree {
    /// `differs` lists the paths that changed between `revision` and the working tree.
    pub(crate) fn load(git: &Git, revision: &str, differs: FxHashSet<PathBuf>) -> Result<Self, crate::Error> {
        let mut blobs = FxHashMap::default();
        let mut dirs = FxHashSet::default();
        dirs.insert(git.root().to_path_buf());
        for (path, object) in git.ls_tree(revision)? {
            let absolute = git.root().join(&path);
            let mut parent = absolute.parent();
            while let Some(dir) = parent {
                if !dirs.insert(dir.to_path_buf()) {
                    break;
                }
                parent = dir.parent();
            }
            blobs.insert(absolute, object);
        }
        let objects = Some(git.cat_file());
        Ok(Self(Arc::new(TreeIndex { root: git.root().to_path_buf(), blobs, dirs, differs, objects })))
    }

    fn not_found(path: &Path) -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, path.display().to_string())
    }
}

/// Installed packages are not in git. Paths through `node_modules` are read from the
/// working tree's install, so workspace packages and package configs still resolve; a
/// symlinked workspace package canonicalizes back into the tree.
fn is_installed(path: &Path) -> bool {
    path.components().any(|part| part.as_os_str() == "node_modules")
}

impl FileSystem for GitTree {
    fn new() -> Self {
        Self(Arc::new(TreeIndex {
            root: PathBuf::new(),
            blobs: FxHashMap::default(),
            dirs: FxHashSet::default(),
            differs: FxHashSet::default(),
            objects: None,
        }))
    }
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        if is_installed(path) {
            return fs::read(path);
        }
        let object = self.0.blobs.get(path).ok_or_else(|| Self::not_found(path))?;
        if self.matches_disk(path) {
            return fs::read(path);
        }
        let objects = self.0.objects.as_ref().ok_or_else(|| Self::not_found(path))?;
        objects.read(object)
    }
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        String::from_utf8(self.read(path)?).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
    fn metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        if is_installed(path) {
            return Disk::new().metadata(path);
        }
        if self.0.blobs.contains_key(path) {
            Ok(FileMetadata::new(true, false, false))
        } else if self.0.dirs.contains(path) {
            Ok(FileMetadata::new(false, true, false))
        } else {
            Err(Self::not_found(path))
        }
    }
    fn symlink_metadata(&self, path: &Path) -> io::Result<FileMetadata> {
        if is_installed(path) {
            return Disk::new().symlink_metadata(path);
        }
        self.metadata(path)
    }
    fn read_link(&self, path: &Path) -> Result<PathBuf, ResolveError> {
        if is_installed(path) {
            return Disk::new().read_link(path);
        }
        Err(ResolveError::from(Self::not_found(path)))
    }
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        if is_installed(path) {
            return Disk::new().canonicalize(path);
        }
        self.metadata(path).map(|_| path.to_path_buf())
    }
}

impl Source for GitTree {
    fn files_under(&self, dir: &Path) -> Vec<PathBuf> {
        if !dir.starts_with(&self.0.root) {
            return Vec::new();
        }
        self.0
            .blobs
            .keys()
            .filter(|path| path.starts_with(dir) && !path.components().any(|part| part.as_os_str() == "node_modules"))
            .cloned()
            .collect()
    }

    fn matches_disk(&self, path: &Path) -> bool {
        !self.0.differs.contains(path)
    }
}
