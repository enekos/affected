#![expect(clippy::unwrap_used, reason = "fixture helpers fail the test on any setup error")]

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

/// CI variables that would steer `--base auto` or `--github-output` from the outside.
const CI_VARIABLES: &[&str] = &[
    "AFFECTED_BASE",
    "GITHUB_BASE_REF",
    "CI_MERGE_REQUEST_TARGET_BRANCH_NAME",
    "BUILDKITE_PULL_REQUEST_BASE_BRANCH",
    "BITBUCKET_PR_DESTINATION_BRANCH",
    "SYSTEM_PULLREQUEST_TARGETBRANCH",
    "GITHUB_OUTPUT",
];

/// `main` holds `app` (index → a → lib/b) and `other`; branch `feature` changes `lib/b.ts`.
struct Repo {
    dir: TempDir,
}

impl Repo {
    fn new() -> Self {
        let repo = Self { dir: tempfile::tempdir().unwrap() };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "test@localhost"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.write("app/index.ts", "import { a } from './a'\nexport const x = a\n");
        repo.write("app/a.ts", "export { b as a } from '../lib/b'\n");
        repo.write("lib/b.ts", "export const b = 1\n");
        repo.write("other/index.ts", "export const o = 1\n");
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-qm", "base"]);
        repo.git(&["switch", "-qc", "feature"]);
        repo.write("lib/b.ts", "export const b = 2\n");
        repo.git(&["commit", "-qam", "change b"]);
        repo
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) {
        let status = Command::new("git").arg("-C").arg(self.path()).args(args).status().unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn write(&self, path: &str, contents: &str) {
        let full = self.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, contents).unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_affected"));
        command.current_dir(self.path()).args(args);
        for name in CI_VARIABLES {
            command.env_remove(name);
        }
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).stdin(Stdio::null()).output().unwrap()
    }

    fn run_with_stdin(&self, args: &[&str], stdin: &str) -> Output {
        let mut child =
            self.command(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    }
}

const ENTRIES: [&str; 4] = ["-e", "app=app/index.ts", "-e", "other=other/index.ts"];

fn args<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    ENTRIES.iter().copied().chain(extra.iter().copied()).collect()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn text_output_explains_each_entry() {
    let repo = Repo::new();
    let output = repo.run(&args(&["-b", "main"]));
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "app  ← lib/b.ts  via app/index.ts → app/a.ts\nother  unaffected\n");
}

#[test]
fn json_matches_the_node_report_shape() {
    let repo = Repo::new();
    let output = repo.run(&args(&["-b", "main", "--json"]));
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.starts_with("{\n  \"changed\": [\n    \"lib/b.ts\"\n  ],\n"), "{text}");
    let report: serde_json::Value = serde_json::from_str(&text).unwrap();
    let app = &report["entries"][0];
    let keys: Vec<&str> = app.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["name", "hit", "status", "diagnostics", "reason", "revision", "traceSteps", "hits", "chain", "files"]
    );
    assert_eq!(app["status"], "affected");
    assert_eq!(app["reason"], "graph");
    assert_eq!(app["chain"], serde_json::json!(["app/index.ts", "app/a.ts", "lib/b.ts"]));
    let other = report["entries"][1].as_object().unwrap();
    assert_eq!(other["status"], "unaffected");
    assert!(!other.contains_key("reason") && !other.contains_key("revision"));
}

#[test]
fn names_lists_only_hit_entries() {
    let repo = Repo::new();
    assert_eq!(stdout(&repo.run(&args(&["-b", "main", "--names"]))), "app\n");
    assert_eq!(stdout(&repo.run(&args(&["-c", "README.md", "--names"]))), "");
}

#[test]
fn exit_code_is_0_for_a_hit_1_for_none_and_2_for_errors() {
    let repo = Repo::new();
    assert_eq!(repo.run(&args(&["-b", "main", "--exit-code"])).status.code(), Some(0));
    assert_eq!(repo.run(&args(&["-c", "README.md", "--exit-code"])).status.code(), Some(1));
    assert_eq!(repo.run(&args(&["-c", "README.md"])).status.code(), Some(0), "without --exit-code");

    let missing = repo.run(&["-e", "app=app/missing.ts", "-c", "lib/b.ts", "--exit-code"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(stderr(&missing).starts_with("affected: entry not found: "), "{}", stderr(&missing));

    let usage = repo.run(&["-c", "lib/b.ts", "--exit-code"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(stderr(&usage).starts_with("affected: pass at least one --entry\n\nUsage: affected"));

    for bad in [&["--bogus"][..], &["-e", "app"], &["-e", "app=app/index.ts"], &["--shadow-out", "plan.json"]] {
        assert_eq!(repo.run(bad).status.code(), Some(2), "{bad:?}");
    }
    let unknown_input = repo.run(&args(&["-c", "x", "-i", "nope=*.md"]));
    assert_eq!(stderr(&unknown_input).lines().next(), Some("affected: --input names unknown entry \"nope\""));
}

#[test]
fn help_and_version_exit_0() {
    let repo = Repo::new();
    let help = repo.run(&["--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(stdout(&help).starts_with("Usage: affected"));
    let version = repo.run(&["-V"]);
    assert_eq!(stdout(&version), format!("affected {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn github_output_appends_hit_names_and_any() {
    let repo = Repo::new();
    let file = repo.path().join("github-output");
    fs::write(&file, "previous=1\n").unwrap();
    let output = repo.command(&args(&["-b", "main", "--github-output"])).env("GITHUB_OUTPUT", &file).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(fs::read_to_string(&file).unwrap(), "previous=1\naffected=[\"app\"]\nany=true\n");

    let none =
        repo.command(&args(&["-c", "README.md", "--github-output"])).env("GITHUB_OUTPUT", &file).output().unwrap();
    assert_eq!(none.status.code(), Some(0));
    assert!(fs::read_to_string(&file).unwrap().ends_with("affected=[]\nany=false\n"));

    let unset = repo.run(&args(&["-b", "main", "--github-output"]));
    assert_eq!(unset.status.code(), Some(2));
    assert!(stderr(&unset).starts_with("affected: --github-output needs $GITHUB_OUTPUT"));
}

#[test]
fn changed_files_come_from_stdin() {
    let repo = Repo::new();
    let output = repo.run_with_stdin(&args(&["-c", "-", "--names"]), "README.md\r\n\nother/index.ts\n");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "other\n");

    let json = repo.run_with_stdin(&args(&["-c", "-", "-c", "lib/b.ts", "--json"]), "other/index.ts\n");
    let report: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(report["changed"], serde_json::json!(["lib/b.ts", "other/index.ts"]));

    let empty = repo.run_with_stdin(&args(&["-c", "-", "--exit-code"]), "");
    assert_eq!(empty.status.code(), Some(1));
}

#[test]
fn the_config_file_is_found_in_the_working_directory() {
    let repo = Repo::new();
    repo.write(
        "affected.config.json",
        r#"{
          "entries": {
            "other": "other/index.ts",
            "app": { "from": ["app/index.ts"], "inputs": ["docs/**/*.md"] }
          },
          "base": "main",
          "tests": { "runner": "vitest" }
        }"#,
    );
    let output = repo.run(&[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "other  unaffected\napp  ← lib/b.ts  via app/index.ts → app/a.ts\n");

    // The config's inputs and the flag's changed list.
    assert_eq!(stdout(&repo.run(&["-c", "docs/guide.md", "--names"])), "app\n");
    // A flag entry replaces the config entry of the same name, inputs included, and adds new ones.
    let output = repo.run(&["-e", "app=other/index.ts", "-e", "more=app/a.ts", "-c", "docs/guide.md", "--names"]);
    assert_eq!(stdout(&output), "", "{}", stderr(&output));
    assert_eq!(stdout(&repo.run(&["-e", "more=app/a.ts", "--names"])), "app\nmore\n");
    // Flags add globals and override the base.
    assert_eq!(stdout(&repo.run(&["-g", "*.yaml", "-c", "pnpm-lock.yaml", "--names"])), "other\napp\n");
    assert_eq!(stdout(&repo.run(&["-b", "feature", "--names"])), "");
}

#[test]
fn an_explicit_config_replaces_the_default_one() {
    let repo = Repo::new();
    repo.write("affected.config.json", r#"{ "entries": { "app": "app/index.ts" } }"#);
    repo.write("ci/affected.json", r#"{ "entries": { "other": "other/index.ts" }, "globals": ["*.lock"] }"#);
    let output = repo.run(&["--config", "ci/affected.json", "-c", "x.lock"]);
    assert_eq!(stdout(&output), "other  ← x.lock\n", "{}", stderr(&output));

    repo.write("bad.json", r#"{ "entries": {}, "global": [] }"#);
    let bad = repo.run(&["--config", "bad.json", "-c", "x"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(stderr(&bad).contains("unknown field `global`"), "{}", stderr(&bad));
    assert_eq!(repo.run(&["--config", "missing.json", "-c", "x"]).status.code(), Some(2));
}

#[test]
fn base_auto_reads_the_ci_target_branch() {
    let repo = Repo::new();
    repo.git(&["update-ref", "refs/remotes/origin/main", "main"]);
    repo.git(&["update-ref", "refs/remotes/origin/feature", "feature"]);
    let names = |env: &[(&str, &str)]| {
        let mut command = repo.command(&args(&["-b", "auto", "--names"]));
        command.envs(env.iter().copied());
        let output = command.output().unwrap();
        (output.status.code(), stdout(&output))
    };
    assert_eq!(names(&[("GITHUB_BASE_REF", "main")]), (Some(0), "app\n".into()));
    assert_eq!(names(&[("GITHUB_BASE_REF", "feature")]), (Some(0), String::new()));
    assert_eq!(names(&[("SYSTEM_PULLREQUEST_TARGETBRANCH", "refs/heads/main")]), (Some(0), "app\n".into()));
    assert_eq!(names(&[("AFFECTED_BASE", "main"), ("GITHUB_BASE_REF", "feature")]), (Some(0), "app\n".into()));
    // No CI variables: origin/HEAD, which this repository lacks until it is set.
    assert_eq!(names(&[]).0, Some(2));
    repo.git(&["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
    assert_eq!(names(&[]), (Some(0), "app\n".into()));
}
