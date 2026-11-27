use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, PoisonError};

use crate::Error;

/// Upper bound for one blob read through `cat-file`; larger files are not source code.
const MAX_BLOB_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct Git {
    root: PathBuf,
}

pub(crate) struct TreeBlob {
    pub path: PathBuf,
    pub oid: Box<str>,
    pub link: bool,
}

impl Git {
    /// The repository containing `cwd`, or `None` outside a work tree.
    pub(crate) fn discover(cwd: &Path) -> Option<Self> {
        let output = Command::new("git").arg("-C").arg(cwd).args(["rev-parse", "--show-toplevel"]).output().ok()?;
        if !output.status.success() {
            return None;
        }
        let root = String::from_utf8(output.stdout).ok()?;
        let root = dunce::canonicalize(root.trim_end()).ok()?;
        Some(Self { root })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    fn run(&self, args: &[&str]) -> Result<Vec<u8>, Error> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .map_err(|source| Error::Io { path: PathBuf::from("git"), source })?;
        if !output.status.success() {
            return Err(Error::Git {
                command: args.join(" "),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(output.stdout)
    }

    fn lines(bytes: &[u8]) -> impl Iterator<Item = &str> {
        bytes.split(|byte| *byte == 0).filter(|part| !part.is_empty()).filter_map(|part| std::str::from_utf8(part).ok())
    }

    /// The commit both `base` and `HEAD` descend from: the point a branch forked.
    pub(crate) fn merge_base(&self, base: &str) -> Result<String, Error> {
        match self.run(&["merge-base", base, "HEAD"]) {
            Ok(output) => Ok(String::from_utf8_lossy(&output).trim().to_owned()),
            Err(error) => {
                let shallow =
                    self.run(&["rev-parse", "--is-shallow-repository"]).is_ok_and(|out| out.starts_with(b"true"));
                Err(if shallow { Error::ShallowClone(base.to_owned()) } else { error })
            }
        }
    }

    /// Paths changed between `commit` and the working tree, plus untracked files.
    pub(crate) fn changed_since(&self, commit: &str, uncommitted: bool) -> Result<Vec<PathBuf>, Error> {
        let mut changed: Vec<PathBuf> = if uncommitted {
            let diff = self.run(&["diff", "--name-only", "--no-renames", "-z", commit, "--"])?;
            let untracked = self.run(&["ls-files", "-z", "--others", "--exclude-standard"])?;
            Self::lines(&diff).chain(Self::lines(&untracked)).map(|line| self.root.join(line)).collect()
        } else {
            let diff = self.run(&["diff", "--name-only", "--no-renames", "-z", commit, "HEAD", "--"])?;
            Self::lines(&diff).map(|line| self.root.join(line)).collect()
        };
        changed.sort();
        changed.dedup();
        Ok(changed)
    }

    /// Tracked and untracked files under `dir` that git does not ignore, as absolute paths.
    pub(crate) fn files(&self, dir: &Path) -> Result<Vec<PathBuf>, Error> {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--full-name", "--", "."])
            .output()
            .map_err(|source| Error::Io { path: PathBuf::from("git"), source })?;
        if !output.status.success() {
            return Err(Error::Git {
                command: "ls-files".to_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        let mut files: Vec<PathBuf> = Self::lines(&output.stdout).map(|line| self.root.join(line)).collect();
        files.sort();
        files.dedup();
        Ok(files)
    }

    /// Every blob in `revision` as (repository-relative path, object id).
    pub(crate) fn ls_tree(&self, revision: &str) -> Result<Vec<TreeBlob>, Error> {
        let output = self.run(&["ls-tree", "-r", "-z", "--full-tree", revision])?;
        let mut blobs = Vec::new();
        for line in Self::lines(&output) {
            let Some((meta, path)) = line.split_once('\t') else { continue };
            let mut fields = meta.split(' ');
            let (Some(mode), Some(kind), Some(object)) = (fields.next(), fields.next(), fields.next()) else {
                continue;
            };
            if kind == "blob" {
                blobs.push(TreeBlob { path: path.into(), oid: object.into(), link: mode == "120000" });
            }
        }
        Ok(blobs)
    }

    /// One `cat-file --batch` slot per parsing thread, so blob reads never queue.
    pub(crate) fn cat_file(&self) -> CatFile {
        let workers = rayon::current_num_threads().max(1);
        CatFile { root: self.root.clone(), pipes: (0..workers).map(|_| Mutex::new(None)).collect() }
    }
}

impl Pipe {
    fn spawn(root: &Path) -> io::Result<Self> {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(io::Error::other("git cat-file has no pipes"));
        };
        Ok(Self { stdin, stdout: BufReader::new(stdout), child })
    }
}

struct Pipe {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    child: Child,
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // kill() fails only when the process already exited; wait() reaps it either way.
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// Long-lived `git cat-file --batch` processes, one per rayon worker, started on first use.
pub(crate) struct CatFile {
    root: PathBuf,
    pipes: Vec<Mutex<Option<Pipe>>>,
}

impl CatFile {
    /// The contents of one object, read through the calling worker's own process.
    pub(crate) fn read(&self, object: &str) -> io::Result<Vec<u8>> {
        let slot = rayon::current_thread_index().unwrap_or(0) % self.pipes.len().max(1);
        let pipe = self.pipes.get(slot).ok_or_else(|| io::Error::other("no cat-file process"))?;
        // Poison only means a reader panicked mid-response; the next read then fails with an error.
        let mut slot = pipe.lock().unwrap_or_else(PoisonError::into_inner);
        let pipe = match &mut *slot {
            Some(pipe) => pipe,
            empty => empty.insert(Pipe::spawn(&self.root)?),
        };
        writeln!(pipe.stdin, "{object}")?;
        pipe.stdin.flush()?;
        let mut header = String::new();
        pipe.stdout.read_line(&mut header)?;
        let size = header
            .trim_end()
            .rsplit(' ')
            .next()
            .and_then(|size| size.parse::<usize>().ok())
            .filter(|size| *size <= MAX_BLOB_BYTES)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("cat-file: {}", header.trim_end())))?;
        let mut contents = vec![0; size];
        pipe.stdout.read_exact(&mut contents)?;
        let mut newline = [0; 1];
        pipe.stdout.read_exact(&mut newline)?;
        Ok(contents)
    }
}
