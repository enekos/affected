use std::path::{Component, Path, PathBuf};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde_json::Value;

use crate::Error;
use crate::source::Source;

/// How deep an `extends` chain may go before it is treated as a cycle.
const MAX_EXTENDS_DEPTH: usize = 32;

const TS_EXTENSIONS: [&str; 5] = [".ts", ".tsx", ".d.ts", ".mts", ".cts"];
const JS_EXTENSIONS: [&str; 4] = [".js", ".jsx", ".mjs", ".cjs"];

/// The part of a tsconfig that decides which files a program starts from.
#[derive(Debug)]
pub(crate) struct Tsconfig {
    /// The config file and every config it extends, nearest first.
    pub(crate) configs: Vec<PathBuf>,
    /// The program's root files: `files` plus everything `include` matches.
    pub(crate) roots: Vec<PathBuf>,
    /// `compilerOptions.customConditions`.
    pub(crate) conditions: Vec<String>,
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
    let mut merged = Merged::default();
    let mut configs = Vec::new();
    let mut pending = vec![path.to_path_buf()];
    while let Some(config) = pending.pop() {
        if configs.len() >= MAX_EXTENDS_DEPTH || configs.contains(&config) {
            break;
        }
        let value = read(source, &config)?;
        let dir = config.parent().map(Path::to_path_buf).unwrap_or_default();
        merge(&mut merged, &value, &dir);
        configs.push(config);
        for parent in extends(&value).iter().rev() {
            if parent.starts_with('.') || Path::new(parent).is_absolute() {
                let mut base = normalize(&dir.join(parent)).into_os_string();
                if !base.to_string_lossy().ends_with(".json") {
                    base.push(".json");
                }
                pending.push(PathBuf::from(base));
            }
        }
    }

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
    Ok(Tsconfig { configs, roots, conditions: merged.conditions.unwrap_or_default() })
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
    let mut text = joined.to_string_lossy().into_owned();
    let last = text.rsplit('/').next().unwrap_or_default();
    if !has_wildcard(last) && !last.contains('.') {
        text.push_str("/**/*");
    }
    text
}

/// tsconfig globs: `*` stays within one path segment, `**` crosses them.
fn glob(pattern: &str) -> Result<globset::Glob, globset::Error> {
    GlobBuilder::new(pattern).literal_separator(true).build()
}

fn glob_set(dir: &Path, patterns: &[String], directory_prefix: bool) -> Result<GlobSet, Error> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let text = pattern_glob(dir, pattern);
        let invalid = |error: globset::Error| Error::Tsconfig { path: dir.to_path_buf(), message: error.to_string() };
        builder.add(glob(&text).map_err(invalid)?);
        if directory_prefix && !has_wildcard(pattern) {
            builder.add(glob(&format!("{}/**", normalize(&dir.join(pattern)).display())).map_err(invalid)?);
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
fn static_prefix(dir: &Path, patterns: &[String]) -> PathBuf {
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
