//! `affected.config.json`: entries, globals and defaults shared by CI and local runs.
//!
//! ```json
//! {
//!   "entries": {
//!     "api": "apps/api/tsconfig.json",
//!     "web": ["src/main.ts", "src/worker.ts"],
//!     "prompts": { "from": "packages/prompts/tsconfig.json", "inputs": ["packages/prompts/**/*.md"] }
//!   },
//!   "globals": ["pnpm-lock.yaml"],
//!   "base": "auto",
//!   "strict": true,
//!   "types": true
//! }
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The file `affected` reads from the working directory when `--config` is not given.
pub const FILE_NAME: &str = "affected.config.json";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Config {
    /// In declaration order.
    pub entries: Vec<(String, EntryConfig)>,
    pub globals: Vec<String>,
    pub base: Option<String>,
    pub strict: Option<bool>,
    pub types: Option<bool>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EntryConfig {
    /// A tsconfig, or source files.
    pub paths: Vec<String>,
    /// Globs whose changes hit this entry although no import reaches them.
    pub inputs: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    entries: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    globals: Vec<String>,
    base: Option<String>,
    strict: Option<bool>,
    types: Option<bool>,
    /// Test discovery settings for the Node CLI (`{ "runner", "config" }`); accepted and ignored here.
    #[serde(default, rename = "tests")]
    _tests: Option<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawPaths {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawEntry {
    Paths(RawPaths),
    Detailed(RawDetailed),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDetailed {
    from: RawPaths,
    #[serde(default)]
    inputs: Vec<String>,
}

impl From<RawPaths> for Vec<String> {
    fn from(paths: RawPaths) -> Self {
        match paths {
            RawPaths::One(path) => vec![path],
            RawPaths::Many(paths) => paths,
        }
    }
}

impl Config {
    /// Parses a config file's text.
    ///
    /// # Errors
    ///
    /// Invalid JSON, unknown keys, or an entry that is not a path, a list of paths, or
    /// `{ "from", "inputs" }`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let raw: RawConfig = serde_json::from_str(text).map_err(|error| error.to_string())?;
        let entries = raw
            .entries
            .into_iter()
            .map(|(name, value)| {
                let entry = match serde_json::from_value(value) {
                    Ok(RawEntry::Paths(paths)) => EntryConfig { paths: paths.into(), inputs: Vec::new() },
                    Ok(RawEntry::Detailed(RawDetailed { from, inputs })) => EntryConfig { paths: from.into(), inputs },
                    Err(_) => {
                        return Err(format!(
                            "entries.{name}: expected a path, a list of paths, or {{ \"from\": …, \"inputs\": […] }}"
                        ));
                    }
                };
                Ok((name, entry))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self { entries, globals: raw.globals, base: raw.base, strict: raw.strict, types: raw.types })
    }

    /// Reads `explicit`, or `affected.config.json` in `cwd` when it exists.
    ///
    /// # Errors
    ///
    /// An explicit file that cannot be read, or any file that does not parse.
    pub fn find(explicit: Option<&Path>, cwd: &Path) -> Result<Option<Self>, String> {
        let path: PathBuf = match explicit {
            Some(path) => cwd.join(path),
            None => {
                let path = cwd.join(FILE_NAME);
                if !path.is_file() {
                    return Ok(None);
                }
                path
            }
        };
        let text = std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&text).map(Some).map_err(|error| format!("{}: {error}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, EntryConfig};

    fn entry(paths: &[&str], inputs: &[&str]) -> EntryConfig {
        EntryConfig {
            paths: paths.iter().map(|p| (*p).to_owned()).collect(),
            inputs: inputs.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    #[test]
    fn entries_keep_declaration_order_and_every_shape() {
        let config = Config::parse(
            r#"{
              "entries": {
                "web": ["src/main.ts", "src/worker.ts"],
                "api": "apps/api/tsconfig.json",
                "prompts": { "from": "packages/prompts/tsconfig.json", "inputs": ["packages/prompts/**/*.md"] },
                "bare": { "from": ["a.ts"] }
              },
              "globals": ["pnpm-lock.yaml"],
              "base": "auto",
              "strict": true,
              "types": false
            }"#,
        )
        .unwrap();
        let names: Vec<&str> = config.entries.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["web", "api", "prompts", "bare"]);
        assert_eq!(config.entries[0].1, entry(&["src/main.ts", "src/worker.ts"], &[]));
        assert_eq!(config.entries[1].1, entry(&["apps/api/tsconfig.json"], &[]));
        assert_eq!(config.entries[2].1, entry(&["packages/prompts/tsconfig.json"], &["packages/prompts/**/*.md"]));
        assert_eq!(config.entries[3].1, entry(&["a.ts"], &[]));
        assert_eq!(config.globals, ["pnpm-lock.yaml"]);
        assert_eq!((config.base.as_deref(), config.strict, config.types), (Some("auto"), Some(true), Some(false)));
    }

    #[test]
    fn every_key_is_optional() {
        assert_eq!(Config::parse("{}").unwrap(), Config::default());
    }

    #[test]
    fn the_node_only_tests_key_is_ignored() {
        let config = Config::parse(r#"{ "tests": { "runner": "vitest", "config": "vitest.config.ts" } }"#).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn typos_and_wrong_shapes_are_errors() {
        assert!(Config::parse(r#"{ "global": ["x"] }"#).unwrap_err().contains("unknown field `global`"));
        assert!(Config::parse(r#"{ "entries": { "a": 1 } }"#).unwrap_err().starts_with("entries.a: expected"));
        assert!(Config::parse(r#"{ "entries": { "a": { "form": "x" } } }"#).is_err());
        assert!(Config::parse("{ entries: {} }").is_err());
    }
}
