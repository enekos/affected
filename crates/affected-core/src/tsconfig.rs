use std::path::{Component, Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde_json::Value;

use crate::Error;
use crate::source::Source;

/// How deep an `extends` chain may go before it is treated as a cycle.
const MAX_EXTENDS_DEPTH: usize = 32;

const TS_EXTENSIONS: [&str; 5] = [".ts", ".tsx", ".d.ts", ".mts", ".cts"];
const JS_EXTENSIONS: [&str; 4] = [".js", ".jsx", ".mjs", ".cjs"];

/// How many projects a chain of `references` may pull in.
const MAX_PROJECTS: usize = 256;

/// The part of a tsconfig that decides which files a program starts from.
#[derive(Debug)]
pub(crate) struct Tsconfig {
    /// The config file itself.
    pub(crate) path: PathBuf,
    /// The config file and every config it extends, nearest first.
    pub(crate) configs: Vec<PathBuf>,
    /// The program's root files: `files` plus everything `include` matches.
    pub(crate) roots: Vec<PathBuf>,
    /// `compilerOptions.customConditions`.
    pub(crate) conditions: Vec<String>,
    /// The configs its `references` point at.
    pub(crate) references: Vec<PathBuf>,
}

/// A tsconfig and, transitively, every project it references: what `tsc --build` builds.
/// A missing referenced project is skipped.
pub(crate) fn load_projects<S: Source>(source: &S, path: &Path) -> Result<Vec<Tsconfig>, Error> {
    let mut projects = Vec::new();
    let mut seen = vec![path.to_path_buf()];
    let mut pending = vec![path.to_path_buf()];
    while let Some(config) = pending.pop() {
        let project = match load(source, &config) {
            Ok(project) => project,
            Err(Error::EntryNotFound(_)) if config != path => continue,
            Err(error) => return Err(error),
        };
        for reference in &project.references {
            if seen.len() < MAX_PROJECTS && !seen.contains(reference) {
                seen.push(reference.clone());
                pending.push(reference.clone());
            }
        }
        projects.push(project);
    }
    Ok(projects)
}

#[derive(Default)]
struct Merged {
    files: Option<(PathBuf, Vec<String>)>,
    include: Option<(PathBuf, Vec<String>)>,
    exclude: Option<(PathBuf, Vec<String>)>,
    allow_js: Option<bool>,
    conditions: Option<Vec<String>>,
    out_dir: Option<PathBuf>,
}

pub(crate) fn load<S: Source>(source: &S, path: &Path) -> Result<Tsconfig, Error> {
    let (merged, configs, references) = config_chain(source, path)?;
    roots(source, path, merged, configs, references)
}

type ConfigChain = (Merged, Vec<PathBuf>, Vec<PathBuf>);

pub(crate) fn resolution_configs<S: Source>(source: &S, path: &Path) -> Result<(Vec<PathBuf>, Vec<String>), Error> {
    let (merged, paths, _) = config_chain(source, path)?;
    Ok((paths, merged.conditions.unwrap_or_default()))
}

fn config_chain<S: Source>(source: &S, path: &Path) -> Result<ConfigChain, Error> {
    let mut merged = Merged::default();
    let mut configs = Vec::new();
    let mut references = Vec::new();
    let mut pending = vec![path.to_path_buf()];
    while let Some(config) = pending.pop() {
        if configs.contains(&config) {
            continue;
        }
        if configs.len() >= MAX_EXTENDS_DEPTH {
            return Err(Error::Tsconfig { path: path.into(), message: "extends chain exceeds limit".into() });
        }
        let value = read(source, &config)?;
        let dir = config.parent().map(Path::to_path_buf).unwrap_or_default();
        merge(&mut merged, &value, &dir);
        configs.push(config);
        if configs.len() == 1 {
            references = references_of(source, &value, &dir);
        }
        for parent in &extends(&value) {
            if parent.starts_with('.') || Path::new(parent).is_absolute() {
                let mut base = normalize(&dir.join(parent)).into_os_string();
                if !base.to_string_lossy().ends_with(".json") {
                    base.push(".json");
                }
                pending.push(PathBuf::from(base));
            } else if let Some(package) = package_config(source, &dir, parent) {
                pending.push(package);
            }
        }
    }

    Ok((merged, configs, references))
}

fn roots<S: Source>(
    source: &S,
    path: &Path,
    merged: Merged,
    configs: Vec<PathBuf>,
    references: Vec<PathBuf>,
) -> Result<Tsconfig, Error> {
    let config_dir = path.parent().unwrap_or(Path::new("/"));
    let allow_js = merged.allow_js.unwrap_or(false);
    let mut roots: Vec<PathBuf> = merged
        .files
        .as_ref()
        .map(|(dir, files)| files.iter().map(|file| normalize(&dir.join(file))).collect())
        .unwrap_or_default();
    let include = match (&merged.include, &merged.files) {
        (Some(include), _) => Some(include.clone()),
        (None, None) => Some((config_dir.to_path_buf(), vec!["**/*".to_owned()])),
        (None, Some(_)) => None,
    };
    if let Some((dir, patterns)) = include {
        let exclude = merged.exclude.clone().unwrap_or_else(|| {
            let mut defaults =
                vec!["node_modules".to_owned(), "bower_components".to_owned(), "jspm_packages".to_owned()];
            if let Some(out_dir) = &merged.out_dir {
                defaults.push(out_dir.to_string_lossy().into_owned());
            }
            (config_dir.to_path_buf(), defaults)
        });
        roots.extend(expand(source, &dir, &patterns, &exclude, allow_js)?);
    }
    roots.sort();
    roots.dedup();
    Ok(Tsconfig {
        path: path.to_path_buf(),
        configs,
        roots,
        conditions: merged.conditions.unwrap_or_default(),
        references,
    })
}

fn read<S: Source>(source: &S, path: &Path) -> Result<Value, Error> {
    let invalid = |message: String| Error::Tsconfig { path: path.to_path_buf(), message };
    let mut text = source.read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::EntryNotFound(path.to_path_buf())
        } else {
            Error::Io { path: path.to_path_buf(), source: error }
        }
    })?;
    json_strip_comments::strip(&mut text).map_err(|error| invalid(error.to_string()))?;
    serde_json::from_str(&text).map_err(|error| invalid(error.to_string()))
}

/// `references[].path`: a config file, or a directory holding `tsconfig.json`.
fn references_of<S: Source>(source: &S, value: &Value, dir: &Path) -> Vec<PathBuf> {
    let Some(references) = value.get("references").and_then(Value::as_array) else { return Vec::new() };
    references
        .iter()
        .filter_map(|reference| reference.get("path")?.as_str())
        .map(|path| {
            let target = normalize(&dir.join(path));
            if source.metadata(&target).is_ok_and(|metadata| metadata.is_dir()) {
                target.join("tsconfig.json")
            } else {
                target
            }
        })
        .collect()
}

/// An `extends` naming a package, found the way TypeScript finds it: in the nearest
/// `node_modules`, as the named file, that file plus `.json`, or a directory's
/// `tsconfig.json`. Symlinked workspace packages come back as their path in the repository.
fn package_config<S: Source>(source: &S, dir: &Path, specifier: &str) -> Option<PathBuf> {
    let is_file = |path: &Path| source.metadata(path).is_ok_and(|metadata| metadata.is_file());
    dir.ancestors().find_map(|ancestor| {
        let target = ancestor.join("node_modules").join(specifier);
        let mut with_json = target.clone().into_os_string();
        with_json.push(".json");
        let found =
            [target.clone(), PathBuf::from(with_json), target.join("tsconfig.json")].into_iter().find(|candidate| {
                candidate.extension().is_some_and(|extension| extension == "json") && is_file(candidate)
            })?;
        Some(source.canonicalize(&found).unwrap_or(found))
    })
}

fn strings(value: Option<&Value>) -> Option<Vec<String>> {
    Some(value?.as_array()?.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect())
}

fn extends(value: &Value) -> Vec<String> {
    match value.get("extends") {
        Some(Value::String(parent)) => vec![parent.clone()],
        other => strings(other).unwrap_or_default(),
    }
}

/// Fields of a nearer config win; paths stay relative to the config that declared them.
fn merge(merged: &mut Merged, value: &Value, dir: &Path) {
    let located = |key: &str| strings(value.get(key)).map(|items| (dir.to_path_buf(), items));
    merged.files = merged.files.take().or_else(|| located("files"));
    merged.include = merged.include.take().or_else(|| located("include"));
    merged.exclude = merged.exclude.take().or_else(|| located("exclude"));
    let options = value.get("compilerOptions");
    let option = |key: &str| options.and_then(|options| options.get(key));
    merged.allow_js = merged.allow_js.or_else(|| option("allowJs").and_then(Value::as_bool));
    merged.conditions = merged.conditions.take().or_else(|| strings(option("customConditions")));
    merged.out_dir =
        merged.out_dir.take().or_else(|| option("outDir").and_then(Value::as_str).map(|out| normalize(&dir.join(out))));
}

fn has_wildcard(segment: &str) -> bool {
    segment.contains(['*', '?'])
}

/// A pattern whose last segment has no wildcard and no extension names a directory.
fn pattern_glob(dir: &Path, pattern: &str) -> String {
    let joined = normalize(&dir.join(pattern));
    let mut text = glob_text(&joined);
    let last = text.rsplit('/').next().unwrap_or_default();
    if !has_wildcard(last) && !last.contains('.') {
        text.push_str("/**/*");
    }
    text
}

/// A path as glob text: `/` separators, since globset reads `\\` as an escape (candidates are normalized for us).
fn glob_text(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// tsconfig globs: `*` stays within one path segment, `**` crosses them.
fn glob(pattern: &str) -> Result<globset::Glob, globset::Error> {
    GlobBuilder::new(pattern).literal_separator(true).build()
}

pub(crate) fn glob_set(dir: &Path, patterns: &[String], directory_prefix: bool) -> Result<GlobSet, Error> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let text = pattern_glob(dir, pattern);
        let invalid = |error: globset::Error| Error::Tsconfig { path: dir.to_path_buf(), message: error.to_string() };
        builder.add(glob(&text).map_err(invalid)?);
        if directory_prefix && !has_wildcard(pattern) {
            builder.add(glob(&format!("{}/**", glob_text(&normalize(&dir.join(pattern))))).map_err(invalid)?);
        }
    }
    builder.build().map_err(|error| Error::Tsconfig { path: dir.to_path_buf(), message: error.to_string() })
}

fn expand<S: Source>(
    source: &S,
    dir: &Path,
    patterns: &[String],
    exclude: &(PathBuf, Vec<String>),
    allow_js: bool,
) -> Result<Vec<PathBuf>, Error> {
    let include = glob_set(dir, patterns, false)?;
    let exclude = glob_set(&exclude.0, &exclude.1, true)?;
    let base = static_prefix(dir, patterns);
    let supported = |path: &Path| {
        let name = path.to_string_lossy();
        TS_EXTENSIONS.iter().any(|extension| name.ends_with(extension))
            || (allow_js && JS_EXTENSIONS.iter().any(|extension| name.ends_with(extension)))
    };
    Ok(source
        .files_under(&base)
        .into_iter()
        .filter(|path| supported(path) && include.is_match(path) && !exclude.is_match(path))
        .collect())
}

/// The deepest directory every include pattern lives under, so only that subtree is listed.
pub(crate) fn static_prefix(dir: &Path, patterns: &[String]) -> PathBuf {
    let prefixes: Vec<PathBuf> = patterns
        .iter()
        .map(|pattern| {
            let full = normalize(&dir.join(pattern));
            full.ancestors()
                .find(|ancestor| !ancestor.to_string_lossy().contains(['*', '?']) && ancestor != &full)
                .map(Path::to_path_buf)
                .unwrap_or_else(|| dir.to_path_buf())
        })
        .collect();
    let Some(first) = prefixes.first() else { return dir.to_path_buf() };
    let mut common = first.clone();
    for prefix in &prefixes {
        while !prefix.starts_with(&common) {
            if !common.pop() {
                return PathBuf::from("/");
            }
        }
    }
    common
}

/// Removes `.` and folds `..` without touching the filesystem.
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_pattern_includes_its_tree() {
        assert_eq!(pattern_glob(Path::new("/repo"), "src"), "/repo/src/**/*");
        assert_eq!(pattern_glob(Path::new("/repo"), "src/**/*.ts"), "/repo/src/**/*.ts");
        assert_eq!(pattern_glob(Path::new("/repo/pkg"), "../server/index.ts"), "/repo/server/index.ts");
    }

    #[test]
    fn lists_only_the_subtree_patterns_can_match() {
        let patterns = ["src/**/*".to_owned(), "src/lib/*.ts".to_owned()];
        assert_eq!(static_prefix(Path::new("/repo"), &patterns), PathBuf::from("/repo/src"));
        let spread = ["src/**/*".to_owned(), "test/*.ts".to_owned()];
        assert_eq!(static_prefix(Path::new("/repo"), &spread), PathBuf::from("/repo"));
    }
}
