//! Command-line flags, the same as the Node CLI's.

use std::ffi::OsString;
use std::path::PathBuf;

use lexopt::prelude::*;

use crate::Error;

pub const USAGE: &str = "Usage: affected --entry <name>=<tsconfig.json | file,file…> [options]

Options:
  --entry, -e <name>=<paths>  Entry to check: a tsconfig.json, or files and globs, comma-separated
  --entry, -e <glob>          One entry per matching file, named by its path; repeat for several
  --base, -b <ref>            Compare against the fork point of <ref> (default: auto, the CI target branch)
  --changed, -c <file>        A changed file; repeat for several, or pass - to read lines from stdin
  --committed                 With --base, ignore uncommitted and untracked files
  --no-types                  Do not follow `import type`
  --global, -g <glob>         A changed file matching <glob> hits every entry; repeat for several
  --input, -i <name>=<glob>   A changed file matching <glob> hits entry <name>; repeat for several
  --strict                    Select entries with incomplete analysis when any files changed
  --trace <file>              Read traced edges [{from,to,revision?}] from JSON
  --config <file>             Read entries, globals, base, strict and types from JSON (default: ./affected.config.json)
  --graph mermaid             Print the impact diagram as fenced Markdown
  --json                      Print the full report as JSON
  --names                     Print the names of the hit entries, one per line
  --github-output             Append affected, any, files, matrix, count and total to $GITHUB_OUTPUT
  --exit-code                 Exit 0 when an entry is hit, 1 when none is
  --help, -h                  Show this help
  --version, -V               Print the version

--shadow-out and --compare-shadow are only in the Node CLI (npx @enekos/affected).";

/// What the command line asks for.
#[derive(Debug)]
pub enum Command {
    Help,
    Version,
    Run(Args),
}

/// One changed-file argument: a path, or `-` for lines on stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Changed {
    Path(PathBuf),
    Stdin,
}

/// One `--entry` flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntrySpec {
    /// `name=a,b`, unsplit; each path may be a glob.
    Named(String, String),
    /// A bare glob: one entry per matching file, named by its path.
    Glob(String),
}

/// Whether `path` is a glob rather than a literal path.
#[must_use]
pub fn is_glob(path: &str) -> bool {
    path.contains(['*', '?', '[', ']', '{', '}'])
}

#[derive(Debug, Default)]
pub struct Args {
    /// `--entry` flags, in order.
    pub entries: Vec<EntrySpec>,
    pub base: Option<String>,
    pub changed: Option<Vec<Changed>>,
    pub committed: bool,
    /// `--types` or `--no-types`; the last one wins.
    pub types: Option<bool>,
    pub strict: bool,
    pub trace: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub globals: Vec<String>,
    /// `--input name=glob`, in order.
    pub inputs: Vec<(String, String)>,
    pub json: bool,
    pub names: bool,
    pub mermaid: bool,
    pub github_output: bool,
    pub exit_code: bool,
}

/// Splits `name=value` at the first `=`; the name must not be empty.
fn pair(spec: String, flag: &str) -> Result<(String, String), Error> {
    match spec.split_once('=') {
        Some((name, value)) if !name.is_empty() => Ok((name.to_owned(), value.to_owned())),
        _ => Err(Error::Usage(format!("expected {flag} <name>=<value>, got \"{spec}\""))),
    }
}

/// Parses the arguments after the program name.
///
/// # Errors
///
/// [`Error::Usage`] for unknown flags, missing values, positionals, and Node-only flags.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, Error> {
    let usage = |error: lexopt::Error| Error::Usage(error.to_string());
    let mut parser = lexopt::Parser::from_args(args);
    let mut args = Args::default();
    let mut help = false;
    let mut version = false;
    while let Some(arg) = parser.next().map_err(usage)? {
        match arg {
            Short('e') | Long("entry") => {
                let spec = parser.value().map_err(usage)?.string().map_err(usage)?;
                args.entries.push(if !spec.contains('=') && is_glob(&spec) {
                    EntrySpec::Glob(spec)
                } else {
                    let (name, paths) = pair(spec, "--entry")?;
                    EntrySpec::Named(name, paths)
                });
            }
            Short('b') | Long("base") => args.base = Some(parser.value().map_err(usage)?.string().map_err(usage)?),
            Short('c') | Long("changed") => {
                let value = parser.value().map_err(usage)?;
                let changed = if value == "-" { Changed::Stdin } else { Changed::Path(value.into()) };
                args.changed.get_or_insert_with(Vec::new).push(changed);
            }
            Long("committed") => args.committed = true,
            Long("types") => args.types = Some(true),
            Long("no-types") => args.types = Some(false),
            Long("strict") => args.strict = true,
            Long("trace") => args.trace = Some(parser.value().map_err(usage)?.into()),
            Long("config") => args.config = Some(parser.value().map_err(usage)?.into()),
            Short('g') | Long("global") => args.globals.push(parser.value().map_err(usage)?.string().map_err(usage)?),
            Short('i') | Long("input") => {
                args.inputs.push(pair(parser.value().map_err(usage)?.string().map_err(usage)?, "--input")?);
            }
            Long("graph") => {
                if parser.value().map_err(usage)? != "mermaid" {
                    return Err(Error::Usage("expected --graph mermaid".into()));
                }
                args.mermaid = true;
            }
            Long("json") => args.json = true,
            Long("names") => args.names = true,
            Long("github-output") => args.github_output = true,
            Long("exit-code") => args.exit_code = true,
            Short('h') | Long("help") => help = true,
            Short('V') | Long("version") => version = true,
            Long(flag @ ("shadow-out" | "compare-shadow" | "results")) => {
                return Err(Error::Usage(format!("--{flag} is only in the Node CLI (npx @enekos/affected)")));
            }
            _ => return Err(usage(arg.unexpected())),
        }
    }
    if args.mermaid && (args.json || args.names) {
        return Err(Error::Usage("--graph cannot be combined with other output formats".into()));
    }
    Ok(if help {
        Command::Help
    } else if version {
        Command::Version
    } else {
        Command::Run(args)
    })
}

#[cfg(test)]
mod tests {
    use super::{Args, Changed, Command, EntrySpec, parse};
    use crate::Error;

    fn run(args: &[&str]) -> Result<Args, Error> {
        match parse(args.iter().map(Into::into))? {
            Command::Run(args) => Ok(args),
            other => panic!("expected a run, got {other:?}"),
        }
    }

    #[test]
    fn short_long_and_inline_values() {
        let args = run(&["-e", "a=x.ts,y.ts", "--entry=b=t.json", "-btrunk", "-c", "-", "--changed", "z.ts"]).unwrap();
        assert_eq!(
            args.entries,
            [EntrySpec::Named("a".into(), "x.ts,y.ts".into()), EntrySpec::Named("b".into(), "t.json".into())]
        );
        let globs = run(&["-e", "src/**/*.test.ts", "-e", "x=src/*.ts"]).unwrap();
        assert_eq!(
            globs.entries,
            [EntrySpec::Glob("src/**/*.test.ts".into()), EntrySpec::Named("x".into(), "src/*.ts".into())]
        );
        assert_eq!(args.base.as_deref(), Some("trunk"));
        assert_eq!(args.changed, Some(vec![Changed::Stdin, Changed::Path("z.ts".into())]));
    }

    #[test]
    fn the_last_types_flag_wins() {
        assert_eq!(run(&["--no-types"]).unwrap().types, Some(false));
        assert_eq!(run(&["--no-types", "--types"]).unwrap().types, Some(true));
        assert_eq!(run(&[]).unwrap().types, None);
    }

    #[test]
    fn bad_input_is_a_usage_error() {
        for args in
            [&["--entry", "=x"][..], &["-e", "x"], &["--nope"], &["stray"], &["-e"], &["--shadow-out", "p.json"]]
        {
            assert!(matches!(run(args), Err(Error::Usage(_))), "{args:?}");
        }
    }

    #[test]
    fn help_wins_over_everything_else() {
        assert!(matches!(parse(["-V".into(), "--help".into()]), Ok(Command::Help)));
    }
}
