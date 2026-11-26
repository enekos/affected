# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Before 1.0, minor versions may break the API.

## [Unreleased]

## [0.1.0] - unreleased

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
