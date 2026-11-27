---
title: JavaScript API
parent: Reference
nav_order: 1
---

# JavaScript API
{: .no_toc }

1. TOC
{:toc}

## `affected(options)` / `affectedAsync(options)`

```ts
import { affected, affectedAsync } from '@enekos/affected'
```

`affected` runs synchronously; `affectedAsync` runs on the libuv thread pool and returns a promise. Both
throw on invalid input (see [errors](#errors)).

## Options

| Option | Type | Default | |
|---|---|---|---|
| `entries` | `Record<Name, Entry>` | — | Named starting points. See [entries](#entries). |
| `base` | `string` | — | Compare against the merge base of this ref and `HEAD`; changed files come from git. `'auto'` picks the CI target branch ([detection order](../guides/ci.html#base-detection)). |
| `changed` | `string[]` | — | The changed files, relative to `cwd`. Required without `base`; with `base` it replaces git's list while the fork-point graph is still built. |
| `cwd` | `string` | `process.cwd()` | Paths in and out are relative to it. |
| `uncommitted` | `boolean` | `true` | With `base`, include uncommitted and untracked files. |
| `types` | `boolean` | `true` | Follow `import type` edges. Set `false` for runtime-only reachability. |
| `strict` | `boolean` | `false` | Include uncertain entries on a nonempty diff. |
| `trace` | `TraceEdge[]` | `[]` | Additional parent-to-child dependencies, optionally bound to a revision. |
| `globals` | `string[]` | `[]` | Globs (relative to `cwd`) whose changes hit **every** entry. |

## Selection policy and traced inputs

`strict: true` selects entries with incomplete analysis when the diff contains changes. With the default `strict: false`, inspect `entry.status` and `entry.diagnostics` before skipping an entry. Diagnostics identify the file, expression and graph revision. A definite hit remains selected even when other imports in that entry are unresolved.

npm package-lock and shrinkwrap v2/v3 changes follow resolved packages and transitive consumers on both revisions. A missing snapshot, unsupported npm format or unresolved required lockfile dependency selects every entry. Changes to pnpm, Yarn and Bun lockfiles also select every entry. A global glob for an npm lockfile explicitly selects every entry.

Pass observed dependencies as `trace: [{ from, to, revision }]`. Paths are relative to `cwd`. Use `head` or `base` to bind an observation to its snapshot. Omitting the revision applies the edge to both. `why(name)` returns the existing path array. `why(name, { details: true })` adds `reason`, `revision` and `traceSteps` (the target of each traced edge along the chain). A strict fallback has reason `unknown` and an empty chain.

```ts
import { nodeFileTrace } from '@vercel/nft'
import { affected, fromNftTrace } from '@enekos/affected'

const cwd = process.cwd()
const test = 'test/bin.test.js'
const trace = await nodeFileTrace([test], { base: cwd, processCwd: cwd })
const result = affected({ cwd, entries: { cli: test }, base: 'auto', strict: true,
  trace: fromNftTrace(trace, 'head') })
console.log(result.why('cli', { details: true }))
```

## Entries

```ts
type EntryPaths = string | readonly string[]
type Entry = EntryPaths | { from: EntryPaths; inputs?: readonly string[] }
```

- A single path ending in `.json` is a **tsconfig**: its roots are `files` + `include` and every
  referenced project, resolved with its `paths` and `customConditions`.
- Anything else is a list of **source files**, each resolved with its nearest tsconfig.
- `inputs` are globs (relative to `cwd`) whose changes hit this entry although no import reaches them.

Entry names are inferred as a union of string literals, so `has`, `why` and `entry` only accept names
you declared.

## Result

| Member | Type | |
|---|---|---|
| `hit` | `Name[]` | Hit entries, in declaration order. |
| `any` | `boolean` | Whether any entry is hit. |
| `has(name)` | `boolean` | Whether `name` is hit. |
| `why(name)` | `string[] \| undefined` | Shortest chain from an entry root to a changed file; for an input or global, just the changed file. |
| `entry(name)` | `Entry` | `{ name, hit, hits, chain, files }`. `files` is everything the entry reaches on either revision. |
| `entries` | `Entry[]` | All entries. |
| `changed` | `string[]` | The changed files considered. |
| `toString()` | `string` | One line per entry. |

`hits` and `chain` can contain dependency nodes such as `package.json#zod` — the version range of an
installed package. See [how it works](../how-it-works.html#packages-and-manifests).

## `detectBase(env?)`

Returns the ref `base: 'auto'` resolves to for the given environment (default `process.env`).

## Errors

| Message | Cause |
|---|---|
| `pass \`base\` or \`changed\`` | Neither was given. |
| `entry not found: …` | An entry path exists on neither side of the diff. |
| `no merge base with …: this is a shallow clone` | Fetch history (`fetch-depth: 0`). |
| `git merge-base … failed` | The base ref does not exist locally. |
| `invalid tsconfig …` / `invalid glob …` | Unparseable input. |
