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
| `entries` | `Record<name, string \| string[]>` | A `tsconfig.json` (its `files` + `include`, resolved with its `paths`, `extends` and `customConditions`), or source files resolved with their nearest tsconfig. |
| `base` | `string` | Compare against the merge base of this ref and `HEAD`. Changed files come from git. |
| `changed` | `string[]` | The changed files. Required without `base`; with `base`, overrides git's list. |
| `cwd` | `string` | Paths in and out are relative to it. Default `process.cwd()`. |
| `uncommitted` | `boolean` | With `base`, count uncommitted and untracked files. Default `true`. |
| `types` | `boolean` | Follow `import type` edges. Default `true`. |

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

`--json` prints the full report; `--committed` ignores uncommitted work; `--no-types` skips `import type`.
Exit codes with `--exit-code`: 0 hit, 1 not hit, 2 error.

## What counts as an edge

Static imports and re-exports, `import()` and `require()` with a literal, `import x = require()`, and
`import('…')` types. Resolution follows tsconfig `paths`/`baseUrl`, package.json `imports`/`exports`
with `customConditions`, and `.js` → `.ts` extension aliasing. Files in `node_modules` and outside the
repository are leaves. A changed tsconfig in an entry's `extends` chain hits that entry.

## Speed

On a 5,166-file TypeScript service (Apple M-series, 12 threads), compared with
`tsc --listFilesOnly` from TypeScript 7 on the same tsconfig. Same file list in both:

| | Time |
|---|---|
| `tsc --listFilesOnly` (TS 7) | 700 ms |
| `affected`, `changed` given (working tree only) | ~110 ms |
| `affected`, `base` given (working tree + fork point) | ~170 ms |
| `affected`, a 53-file entry, `base` given | ~58 ms |

The fork point reuses the working tree's parse for every file the diff did not touch and reads the
rest through one `git cat-file --batch` process per worker thread.

## License

MIT
