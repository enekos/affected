#![expect(clippy::unwrap_used, reason = "fixture helpers fail the test on any setup error")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use affected_core::{Entry, EntryReport, Options, analyze};
use tempfile::TempDir;

struct Repo {
    dir: TempDir,
}

impl Repo {
    fn new(files: &[(&str, &str)]) -> Self {
        let repo = Self { dir: tempfile::tempdir().unwrap() };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "test@localhost"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.write(files);
        repo.commit("base");
        repo
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn git(&self, args: &[&str]) {
        let status = Command::new("git").arg("-C").arg(self.path()).args(args).status().unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn write(&self, files: &[(&str, &str)]) {
        for (path, contents) in files {
            let full = self.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, contents).unwrap();
        }
    }

    fn remove(&self, path: &str) {
        fs::remove_file(self.path().join(path)).unwrap();
    }

    fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-qm", message]);
    }

    fn branch(&self, name: &str) {
        self.git(&["switch", "-qc", name]);
    }

    fn analyze(&self, entry: Entry) -> EntryReport {
        let options = Options::new(self.path()).entry("app", entry).base("main");
        analyze(&options).unwrap().entries.remove(0)
    }
}

fn tsconfig(include: &str) -> String {
    format!(
        r#"{{ "compilerOptions": {{ "strict": true, }}, // comment
        "include": [{include}] }}"#
    )
}

fn config(path: &str) -> Entry {
    Entry::Tsconfig(PathBuf::from(path))
}

#[test]
fn a_deep_dependency_change_hits_with_the_shortest_chain() {
    let repo = Repo::new(&[
        ("app/tsconfig.json", &tsconfig(r#""index.ts""#)),
        ("app/index.ts", "import { a } from './a.js'\nexport const x = a"),
        ("app/a.ts", "export { b as a } from './lib/b'"),
        ("app/lib/b.ts", "import { c } from '../../shared/c'\nexport const b = c"),
        ("shared/c.ts", "export const c = 1"),
        ("shared/unrelated.ts", "export const u = 1"),
    ]);
    repo.branch("change");
    repo.write(&[("shared/c.ts", "export const c = 2"), ("shared/unrelated.ts", "export const u = 2")]);

    let report = repo.analyze(config("app/tsconfig.json"));
    assert_eq!(report.hits, ["shared/c.ts"]);
    assert_eq!(report.chain, ["app/index.ts", "app/a.ts", "app/lib/b.ts", "shared/c.ts"]);
    assert!(report.files.contains(&"app/tsconfig.json".to_owned()));
    assert!(!report.files.contains(&"shared/unrelated.ts".to_owned()));
}

#[test]
fn unrelated_changes_do_not_hit() {
    let repo = Repo::new(&[
        ("app/tsconfig.json", &tsconfig(r#""src""#)),
        ("app/src/index.ts", "export const x = 1"),
        ("other/index.ts", "export const y = 1"),
    ]);
    repo.write(&[("other/index.ts", "export const y = 2")]);
    let report = repo.analyze(config("app/tsconfig.json"));
    assert!(!report.is_hit());
    assert_eq!(report.files, ["app/src/index.ts", "app/tsconfig.json"]);
}

#[test]
fn an_import_removed_by_the_change_still_counts() {
    let repo = Repo::new(&[
        ("tsconfig.json", r#"{ "files": ["index.ts"] }"#),
        ("index.ts", "import './old'"),
        ("old.ts", "export {}"),
    ]);
    repo.branch("change");
    repo.write(&[("index.ts", "import './new'"), ("new.ts", "export {}")]);
    repo.remove("old.ts");
    repo.commit("swap helper");

    let report = repo.analyze(config("tsconfig.json"));
    assert_eq!(report.hits, ["index.ts", "new.ts", "old.ts"]);
    assert_eq!(report.chain, ["index.ts"]);
}

#[test]
fn a_deleted_file_reached_only_at_the_fork_point_hits() {
    let repo = Repo::new(&[
        ("tsconfig.json", r#"{ "files": ["index.ts"] }"#),
        ("index.ts", "import './helper'"),
        ("helper.ts", "import './gone'"),
        ("gone.ts", "export {}"),
    ]);
    repo.branch("change");
    repo.write(&[("helper.ts", "export {}")]);
    repo.remove("gone.ts");
    repo.commit("drop");
    let report = repo.analyze(config("tsconfig.json"));
    assert_eq!(report.hits, ["gone.ts", "helper.ts"]);
    assert_eq!(report.chain, ["index.ts", "helper.ts"]);
}

#[test]
fn follows_tsconfig_paths_and_package_imports_with_custom_conditions() {
    let repo = Repo::new(&[
        (
            "tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@lib/*": ["lib/*"] },
                 "customConditions": ["source"], "module": "nodenext", "moduleResolution": "nodenext" },
                 "files": ["index.ts"] }"#,
        ),
        (
            "package.json",
            r##"{ "type": "module", "imports": { "#util/*": { "source": "./util/*.ts", "default": "./dist/*.js" } } }"##,
        ),
        ("index.ts", "import { a } from '@lib/a'\nimport { u } from '#util/u'\nexport const x = a + u"),
        ("lib/a.ts", "export const a = 1"),
        ("util/u.ts", "export const u = 1"),
    ]);
    repo.write(&[("util/u.ts", "export const u = 2"), ("lib/a.ts", "export const a = 2")]);
    let report = repo.analyze(config("tsconfig.json"));
    assert_eq!(report.hits, ["lib/a.ts", "util/u.ts"]);
}

#[test]
fn type_only_imports_are_optional() {
    let repo = Repo::new(&[
        ("tsconfig.json", r#"{ "files": ["index.ts"] }"#),
        ("index.ts", "import type { T } from './types'\nexport const x: T = 1"),
        ("types.ts", "export type T = number"),
    ]);
    repo.write(&[("types.ts", "export type T = 1")]);
    assert!(repo.analyze(config("tsconfig.json")).is_hit());
    let options = Options::new(repo.path()).entry("app", config("tsconfig.json")).base("main").types(false);
    assert!(!analyze(&options).unwrap().entries[0].is_hit());
}

#[test]
fn a_config_change_hits_through_the_extends_chain() {
    let repo = Repo::new(&[
        ("tsconfig.base.json", r#"{ "compilerOptions": { "strict": true } }"#),
        ("app/tsconfig.json", r#"{ "extends": "../tsconfig.base", "include": ["*.ts"] }"#),
        ("app/index.ts", "export {}"),
    ]);
    repo.write(&[("tsconfig.base.json", r#"{ "compilerOptions": { "strict": false } }"#)]);
    let report = repo.analyze(config("app/tsconfig.json"));
    assert_eq!(report.hits, ["tsconfig.base.json"]);
    assert_eq!(report.chain, ["tsconfig.base.json"]);
}

#[test]
fn file_entries_see_uncommitted_and_untracked_changes() {
    let repo = Repo::new(&[("src/main.ts", "export {}"), ("src/a.ts", "export {}")]);
    repo.write(&[("src/main.ts", "import './a'\nimport './fresh'"), ("src/fresh.ts", "export {}")]);
    let report = repo.analyze(Entry::Files(vec!["src/main.ts".into()]));
    assert_eq!(report.hits, ["src/fresh.ts", "src/main.ts"]);
    let committed = Options::new(repo.path())
        .entry("app", Entry::Files(vec!["src/main.ts".into()]))
        .base("main")
        .uncommitted(false);
    assert!(!analyze(&committed).unwrap().entries[0].is_hit());
}

#[test]
fn packages_in_node_modules_are_not_followed() {
    let repo = Repo::new(&[
        (".gitignore", "node_modules\n"),
        ("index.ts", "import { x } from 'dep'\nimport { readFileSync } from 'node:fs'\nexport const y = x"),
        ("node_modules/dep/package.json", r#"{ "name": "dep", "main": "index.js" }"#),
        ("node_modules/dep/index.js", "exports.x = 1"),
    ]);
    let report = repo.analyze(Entry::Files(vec!["index.ts".into()]));
    assert_eq!(report.files, ["index.ts"]);
}

#[test]
fn an_entry_added_by_the_change_has_no_fork_point_graph() {
    let repo = Repo::new(&[("readme.md", "hi")]);
    repo.branch("change");
    repo.write(&[("tsconfig.json", r#"{ "files": ["index.ts"] }"#), ("index.ts", "export {}")]);
    repo.commit("add app");
    assert_eq!(repo.analyze(config("tsconfig.json")).hits, ["index.ts", "tsconfig.json"]);
}

#[test]
fn explicit_changes_work_without_git() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("index.ts"), "import './a'").unwrap();
    fs::write(dir.path().join("a.ts"), "export {}").unwrap();
    let options =
        Options::new(dir.path()).entry("app", Entry::Files(vec!["index.ts".into()])).changed(vec!["a.ts".into()]);
    let report = analyze(&options).unwrap();
    assert_eq!(report.changed, ["a.ts"]);
    assert_eq!(report.entries[0].chain, ["index.ts", "a.ts"]);
}

#[test]
fn reports_missing_inputs() {
    let repo = Repo::new(&[("index.ts", "export {}")]);
    let missing = Options::new(repo.path()).entry("app", config("nope/tsconfig.json")).base("main");
    assert!(matches!(analyze(&missing), Err(affected_core::Error::EntryNotFound(_))));
    let no_changes = Options::new(repo.path()).entry("app", Entry::Files(vec!["index.ts".into()]));
    assert!(matches!(analyze(&no_changes), Err(affected_core::Error::NoChanges)));
}
