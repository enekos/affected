# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor versions may break the API.

## [Unreleased]

- Added per-entry uncertainty diagnostics, opt-in strict selection, traced dependency edges with a Node File Trace adapter, and shadow comparison against full Vitest or Jest JSON results.
- npm v2/v3 lockfile changes select transitive consumers. Missing or unsupported lockfile information selects all entries.
- pnpm v6/v9 and Yarn Berry lockfile changes select transitive consumers, following pnpm peer-suffixed snapshots and Yarn peer dependencies. Yarn classic and Bun lockfiles, unknown versions, and settings, override or patch changes still select all entries.
- Fixed runtime and declaration export conditions, dynamic import comments, file-entry tsconfig inputs, config parent precedence, tracked symlinks, committed snapshots, module-relative CLI paths and Vite glob exclusions.
- CLI option errors exit 2. CI examples propagate analysis errors.

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
