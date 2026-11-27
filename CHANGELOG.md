# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor versions may break the API.

## [Unreleased]

- `affected tests` asks Vitest (3+) or Jest which test files and setup files it would run, and makes one entry per test file. Setup files are extra roots of each entry; runner configs are globals.
- A command after `--` runs with the selection: `{files}` and `{names}` expand to it, an empty selection skips it, and a failed analysis runs it on everything. Strict selection is the default with `tests` and with a command.
- `--format paths|vitest|jest|playwright` prints the hit files, or runner arguments that run exactly them or nothing. `--json` and `--names` remain as short forms.
- Glob entries: `-e 'src/**/*.test.ts'` makes one entry per file git does not ignore; globs also expand inside `-e name=…`. `globEntries()` and `expandGlobs()` in the API.
- `affected.config.{ts,mts,js,mjs,json}` holds entries, globals, base, strict, types and test discovery; `defineConfig` keeps entry names typed. Flags replace same-named entries and add to globals.
- GitHub Action `enekos/affected@v0`: fetches the base and deepens shallow checkouts, runs a command on the selection, sets outputs and writes a job summary. Releases move the `v0` tag.
- `--github-output` also writes `files`, `matrix`, `count` and `total`; `--github-summary` writes a Markdown table of hits and chains.
- `--base` defaults to `auto` when `--changed` is not given.

- Added per-entry uncertainty diagnostics, opt-in strict selection, traced dependency edges with a Node File Trace adapter, and shadow comparison against full Vitest or Jest JSON results.
- npm v2/v3 lockfile changes select transitive consumers. Missing or unsupported lockfile information selects all entries.
- pnpm v6/v9 and Yarn Berry lockfile changes select transitive consumers, following pnpm peer-suffixed snapshots and Yarn peer dependencies. Yarn classic and Bun lockfiles, unknown versions, and settings, override or patch changes still select all entries.
- Fixed runtime and declaration export conditions, dynamic import comments, file-entry tsconfig inputs, config parent precedence, tracked symlinks, committed snapshots, module-relative CLI paths and Vite glob exclusions.
- CLI option errors exit 2. CI examples propagate analysis errors.
- Standalone `affected` binary (Rust crate `affected-ts`) for CI without Node: static Linux musl, macOS and Windows builds on each GitHub release with sha256 files, `cargo binstall` metadata and a Homebrew formula template. Same flags and output as the Node CLI except shadow mode; reads `affected.config.json` and adds `--version`.

## [0.1.0] - 2026-11-26

First release.

### Added

- `affected()` / `affectedAsync()` and the `affected` CLI: which named entries (a tsconfig or source
  files) a git diff reaches, with the shortest import chain.
- Import graphs on both sides of the diff; the fork point is read from git objects without a checkout.
- Edges: static and dynamic imports, `require`, `import = require`, `import()` types, triple-slash
  references, `require.resolve`, `jest`/`vi` mocks, `import.meta.glob`, `require.context`, template-literal
  imports as globs, `new URL(…, import.meta.url)` and `join(__dirname, …)` assets, `.vue`/`.svelte`/`.astro`
  scripts.
- tsconfig `paths`, `baseUrl`, `customConditions`, relative and package `extends`, `references`.
- `package.json` dependency nodes: a version bump hits only the files importing that package.
- `globals` and per-entry `inputs`; `base: 'auto'` for GitHub, GitLab, Buildkite, Bitbucket and Azure.
- CLI `--json`, `--names`, `--github-output`, `--exit-code`, stdin changed lists.
- Prebuilt binaries for macOS (arm64, x64), Linux (x64, arm64; glibc and musl) and Windows x64.

[Unreleased]: https://github.com/enekos/affected/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/enekos/affected/releases/tag/v0.1.0
