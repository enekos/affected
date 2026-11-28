#![expect(clippy::unwrap_used, reason = "fixture helpers fail the test on any setup error")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use affected_core::{Entry, EntryReport, Options, analyze, expand};
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

#[test]
fn a_solution_tsconfig_reaches_its_referenced_projects() {
    let repo = Repo::new(&[
        ("tsconfig.json", r#"{ "files": [], "references": [{ "path": "./tsconfig.app.json" }, { "path": "lib" }] }"#),
        ("tsconfig.app.json", r#"{ "include": ["src"] }"#),
        ("src/index.ts", "export {}"),
        ("lib/tsconfig.json", r#"{ "include": ["*.ts"] }"#),
        ("lib/a.ts", "export {}"),
    ]);
    repo.write(&[("lib/a.ts", "export const a = 1")]);
    let report = repo.analyze(config("tsconfig.json"));
    assert_eq!(report.hits, ["lib/a.ts"]);
    assert!(report.files.contains(&"src/index.ts".to_owned()));
}

#[test]
fn a_package_extends_is_followed_through_node_modules() {
    let repo = Repo::new(&[
        (".gitignore", "node_modules\n"),
        ("config/package.json", r#"{ "name": "@ws/config" }"#),
        ("config/base.json", r#"{ "compilerOptions": { "allowJs": true } }"#),
        ("app/tsconfig.json", r#"{ "extends": "@ws/config/base.json", "include": ["src"] }"#),
        ("app/src/index.js", "export {}"),
    ]);
    fs::create_dir_all(repo.path().join("app/node_modules/@ws")).unwrap();
    symlink_dir(&repo.path().join("config"), &repo.path().join("app/node_modules/@ws/config"));
    repo.write(&[("config/base.json", r#"{ "compilerOptions": { "allowJs": true, "strict": true } }"#)]);
    let report = repo.analyze(config("app/tsconfig.json"));
    assert_eq!(report.hits, ["config/base.json"]);
    assert!(report.files.contains(&"app/src/index.js".to_owned()));
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) {
    std::os::windows::fs::symlink_dir(target, link).unwrap();
}

#[test]
fn a_dependency_bump_hits_the_files_importing_it() {
    let repo = Repo::new(&[
        ("package.json", r#"{ "dependencies": { "zod": "^3.0.0", "left-pad": "1.0.0" } }"#),
        ("a.ts", "import { z } from 'zod'"),
        ("b.ts", "import pad from 'left-pad'"),
    ]);
    repo.write(&[("package.json", r#"{ "dependencies": { "zod": "^3.1.0", "left-pad": "1.0.0" }, "scripts": {} }"#)]);
    let options = Options::new(repo.path())
        .entry("a", Entry::Files(vec!["a.ts".into()]))
        .entry("b", Entry::Files(vec!["b.ts".into()]))
        .base("main");
    let report = analyze(&options).unwrap();
    assert_eq!(report.entries[0].hits, ["package.json#zod"]);
    assert_eq!(report.entries[0].chain, ["a.ts", "package.json#zod"]);
    assert_eq!(report.entries[0].files, ["a.ts"]);
    assert!(!report.entries[1].is_hit());
    assert_eq!(report.changed, ["package.json"]);
}

#[test]
fn globs_and_asset_paths_are_edges() {
    let repo = Repo::new(&[
        ("main.ts", "import.meta.glob('./pages/*.ts')\nreadFileSync(join(__dirname, 'prompts', 'a.md'))"),
        ("pages/a.ts", "export {}"),
        ("prompts/a.md", "hi"),
    ]);
    repo.write(&[("pages/b.ts", "export {}"), ("prompts/a.md", "hello")]);
    let report = repo.analyze(Entry::Files(vec!["main.ts".into()]));
    assert_eq!(report.hits, ["pages/b.ts", "prompts/a.md"]);
}

#[test]
fn an_entry_deleted_by_the_change_hits_through_the_fork_point() {
    let repo = Repo::new(&[("main.ts", "import './a'"), ("a.ts", "export {}")]);
    repo.branch("change");
    repo.remove("main.ts");
    repo.commit("drop entry");
    assert_eq!(repo.analyze(Entry::Files(vec!["main.ts".into()])).hits, ["main.ts"]);
}

#[test]
fn global_and_entry_inputs_hit_without_an_import() {
    let repo =
        Repo::new(&[("a.ts", "export {}"), ("b.ts", "export {}"), ("global.yml", "v1"), ("templates/a.hbs", "hi")]);
    repo.write(&[("global.yml", "v2"), ("templates/a.hbs", "hello")]);
    let entries = Options::new(repo.path())
        .entry("a", Entry::Files(vec!["a.ts".into()]))
        .entry("b", Entry::Files(vec!["b.ts".into()]))
        .base("main");
    let report = analyze(&entries.clone().inputs("a", vec!["templates/**".into()])).unwrap();
    assert_eq!(report.entries[0].hits, ["templates/a.hbs"]);
    assert!(!report.entries[1].is_hit());
    let report = analyze(&entries.globals(vec!["global.yml".into()])).unwrap();
    assert_eq!(report.entries[1].chain, ["global.yml"]);
}

#[test]
fn resolves_value_and_declaration_exports_in_the_callers_mode() {
    let repo = Repo::new(&[
        (
            "package.json",
            r#"{"name":"dual","exports":{"types":"./types.d.ts","import":"./esm.mjs","require":"./cjs.cjs"}}"#,
        ),
        ("types.d.ts", "export declare const n: number"),
        ("esm.mjs", "export const n = 1"),
        ("cjs.cjs", "exports.n = 1"),
        ("e.mjs", "import /* lazy */ ('dual')"),
        ("c.cjs", "require('dual')"),
    ]);
    let entries = Options::new(repo.path())
        .entry("esm", Entry::Files(vec!["e.mjs".into()]))
        .entry("cjs", Entry::Files(vec!["c.cjs".into()]))
        .base("main");
    let runtime = analyze(&entries.clone().types(false).changed(vec!["cjs.cjs".into()])).unwrap();
    assert!(!runtime.entries[0].is_hit());
    assert_eq!(runtime.entries[1].hits, ["cjs.cjs"]);
    let types = analyze(&entries.changed(vec!["types.d.ts".into(), "esm.mjs".into()])).unwrap();
    assert_eq!(types.entries[0].hits, ["esm.mjs", "types.d.ts"]);
    assert_eq!(types.entries[1].hits, ["types.d.ts"]);
}

#[test]
fn file_entries_track_their_config_and_last_parent_precedence() {
    let repo = Repo::new(&[
        ("tsconfig.json", r#"{"extends":["./first.json","./last.json"]}"#),
        ("first.json", r#"{"files":["a.ts"]}"#),
        ("last.json", r#"{"files":["b.ts"]}"#),
        ("a.ts", ""),
        ("b.ts", "export const value = 1"),
    ]);
    repo.write(&[("b.ts", "export const value = 2")]);
    assert_eq!(repo.analyze(config("tsconfig.json")).hits, ["b.ts"]);
    repo.write(&[("last.json", r#"{"files":["b.ts"],"compilerOptions":{"strict":true}}"#)]);
    assert!(repo.analyze(Entry::Files(vec!["a.ts".into()])).hits.contains(&"last.json".to_owned()));
}

#[test]
fn committed_graph_ignores_a_dirty_importer() {
    let repo = Repo::new(&[("entry.mjs", "import('./plugin.mjs').catch(() => {})")]);
    repo.branch("new-plugin");
    repo.write(&[("plugin.mjs", "export default 2")]);
    repo.commit("install plugin");
    repo.write(&[("entry.mjs", "export {}")]);
    let options =
        Options::new(repo.path()).entry("app", Entry::Files(vec!["entry.mjs".into()])).base("main").uncommitted(false);
    let report = analyze(&options).unwrap();
    assert_eq!(report.changed, ["plugin.mjs"]);
    assert_eq!(report.entries[0].chain, ["entry.mjs", "plugin.mjs"]);
}

#[test]
fn negative_globs_apply_only_to_their_own_call() {
    let repo = Repo::new(&[
        ("entry.ts", "import.meta.glob(['./pages/*.ts','!./pages/draft.ts'])"),
        ("pages/draft.ts", ""),
        ("pages/live.ts", ""),
    ]);
    repo.write(&[("pages/draft.ts", "export default 1")]);
    let entry = Entry::Files(vec!["entry.ts".into()]);
    assert!(!repo.analyze(entry.clone()).is_hit());
    repo.write(&[("entry.ts", "import.meta.glob(['./pages/*.ts','!./pages/draft.ts']); import './pages/draft.ts'")]);
    assert!(repo.analyze(entry).hits.contains(&"pages/draft.ts".to_owned()));
}

#[cfg(unix)]
#[test]
fn retargeting_a_link_keeps_both_implementations() {
    let repo = Repo::new(&[("main.mjs", "import './alias.mjs'"), ("a.mjs", ""), ("b.mjs", "")]);
    std::os::unix::fs::symlink("a.mjs", repo.path().join("alias.mjs")).unwrap();
    repo.commit("first target");
    repo.remove("alias.mjs");
    std::os::unix::fs::symlink("b.mjs", repo.path().join("alias.mjs")).unwrap();
    let report = repo.analyze(Entry::Files(vec!["main.mjs".into()]));
    assert_eq!(report.hits, ["alias.mjs"]);
    assert!(report.files.contains(&"a.mjs".to_owned()) && report.files.contains(&"b.mjs".to_owned()));
}

#[test]
fn npm_lockfile_follows_transitive_consumers_and_falls_back_without_a_comparable_snapshot() {
    let old = r#"{"lockfileVersion":3,"packages":{"":{"dependencies":{"a":"1","b":"1"}},
        "node_modules/a":{"version":"1","dependencies":{"@s/leaf":"*"}},"node_modules/b":{"version":"1"},
        "node_modules/@s/leaf":{"version":"1"},"node_modules/a/node_modules/@s/leaf":{"version":"2"}}}"#;
    let repo = Repo::new(&[
        ("package.json", r#"{"dependencies":{"a":"1","b":"1"}}"#),
        ("package-lock.json", old),
        ("a.mjs", "import 'a'"),
        ("b.mjs", "import 'b'"),
    ]);
    let options = Options::new(repo.path())
        .entry("a", Entry::Files(vec!["a.mjs".into()]))
        .entry("b", Entry::Files(vec!["b.mjs".into()]))
        .base("main");
    repo.write(&[("package-lock.json", &old.replace("\"version\":\"2\"", "\"version\":\"3\""))]);
    let report = analyze(&options).unwrap();
    assert_eq!(report.entries[0].chain, ["a.mjs", "package.json#a", "package-lock.json"]);
    assert!(!report.entries[1].is_hit());
    repo.write(&[("package-lock.json", "{}")]);
    assert!(analyze(&options).unwrap().entries.iter().all(EntryReport::is_hit));
    assert!(
        analyze(&options.clone().changed(vec!["yarn.lock".into()])).unwrap().entries.iter().all(EntryReport::is_hit)
    );
}

#[test]
fn expand_lists_unignored_files_relative_to_cwd() {
    let repo = Repo::new(&[
        (".gitignore", "dist/\n"),
        ("pkg/src/a.test.ts", ""),
        ("pkg/src/b.test.ts", ""),
        ("pkg/dist/a.test.ts", ""),
        ("other/c.test.ts", ""),
    ]);
    repo.write(&[("pkg/src/new.test.ts", "")]);
    let globs = |patterns: &[&str]| patterns.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>();
    let cwd = repo.path().join("pkg");
    assert_eq!(
        expand(&cwd, &globs(&["**/*.test.ts"]), &[]).unwrap(),
        ["src/a.test.ts", "src/b.test.ts", "src/new.test.ts"]
    );
    assert_eq!(
        expand(&cwd, &globs(&["src/*.ts"]), &globs(&["src/b.*"])).unwrap(),
        ["src/a.test.ts", "src/new.test.ts"]
    );
    assert!(expand(&cwd, &globs(&["["]), &[]).is_err());
}

const PNPM_V9: &str = include_str!("fixtures/pnpm-lock.v9.yaml");
const PNPM_V6: &str = include_str!("fixtures/pnpm-lock.v6.yaml");
const YARN_BERRY: &str = include_str!("fixtures/yarn.berry.lock");

/// The workspace the lockfile fixtures were installed from: the root and `packages/{a,b,c}`,
/// one entry per file importing a dependency.
fn workspace(lockfile: &str, contents: &str) -> (Repo, Options) {
    let repo = Repo::new(&[
        ("package.json", r#"{"private":true,"workspaces":["packages/*"],"devDependencies":{"is-number":"7.0.0"}}"#),
        ("packages/a/package.json", r#"{"name":"@ws/a","dependencies":{"debug":"4.3.4","@ws/b":"workspace:*"}}"#),
        ("packages/b/package.json", r#"{"name":"@ws/b","dependencies":{"is-odd":"3.0.1"}}"#),
        (
            "packages/c/package.json",
            r#"{"name":"@ws/c","dependencies":{"use-sync-external-store":"1.2.0","react":"18.2.0"}}"#,
        ),
        (lockfile, contents),
        ("root.mjs", "import 'is-number'"),
        ("packages/a/a.mjs", "import 'debug'"),
        ("packages/a/link.mjs", "import '@ws/b'"),
        ("packages/b/b.mjs", "import 'is-odd'"),
        ("packages/c/c.mjs", "import 'use-sync-external-store'"),
    ]);
    let mut options = Options::new(repo.path()).base("main");
    for (name, file) in [
        ("root", "root.mjs"),
        ("a", "packages/a/a.mjs"),
        ("link", "packages/a/link.mjs"),
        ("b", "packages/b/b.mjs"),
        ("c", "packages/c/c.mjs"),
    ] {
        options = options.entry(name, Entry::Files(vec![file.into()]));
    }
    (repo, options)
}

/// The entries selected after `lockfile` becomes `contents`.
fn selected(repo: &Repo, options: &Options, lockfile: &str, contents: &str) -> Vec<String> {
    repo.write(&[(lockfile, contents)]);
    analyze(options).unwrap().entries.into_iter().filter(EntryReport::is_hit).map(|e| e.name).collect()
}

const EVERY_ENTRY: [&str; 5] = ["root", "a", "link", "b", "c"];

#[test]
fn pnpm_v9_lockfile_selects_the_importers_reaching_a_changed_snapshot() {
    let lock = "pnpm-lock.yaml";
    let (repo, options) = workspace(lock, PNPM_V9);
    let deep = PNPM_V9.replace("ms@2.1.2", "ms@2.1.3").replace("ms: 2.1.2", "ms: 2.1.3");
    assert_eq!(selected(&repo, &options, lock, &deep), ["a"]);
    let report = analyze(&options).unwrap();
    assert_eq!(report.entries[1].chain, ["packages/a/a.mjs", "packages/a/package.json#debug", lock]);
    assert_eq!(report.changed, [lock]);
    // is-odd's dependency changes b, and a through its `link:` to b.
    assert_eq!(selected(&repo, &options, lock, &PNPM_V9.replace("sha512-Wu1VHeILBK8K", "sha512-x")), ["link", "b"]);
    // react's dependency reaches use-sync-external-store through its peer-suffixed snapshot.
    assert_eq!(selected(&repo, &options, lock, &PNPM_V9.replace("sha512-lyuxPGr/Wfhr", "sha512-x")), ["c"]);
    assert_eq!(selected(&repo, &options, lock, &PNPM_V9.replace("sha512-41Cifkg6e8Ty", "sha512-x")), ["root"]);
}

#[test]
fn pnpm_v6_lockfile_selects_transitive_consumers() {
    let lock = "pnpm-lock.yaml";
    let (repo, options) = workspace(lock, PNPM_V6);
    let deep = PNPM_V6.replace("/ms@2.1.2", "/ms@2.1.3").replace("ms: 2.1.2", "ms: 2.1.3");
    assert_eq!(selected(&repo, &options, lock, &deep), ["a"]);
    assert_eq!(selected(&repo, &options, lock, &PNPM_V6.replace("sha512-lyuxPGr/Wfhr", "sha512-x")), ["c"]);
    // A package now reached only by dev dependencies is the same package.
    assert!(
        selected(&repo, &options, lock, &PNPM_V6.replace("    dev: false\n\n  /ms", "    dev: true\n\n  /ms"))
            .is_empty()
    );
}

#[test]
fn pnpm_lockfiles_it_cannot_compare_select_every_entry() {
    let lock = "pnpm-lock.yaml";
    let (repo, options) = workspace(lock, PNPM_V9);
    for broken in [
        PNPM_V9.replace("lockfileVersion: '9.0'", "lockfileVersion: '5.4'"),
        PNPM_V9.replace("importers:", "overrides:\n  ms: 2.1.3\n\nimporters:"),
        PNPM_V9.replace("autoInstallPeers: true", "autoInstallPeers: false"),
        PNPM_V9.replace("  ms@2.1.2: {}\n", ""),
        PNPM_V9.replace("importers:", "importerz:"),
        "lockfileVersion: '9.0'\nimporters: [".to_owned(),
    ] {
        assert_eq!(selected(&repo, &options, lock, &broken), EVERY_ENTRY);
    }
    // Moving from v6 to v9 rewrites every key.
    assert_eq!(selected(&repo, &options, lock, PNPM_V6), EVERY_ENTRY);
}

#[test]
fn a_pnpm_lockfile_in_a_subdirectory_maps_importers_relative_to_it() {
    let lock = "app/pnpm-lock.yaml";
    let single = "lockfileVersion: '9.0'\n\nimporters:\n\n  .:\n    dependencies:\n      react:\n        \
                  specifier: 18.2.0\n        version: 18.2.0\n\npackages:\n\n  react@18.2.0:\n    \
                  resolution: {integrity: sha512-a}\n\nsnapshots:\n\n  react@18.2.0: {}\n";
    let manifest = r#"{"dependencies":{"react":"18.2.0"}}"#;
    let repo = Repo::new(&[
        ("package.json", manifest),
        ("app/package.json", manifest),
        (lock, single),
        ("app/index.mjs", "import 'react'"),
        ("index.mjs", "import 'react'"),
    ]);
    repo.write(&[(lock, &single.replace("sha512-a", "sha512-b"))]);
    let options = Options::new(repo.path())
        .entry("app", Entry::Files(vec!["app/index.mjs".into()]))
        .entry("root", Entry::Files(vec!["index.mjs".into()]))
        .base("main");
    let report = analyze(&options).unwrap();
    assert_eq!(report.entries[0].chain, ["app/index.mjs", "app/package.json#react", lock]);
    assert!(!report.entries[1].is_hit());
}

#[test]
fn yarn_berry_lockfile_selects_transitive_and_peer_consumers() {
    let lock = "yarn.lock";
    let (repo, options) = workspace(lock, YARN_BERRY);
    assert_eq!(selected(&repo, &options, lock, &YARN_BERRY.replace("10c0/a437714e2f90", "10c0/x")), ["a"]);
    let report = analyze(&options).unwrap();
    assert_eq!(report.entries[1].chain, ["packages/a/a.mjs", "packages/a/package.json#debug", lock]);
    assert_eq!(selected(&repo, &options, lock, &YARN_BERRY.replace("10c0/5da4c6840152", "10c0/x")), ["link", "b"]);
    // use-sync-external-store's peer on react is resolved at install time: any locked react counts.
    assert_eq!(selected(&repo, &options, lock, &YARN_BERRY.replace("10c0/655d11022098", "10c0/x")), ["c"]);
    assert_eq!(selected(&repo, &options, lock, &YARN_BERRY.replace("10c0/b4686d0d3053", "10c0/x")), ["root"]);
}

#[test]
fn yarn_lockfiles_it_cannot_compare_select_every_entry() {
    let lock = "yarn.lock";
    let (repo, options) = workspace(lock, YARN_BERRY);
    let classic = "# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.\n# yarn lockfile v1\n\n\nms@2.1.2:\n  version \"2.1.2\"\n";
    for broken in [
        YARN_BERRY.replace("cacheKey: 10c0", "cacheKey: 10c1"),
        YARN_BERRY.replace("version: 8\n", "version: 99\n"),
        YARN_BERRY.replace("    ms: \"npm:2.1.2\"\n", "    ms: \"npm:9.9.9\"\n"),
        classic.to_owned(),
    ] {
        assert_eq!(selected(&repo, &options, lock, &broken), EVERY_ENTRY);
    }
}

#[test]
fn named_barrel_consumers_follow_aliases_and_keep_opaque_edges() {
    let repo = Repo::new(&[
        ("ui/search.ts", "export class Search { title() { return 'old' } }"),
        ("ui/editor.ts", "export class Editor {}"),
        ("ui/index.ts", "export * from './search'; export * from './editor'"),
        ("alias.ts", "export { Search as Find, Editor } from './ui'"),
        ("search.spec.ts", "import { Find } from './alias'; new Find()"),
        ("editor.spec.ts", "import { Editor } from './alias'; new Editor()"),
        ("namespace.spec.ts", "import * as all from './ui'; use(all)"),
        ("side-effect.spec.ts", "import './ui'"),
        ("alternate.spec.ts", "import { Editor } from './alias'; import './bridge'"),
        ("bridge.ts", "import { Search } from './ui'; use(Search)"),
    ]);
    let options = ["search", "editor", "namespace", "side-effect", "alternate"]
        .into_iter()
        .fold(Options::new(repo.path()).types(false).base("main"), |o, name| {
            o.entry(name, Entry::Files(vec![format!("{name}.spec.ts").into()]))
        });
    repo.write(&[("ui/search.ts", "export class Search { title() { return 'new' } }")]);
    let run = || analyze(&options).unwrap().entries;
    let entries = run();
    assert_eq!(
        entries.iter().filter(|e| e.selected).map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["search", "namespace", "side-effect", "alternate"]
    );
    assert!(entries[1].files.contains(&"ui/search.ts".to_owned()));
    assert_eq!(entries[0].chain, ["search.spec.ts", "alias.ts", "ui/index.ts", "ui/search.ts"]);
    for effect in [
        "register(); export class Search {}",
        "export class Search { static value = register() }",
        "import './setup'; export class Search {}",
    ] {
        repo.write(&[("ui/search.ts", effect)]);
        assert!(run().iter().all(|e| e.selected));
    }
    // Removing module-load effects must also select all consumers.
    repo.commit("effectful");
    repo.write(&[("ui/search.ts", "export class Search {}")]);
    assert!(run().iter().all(|e| e.selected));
}

#[test]
fn runtime_bindings_union_revisions_and_widen_when_typescript_could_start_loading_a_value_import() {
    let repo = Repo::new(&[
        ("page.ts", "import { activate } from './plugin'; export function page() { return 'old'; }"),
        ("plugin.ts", "console.log('loaded'); export function activate() {}"),
        ("other.ts", "export const other = 1"),
        ("index.ts", "export * from './page'; export * from './other'"),
        ("prior.spec.ts", "import { page } from './index'; page()"),
        ("unrelated.spec.ts", "import { other } from './index'; use(other)"),
    ]);
    let options = Options::new(repo.path())
        .types(false)
        .base("main")
        .changed(vec!["page.ts".into()])
        .entry("prior", Entry::Files(vec!["prior.spec.ts".into()]))
        .entry("unrelated", Entry::Files(vec!["unrelated.spec.ts".into()]));
    repo.write(&[
        ("prior.spec.ts", "import { other } from './index'"),
        ("page.ts", "import { activate } from './plugin'; export function page() { return 'new'; }"),
    ]);
    let report = analyze(&options).unwrap();
    assert_eq!(report.entries[0].revision.as_deref(), Some("base"));
    assert!(!report.entries[1].selected);
    repo.write(&[("page.ts", "import { activate } from './plugin'; export function page() { activate(); }")]);
    assert!(analyze(&options).unwrap().entries.iter().all(|e| e.selected));
}
