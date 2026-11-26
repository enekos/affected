# @enekos/affected

Which TypeScript entry points does this diff reach?

```ts
import { affected } from '@enekos/affected'

const r = affected({
  entries: { jobs: 'packages/jobs-prompts/tsconfig.json', web: ['src/main.ts'] },
  base: 'origin/main',
})

r.hit          // ['jobs']
r.has('jobs')  // true
r.why('jobs')  // ['…/prompts/index.ts', '…/buildImport.ts', '…/constants/limits.ts']
r.has('jbos')  // ✗ type error: entry names are checked at compile time
```

A Rust import graph (oxc parser + oxc resolver) built in parallel over **both sides of the diff**: the
working tree, and the fork point read straight from git objects without a checkout. A changed file
hits an entry when either graph reaches it, so imports a PR adds or removes count too.

## Install

```sh
pnpm add -D @enekos/affected
```

Prebuilt for macOS (arm64, x64), Linux (x64, arm64; glibc and musl) and Windows x64. Node ≥ 20.

## API

### `affected(options)` / `affectedAsync(options)`

| Option | | |
|---|---|---|
| `entries` | `Record<name, string \| string[] \| { from, inputs }>` | A `tsconfig.json` (its `files` + `include` and every project it `references`, resolved with its `paths`, `extends` and `customConditions`), or source files resolved with their nearest tsconfig. `inputs` are globs whose changes hit the entry although no import reaches them. |
| `base` | `string` | Compare against the merge base of this ref and `HEAD`. Changed files come from git. `'auto'` uses `AFFECTED_BASE`, else the CI pull request's target branch (GitHub, GitLab, Buildkite, Bitbucket, Azure), else `origin/HEAD`. |
| `changed` | `string[]` | The changed files. Required without `base`; with `base`, overrides git's list. |
| `cwd` | `string` | Paths in and out are relative to it. Default `process.cwd()`. |
| `uncommitted` | `boolean` | With `base`, count uncommitted and untracked files. Default `true`. |
| `types` | `boolean` | Follow `import type` edges. Default `true`. |
| `globals` | `string[]` | Globs whose changes hit every entry, such as `pnpm-lock.yaml` or a shared build config. |

`affectedAsync` runs on the libuv thread pool.

### Result

| | |
|---|---|
| `hit` | Names of the entries the diff reaches, in declaration order. |
| `any` | Whether any entry is hit. |
| `has(name)` | Whether `name` is hit. |
| `why(name)` | The shortest import chain from an entry root to a changed file, or `undefined`. |
| `entry(name)` | `{ name, hit, hits, chain, files }`; `files` is everything the entry reaches. |
| `changed` | The changed files considered. |
| `toString()` | One line per entry: `jobs  ← constants/limits.ts  via prompts/index.ts → buildImport.ts` |

## CLI

```sh
npx @enekos/affected -e jobs=packages/jobs-prompts/tsconfig.json -e web=src/main.ts -b origin/main
# jobs  ← server/jobs/constants/limits.ts  via server/jobs/prompts/index.ts → server/jobs/prompts/build.ts
# web  unaffected

# CI: run a step only when the prompts can have changed
if npx @enekos/affected -e jobs=packages/jobs-prompts/tsconfig.json -b origin/main --exit-code; then …; fi

git diff --name-only HEAD~1 | npx @enekos/affected -e jobs=… -c -   # changed files from stdin
```

`--json` prints the full report; `--names` prints the hit entry names one per line; `--github-output`
appends `affected=["…"]` and `any=true|false` to `$GITHUB_OUTPUT`; `--committed` ignores uncommitted work;
`--no-types` skips `import type`; `-g <glob>` adds a global input, `-i <name>=<glob>` an entry input.
Exit codes with `--exit-code`: 0 hit, 1 not hit, 2 error.

## What counts as an edge

- **Modules:** static imports and re-exports, `import()` and `require()` with a literal,
  `import x = require()`, `import('…')` types, `require.resolve()`, `/// <reference types>`, and the
  module named by `jest.mock`/`vi.mock` and their `requireActual`/`importActual` relatives.
- **Globs:** `import.meta.glob`, `require.context`, and template-literal or concatenated `import()`/`require()`
  with a static directory (`` import(`./locales/${l}.json`) `` → `./locales/*.json`). A file the diff adds
  that matches the glob hits.
- **Files and assets:** `/// <reference path>`, `new URL('./x', import.meta.url)`, and
  `join(__dirname, 'a', 'b')`/`path.resolve(import.meta.dirname, …)` with literal segments. A directory
  stands for every file below it.
- **Components:** the `<script>` blocks of `.vue` and `.svelte` files and Astro frontmatter.
- **Manifests:** a workspace package reached through `node_modules` brings its `package.json`, so
  retargeting `exports` hits. An installed package is a leaf that stands for its version range in the
  nearest `package.json` declaring it: a bump hits exactly the files importing the bumped package, and
  other `package.json` edits hit nothing. Lockfile-only changes need `globals`.
- **Configs:** every config in an entry's `extends` chain, including package configs, and every
  referenced project.

Resolution follows tsconfig `paths`/`baseUrl`, package.json `imports`/`exports` with
`customConditions`, and `.js` → `.ts` extension aliasing. Files in `node_modules` and outside the
repository are leaves. The fork-point graph reads `node_modules` from the working tree's install.

## Speed

On a 5,166-file TypeScript service (Apple M-series, 12 threads), compared with
`tsc --listFilesOnly` from TypeScript 7 on the same tsconfig. Same file list in both:

| | Time |
|---|---|
| `tsc --listFilesOnly` (TS 7) | 700 ms |
| `affected`, `changed` given (working tree only) | ~110 ms |
| `affected`, `base` given (working tree + fork point) | ~170 ms |
| `affected`, a 53-file entry, `base` given | ~58 ms |
| `affected`, 1,183 entries (one per test file), `base` given | ~220 ms |

Entries run in parallel and share resolvers and each file's resolved imports, so a large set of entries
costs little more than their union. The fork point reuses the working tree's parse for every file the diff did not touch and reads the
rest through one `git cat-file --batch` process per worker thread.

## License

MIT
