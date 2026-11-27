//! The `affected` command line without Node: flags, `affected.config.json`, `--base auto`
//! and report rendering, compatible with the Node CLI.

pub mod args;
mod base;
pub mod config;
pub mod render;

use std::path::{Path, PathBuf};

use affected_core::{Entry, Options, TraceEdge};
pub use args::{Args, Changed, Command, EntrySpec, USAGE, is_glob};
pub use base::detect_base;
pub use config::Config;

/// Why the CLI stops; both exit 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The command line is wrong; print the usage too.
    Usage(String),
    /// Input files, git or the analysis failed.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

/// Process state the options depend on, injected so they can be built without the real process.
pub struct Context<'a> {
    pub cwd: PathBuf,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// Reads all of stdin; called at most once, for the first `-c -`.
    pub stdin: &'a mut dyn FnMut() -> std::io::Result<String>,
}

impl std::fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context").field("cwd", &self.cwd).finish_non_exhaustive()
    }
}

/// Merges the config with the flags into analysis options.
///
/// Flags win: `--entry` replaces a config entry of the same name (inputs included) or adds
/// one, `--input` and `--global` add to the config's, and `--base`, `--strict` and
/// `--types`/`--no-types` override its defaults.
///
/// # Errors
///
/// [`Error::Usage`] for missing entries or changes and unknown `--input` entries;
/// [`Error::Failed`] when stdin or the trace file cannot be read.
pub fn options(args: &Args, config: Option<Config>, context: Context<'_>) -> Result<Options, Error> {
    let config = config.unwrap_or_default();
    let mut entries = config.entries;
    let mut set = |name: String, paths: Vec<String>| {
        let entry = config::EntryConfig { paths, inputs: Vec::new() };
        match entries.iter_mut().find(|(existing, _)| *existing == name) {
            Some((_, existing)) => *existing = entry,
            None => entries.push((name, entry)),
        }
    };
    let expand = |glob: &str| {
        affected_core::expand(&context.cwd, &[glob.to_owned()], &[]).map_err(|error| Error::Failed(error.to_string()))
    };
    for spec in &args.entries {
        match spec {
            EntrySpec::Glob(glob) => {
                let files = expand(glob)?;
                if files.is_empty() {
                    return Err(Error::Usage(format!("--entry {glob} matches no files")));
                }
                for file in files {
                    set(file.clone(), vec![file]);
                }
            }
            EntrySpec::Named(name, list) => {
                let mut paths = Vec::new();
                for path in list.split(',') {
                    if is_glob(path) { paths.extend(expand(path)?) } else { paths.push(path.to_owned()) }
                }
                if paths.is_empty() {
                    return Err(Error::Usage(format!("--entry {name}={list} matches no files")));
                }
                set(name.clone(), paths);
            }
        }
    }
    if entries.is_empty() {
        return Err(Error::Usage("pass at least one --entry".into()));
    }
    for (name, glob) in &args.inputs {
        let Some((_, entry)) = entries.iter_mut().find(|(existing, _)| existing == name) else {
            return Err(Error::Usage(format!("--input names unknown entry \"{name}\"")));
        };
        entry.inputs.push(glob.clone());
    }

    let changed = match &args.changed {
        Some(changed) => Some(changed_files(changed, context.stdin)?),
        None => None,
    };
    let base = args.base.clone().or(config.base).or_else(|| changed.is_none().then(|| "auto".to_owned()));

    let mut globals = config.globals;
    globals.extend(args.globals.iter().cloned());
    let mut options = Options::new(context.cwd)
        .uncommitted(!args.committed)
        .types(args.types.or(config.types).unwrap_or(true))
        .globals(globals);
    options.strict = args.strict || config.strict.unwrap_or(false);
    if let Some(trace) = &args.trace {
        options.trace = trace_edges(&options.cwd.join(trace))?;
    }
    for (name, entry) in entries {
        if !entry.inputs.is_empty() {
            options = options.inputs(name.clone(), entry.inputs);
        }
        options = options.entry(name, Entry::from_paths(entry.paths.into_iter().map(PathBuf::from).collect()));
    }
    if let Some(base) = base {
        options = options.base(if base == "auto" { detect_base(context.env) } else { base });
    }
    if let Some(changed) = changed {
        options = options.changed(changed);
    }
    Ok(options)
}

/// Expands `-` into stdin's nonempty lines; a second `-` reads an exhausted stdin.
fn changed_files(
    changed: &[Changed],
    stdin: &mut dyn FnMut() -> std::io::Result<String>,
) -> Result<Vec<PathBuf>, Error> {
    let mut files = Vec::new();
    let mut stdin_read = false;
    for item in changed {
        match item {
            Changed::Path(path) => files.push(path.clone()),
            Changed::Stdin if stdin_read => {}
            Changed::Stdin => {
                stdin_read = true;
                let text = stdin().map_err(|error| Error::Failed(format!("stdin: {error}")))?;
                let lines = text.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line));
                files.extend(lines.filter(|line| !line.is_empty()).map(PathBuf::from));
            }
        }
    }
    Ok(files)
}

/// Reads `[{ from, to, revision? }]`, rejecting anything else as the Node CLI does.
fn trace_edges(path: &Path) -> Result<Vec<TraceEdge>, Error> {
    let text = std::fs::read_to_string(path).map_err(|error| Error::Failed(format!("{}: {error}", path.display())))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| Error::Failed(format!("{}: {error}", path.display())))?;
    let invalid = || Error::Failed("Expected trace array of {from, to, revision?}".into());
    let edges = value.as_array().ok_or_else(invalid)?;
    edges
        .iter()
        .map(|edge| {
            let field = |key: &str| edge.get(key).and_then(serde_json::Value::as_str);
            let revision = match edge.get("revision") {
                None => None,
                Some(serde_json::Value::String(revision)) if revision == "head" || revision == "base" => {
                    Some(revision.clone())
                }
                Some(_) => return Err(invalid()),
            };
            match (field("from"), field("to")) {
                (Some(from), Some(to)) => Ok(TraceEdge { from: from.into(), to: to.into(), revision }),
                _ => Err(invalid()),
            }
        })
        .collect()
}
