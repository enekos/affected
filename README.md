<h1 align="center">affected</h1>

<p align="center"><b>Which TypeScript entry points does this diff reach?</b></p>

<p align="center">
  <a href="https://www.npmjs.com/package/@enekos/affected"><img alt="npm" src="https://img.shields.io/npm/v/@enekos/affected?color=cb3837&logo=npm"></a>
  <a href="https://github.com/enekos/affected/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/enekos/affected/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://enekos.github.io/affected/"><img alt="docs" src="https://img.shields.io/badge/docs-enekos.github.io%2Faffected-blue"></a>
  <a href="https://www.npmjs.com/package/@enekos/affected"><img alt="node" src="https://img.shields.io/node/v/@enekos/affected"></a>
  <a href="LICENSE"><img alt="license" src="https://img.shields.io/github/license/enekos/affected"></a>
</p>

<p align="center">
  <a href="https://enekos.github.io/affected/">Docs</a> ·
  <a href="https://enekos.github.io/affected/guides/ci.html">CI guide</a> ·
  <a href="https://enekos.github.io/affected/guides/test-selection.html">Test selection</a> ·
  <a href="https://www.npmjs.com/package/@enekos/affected">npm</a> ·
  <a href="CHANGELOG.md">Changelog</a> ·
  <a href="ROADMAP.md">Roadmap</a>
</p>

---

```ts
import { affected } from '@enekos/affected'

const r = affected({
  entries: { jobs: 'packages/jobs-prompts/tsconfig.json', web: ['src/main.ts'] },
  base: 'auto', // the CI pull request's target branch, or any ref
})

r.hit          // ['jobs']
r.has('jobs')  // true
r.why('jobs')  // ['…/prompts/index.ts', '…/buildImport.ts', '…/constants/limits.ts']
r.has('jbos')  // ✗ type error: entry names are checked at compile time
```

A Rust import graph ([oxc](https://oxc.rs) parser and resolver), built in parallel over **both sides of
the diff**: the working tree, and the fork point read straight from git objects without a checkout. A
changed file hits an entry when either graph reaches it, so imports a PR adds *or removes* count.

- **Exact.** Lists the same files as `tsc --listFilesOnly` on every tsconfig of three production
  monorepos, plus what tsc cannot see: globs, assets read through `__dirname`, `.vue`/`.svelte` scripts,
  manifests.
- **Precise about dependencies.** Bumping `zod` in `package.json` hits the files that import `zod` —
  not everything.
- **Fast.** 6–8× faster than `tsc --listFilesOnly`; 1,183 entries in ~220 ms.

## Install

```sh
pnpm add -D @enekos/affected     # npm i -D @enekos/affected · yarn add -D @enekos/affected
```

Prebuilt for macOS (arm64, x64), Linux (x64, arm64; glibc and musl) and Windows x64. Node ≥ 20. No
postinstall scripts.

## CLI

```sh
npx affected -e jobs=packages/jobs-prompts/tsconfig.json -e web=src/main.ts -b origin/main
# jobs  ← server/jobs/constants/limits.ts  via server/jobs/prompts/index.ts → server/jobs/prompts/build.ts
# web  unaffected
```

In GitHub Actions (`fetch-depth: 0`):

```yaml
- id: affected
  run: npx -y @enekos/affected --strict --base auto --github-output -e api=apps/api/tsconfig.json -e prompts=packages/prompts/tsconfig.json
- run: pnpm eval:prompts
  if: contains(fromJSON(steps.affected.outputs.affected), 'prompts')
```

`--exit-code` (0 hit, 1 none, 2 error), `--names`, `--json`, `-c -` (changed files from stdin),
`-g <glob>` (hits every entry), `-i <entry>=<glob>` (hits one). [Full CLI reference →](https://enekos.github.io/affected/reference/cli.html)

## API

| Option | | |
|---|---|---|
| `entries` | `Record<name, string \| string[] \| { from, inputs }>` | A `tsconfig.json` (its `files` + `include` and every project it `references`, resolved with its `paths`, `extends` and `customConditions`), or source files resolved with their nearest tsconfig. `inputs` are globs whose changes hit the entry although no import reaches them. |
| `base` | `string` | Compare against the merge base of this ref and `HEAD`. `'auto'` uses `AFFECTED_BASE`, else the CI target branch (GitHub, GitLab, Buildkite, Bitbucket, Azure), else `origin/HEAD`. |
| `changed` | `string[]` | The changed files. Required without `base`; with `base`, overrides git's list. |
| `cwd` | `string` | Paths in and out are relative to it. Default `process.cwd()`. |
| `uncommitted` | `boolean` | With `base`, count uncommitted and untracked files. Default `true`. |
| `types` | `boolean` | Follow `import type` edges. Default `true`. |
| `globals` | `string[]` | Globs whose changes hit every entry, such as `.nvmrc`. |

The result has `hit`, `any`, `has(name)`, `why(name)`, `entry(name)` (`{ hit, hits, chain, files }`),
`changed` and `toString()`. `affectedAsync` runs on the libuv thread pool.
[Full API reference →](https://enekos.github.io/affected/reference/api.html)

## Checking a CI selection

Use `--strict` to include entries with unresolved imports. Each diagnostic names the file and revision. npm (v2/v3), pnpm (v6/v9) and Yarn Berry lockfiles select entries that import a changed dependency, including transitive and peer updates. Yarn classic and Bun lockfiles, unknown lockfile versions, and changes to lockfile settings, overrides or patches select every entry.

Use `--shadow-out selection.json` alongside a full test run, then `--compare-shadow selection.json --results vitest.json` to see which failing files the selector would have skipped. You can also supply observed parent–child edges from `@vercel/nft`. [Test selection guide](docs/guides/test-selection.md) · [Trace API](docs/reference/api.md)

## What counts as an edge

Static and dynamic imports, `require`, `import = require`, `import()` types, triple-slash references,
`require.resolve`, `jest`/`vi` mocks; `import.meta.glob`, `require.context` and template-literal imports as
globs (a file the PR adds under one hits); `new URL(…, import.meta.url)` and `join(__dirname, …)` assets;
`.vue`/`.svelte`/`.astro` scripts; workspace packages through their `node_modules` symlinks; and each
installed package as its version range in the declaring `package.json`.
[How it works →](https://enekos.github.io/affected/how-it-works.html)

## Speed

On a 5,166-file TypeScript service (Apple M-series, 12 threads), with the same file list as
`tsc --listFilesOnly`:

| | Time |
|---|---|
| `tsc --listFilesOnly` (TypeScript 7) | 700 ms |
| `affected`, `changed` given (working tree only) | ~110 ms |
| `affected`, `base` given (working tree + fork point) | ~170 ms |
| `affected`, a 53-file entry, `base` given | ~58 ms |
| `affected`, 1,183 entries (one per test file), `base` given | ~220 ms |

Entries share resolvers and resolved imports, so many entries cost little more than their union. The fork
point reuses the working tree's parse for every file the diff did not touch.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Report vulnerabilities privately ([SECURITY.md](SECURITY.md)).

## License

[MIT](LICENSE)
