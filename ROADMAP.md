# Roadmap

`affected` answers one question: which named entry points can a diff change? A wrong "no" ships an
untested change, while a wrong "yes" only costs CI time. So correctness comes before reach, and reach
comes before speed. That rule still holds. This roadmap adds a second one: **the tool has to prove
itself on the user's own repo before they trust it.** Nobody turns on test skipping because of a README.

## Where it stands (2026-11-27)

- 0.1.0 on npm, 7 prebuilt targets, ~170 downloads in its first week, 0 stars, 0 known adopters.
- Head graph matches `tsc --listFilesOnly` on 32 tsconfigs across three production monorepos. Agrees with
  esbuild on 735/735 date-fns entry decisions.
- Unreleased on main: per-entry uncertainty and `--strict`, traced edges (`@vercel/nft`), shadow comparison
  against Jest/Vitest results, transitive npm lockfile selection.
- The engine is ahead of the product. Using it today means writing entries by hand, piping names into a
  test runner and wiring a CI `if`. Every item below either removes a step from that path or gives a new
  audience a reason to start on it.

## The bet

Test selection is the wedge: CI minutes are a line item, and Vitest `--changed` / Jest
`--findRelatedTests` only see the head graph, so they miss removed imports, deleted files, lockfile bumps
and assets. `affected` sees both revisions. The blocker is fear of a missed failure, so the plan is:

1. **One line to try** (0.2). A GitHub Action and a `tests` command that need no entry list.
2. **Proof on your own repo** (0.3). Shadow mode becomes a savings-and-safety report, posted on the PR.
3. **New audiences** (0.4–0.5). AI coding agents, editors, visual/E2E tests, deploys and LLM evals, all on
   the same graph.

## 0.2 — one line to try

Goal: a repo with Vitest or Jest goes from zero to selective tests in one copy-pasted step.

- [x] **`affected tests`.** Read the Vitest/Jest config (`include`, `exclude`, `projects`, `setupFiles`,
      `globalSetup`), make one entry per test file, and treat setup files as globals. No entry list.
- [x] **Runner output.** `--format vitest|jest|playwright|paths|json`. `affected tests --format vitest`
      prints args for `vitest run`; an empty selection prints a flag that runs nothing and exits 0.
- [x] **`affected [tests] -- <cmd> {files}`.** Substitute the selection into a command, and run it unfiltered
      when the selection is uncertain or the base is missing. Still not a task runner: one command, no
      cache.
- [x] **GitHub Action `enekos/affected@v0`.** In this repository, ready for a Marketplace listing; handles `fetch-depth`, sets
      `affected`, `files` and `matrix` outputs, writes the job summary. Most CI tools are found on the
      Marketplace, not on npm.
- [x] **Glob entries.** `-e 'src/**/*.test.ts'` makes one entry per match, named by path.
- [x] **Config file.** `affected.config.{ts,json}` for entries, globals and inputs, shared by CI, local
      runs, the Action and the MCP server. `defineConfig` keeps entry names typed.
- [x] **pnpm and Yarn lockfiles.** Precise comparisons for pnpm v6/v9 and Yarn Berry, so a lockfile
      change selects the importers whose dependency closure changed. Yarn classic and Bun still select
      everything.
- [x] **Standalone binary.** Release assets for static Linux, macOS and Windows, `cargo binstall --git` metadata and a Homebrew formula template (the tap and a crates.io name are open), so Bun,
      Deno and non-Node CI images work. Settle the crate name first (`affected-core` is taken).

Ship check: on date-fns, Nano ID and one JOIN repo, the README's two-line CI snippet works unedited.

## 0.3 — prove it on the user's repo

Goal: before skipping a single test, the user sees "over the last N runs, selection would have saved X
minutes and missed 0 failures".

- [ ] **Shadow by default in the Action.** `mode: shadow` runs the full suite, records what affected would
      have picked, and compares it with the runner's JSON report. Switching to `mode: select` is one line.
- [ ] **PR comment: blast radius.** Hit entries, the shortest chain per entry, uncertain entries with
      their reason, and in shadow mode the time that would have been skipped. Reviewers want this before
      CI does.
- [ ] **Savings and safety ledger.** Append each shadow result to a JSON artifact or a branch file. The
      job summary shows trailing precision, recall and minutes saved. "0 misses over 50 runs" is the
      number that gets the flag flipped.
- [ ] **Reorder instead of skip.** `--order` runs hit tests first, closest chain first, then the rest. It
      cuts time to first failure and cannot miss anything. This is the mode for teams that won't skip yet.
- [ ] **Safety-net presets.** Select on PRs, run everything on the merge queue and on `main`. When a later
      full run fails a test the selection skipped, record the miss with its chain.
- [ ] **Selection Bench.** A public, reproducible benchmark that replays real commits from popular OSS
      repos (date-fns, Vite, Zod, tRPC, Nano ID, …) and scores affected, `vitest --changed`,
      `jest --findRelatedTests`, Nx and Turbo on recall against the tests that actually failed and on time
      saved. Publish it on the docs site. It doubles as the regression suite: every miss is a bug report.
- [ ] **Explain.** `--explain <entry>` shows every chain, not only the shortest, and `--graph
      dot|mermaid|json` exports the graph.

## 0.4 — agents and editors

Coding agents edit code and then guess what to test: the whole suite, which is slow, or nothing, which is
wrong. `affected` gives them the exact set and the reason.

- [ ] **MCP server** (`affected mcp`). Tools: `affected_tests(changed?)`, `why(entry)`,
      `blast_radius(file)` (which entries reach a file, asked before editing it) and `entries()`. Uses the
      config file when one exists.
- [ ] **Agent recipes.** A Claude Code / Codex hook that runs the affected tests after edits and before
      "done", plus an `AGENTS.md` snippet.
- [ ] **Daemon and watch mode.** Keep both graphs warm, update on file events, answer in single-digit ms,
      so agents and editors can ask after every edit.
- [ ] **Pre-commit/pre-push.** `affected tests --uncommitted` recipes for lefthook and husky.
- [ ] **VS Code extension.** A status bar showing "N tests affected by your changes", one-click run, and a
      "what reaches this file" view from `blast_radius`.

## 0.5 — beyond unit tests

The same graph decides more than which unit tests to run. Each item brings a new audience.

- [ ] **Stories (open TurboSnap).** Each Storybook story file is an entry; output the affected stories for
      Chromatic, Lost Pixel, Playwright CT or Loki. Chromatic charges for this, and nothing open does it at
      file precision for every bundler.
- [ ] **E2E through observed edges.** Unit tests import app code; Playwright specs don't. Learn
      spec-to-source edges from V8 coverage of a nightly full run, store them as traced edges (the format
      already on main), and select specs from them. Expire learned edges per file blob, and run everything
      for files with no observation.
- [ ] **Deploy selection.** Entries per service or worker (`bin`, Dockerfile `COPY` inputs, serverless
      handlers), with `--format matrix` for "deploy only what changed". Pairs with preview environments.
- [ ] **LLM eval selection.** Where this tool started: prompt catalogs, tool schemas and fixtures as
      entries, so eval suites run only when a prompt can change. A guide plus Promptfoo, Langfuse and
      Braintrust recipes; evals cost more per run than tests.
- [ ] **Route entries.** Next.js `app/`, Remix, SvelteKit and Astro routes as auto-entries, so the PR
      comment can say "changes /checkout and /cart".

## 1.0 — monorepo scale and stability

- [ ] **Workspace auto-entries** from `exports`/`main`/`bin` and tsconfig, with Turbo-style selectors
      (`...pkg` for dependents).
- [ ] **Nx and Turborepo plugins** that add file-level precision inside a package. They stay the task
      layer; `affected` replaces their project-granular file check.
- [ ] **Persistent parse cache keyed by blob OID**, shared by head and base and across CI runs through
      `actions/cache`.
- [ ] **Path interning** (`u32` node ids). The 1,183-entry case spends most of its time hashing paths.
- [ ] **Stable API and output schemas** (`--json`, shadow and ledger files) under semver, so the Action,
      plugins and third-party tools can depend on them.

## Correctness backlog (alongside every release)

Missing evidence is "unknown", never "unaffected". These items gate the "missed 0" claim in 0.3.

- [ ] Diagnose path-casing mismatches on case-insensitive filesystems.
- [ ] Submodules: a gitlink change counts as everything below it changed.
- [ ] Blobless clones (`filter: blob:none`): prefetch the base blobs in one batch.
- [ ] Resolve workspaces from the base revision's `pnpm-workspace.yaml` / `workspaces`, not from head's
      `node_modules`.
- [ ] Bundler aliases outside tsconfig (`vite.config`, webpack `resolve.alias`), read statically.
- [ ] `compilerOptions.types` and ambient `.d.ts` files as inputs of every root.
- [ ] CSS `@import`/`url()`, Sass `@use`, GraphQL `#import`, HTML `<script src>`.
- [ ] Yarn PnP.

## How to measure

| Signal | Now | 0.3 target | 1.0 target |
|---|---|---|---|
| npm weekly downloads | ~170 | 2,000 | 20,000 |
| Repos using the Action | 0 | 25 | 500 |
| Selection Bench recall (vs. real failures) | not measured | ≥ 99.5% | ≥ 99.9% |
| Median CI test time saved (shadow ledgers) | not measured | 50% | 70% |
| External issues and PRs | 0 | 10 | 100 |

## Launch moments

Each release ships one thing worth sharing:

- **0.2:** "Cut Vitest CI time with one line", with a before/after on a known OSS repo.
- **0.3:** the Selection Bench as a public leaderboard, honest about where affected misses.
- **0.4:** an agent demo: an edit, the affected tests, a red test and the import chain that explains it.
- **0.5:** "Open-source TurboSnap" for Storybook users.

## Decisions to make

- **Name and scope.** `@enekos/affected` reads as a personal project. Options: an `@affected/*` npm org,
  or keep the scope and lead with the Action name. Decide before 0.2, because the Action and the MCP
  package fix the name in place.
- **Where `exec` ends.** Substituting one command is in scope. Retries, caching and task graphs are not.

## Non-goals

- **Task orchestration or output caching.** That's Nx, Turbo and moon. `affected` feeds them.
- **Type-level precision** (`tsc --build` signature skipping). Over-selecting is the safe side.
- **Languages beyond JS/TS** until 1.0. Depth beats breadth while trust is being built.

## Inspiration

Nx affected (inputs, lockfile awareness, `--explain`), Turborepo (`--affected`, SCM base env vars),
jest/vitest related-tests, Chromatic TurboSnap (story selection), target-determinator and bazel-diff
(both revisions, fail closed), Pants `--changed-dependents`, predictive test selection services (shadow
evidence, reorder before skip), dependency-cruiser, rev-dep, knip, `@vercel/nft` (traced reads).
