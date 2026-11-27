---
title: How it works
nav_order: 4
---

# How it works
{: .no_toc }

1. TOC
{:toc}

## Two graphs

For each entry, `affected` builds the import graph twice:

- **Head** — the working tree on disk (including uncommitted and untracked files, unless `--committed`).
- **Base** — the merge base of `base` and `HEAD`, read straight from the object database through
  `git ls-tree` and long-lived `git cat-file --batch` processes. Nothing is checked out.

A changed file hits an entry when **either** graph reaches it. That is what makes removed imports and
deleted files count: the PR's own diff can no longer reach them, but the fork point can.

Files that the diff did not touch are parsed once and shared between both graphs. Each breadth-first
frontier is resolved in parallel, and the breadth-first parent of each file gives the shortest chain.

## Entries

| Entry | Roots | Resolution |
|---|---|---|
| `tsconfig.json` | `files` + `include` − `exclude`, merged through `extends` (relative or package), plus every project in `references`, transitively | that tsconfig's `paths`, `baseUrl` and `customConditions` |
| `['a.ts', 'b.ts']` | the files | each file's nearest tsconfig |

Every config in the `extends` chain is a node of the graph, so editing a shared base config hits.

## Edges

| Kind | Examples |
|---|---|
| Modules | `import`/`export … from`, `import()`/`require()` with a literal, `import x = require()`, `import('…')` types, `require.resolve()`, `/// <reference types>`, `jest.mock`/`vi.mock` and `requireActual`/`importActual` |
| Globs | `import.meta.glob`, `require.context`, `` import(`./locales/${l}.json`) ``, `import('./pages/' + name)` |
| Files | `/// <reference path>`, `new URL('./worker.ts', import.meta.url)`, `join(__dirname, 'prompts', 'a.md')`, `path.resolve(import.meta.dirname, …)` |
| Components | `<script>` blocks of `.vue` and `.svelte` files, Astro frontmatter |

A glob is matched against each revision's file list, so a file the PR **adds** under a glob hits. A path
that names a directory stands for everything below it, except a directory that contains the importing
file (that is a base for further path building, not an asset).

Dynamic requests with no static directory — `import(name)`, `` import(`${x}`) `` — are not edges. Declare
what they load with [`inputs`](reference/api.html#entries).

## Packages and manifests

- **Workspace packages** resolve through their `node_modules` symlink to their source in the repository
  and are followed like any other file. Their `package.json` joins the graph, so retargeting `exports`
  hits.
- **Installed packages** are leaves. Each stands for its version range in the nearest `package.json` that
  declares it — a node shown as `package.json#zod`. A diff hits that node only when the range changes,
  so bumping `zod` hits exactly the files importing `zod`, and editing `scripts` hits nothing.
- **Lockfile** changes are compared on both revisions for npm (`package-lock.json`, v2/v3), pnpm
  (`pnpm-lock.yaml`, v6 and v9) and Yarn Berry (`yarn.lock`, v2+). A dependency hits when its own
  resolution changes or anything it transitively reaches does, peer-resolved snapshots included, so a
  transitive bump hits the files importing the package that pulls it in. A change to a linked workspace
  package's resolutions also hits its `link:` dependents. Settings, overrides, patches, an unknown format
  version, Yarn classic (v1, which records no peer or workspace information) and Bun lockfiles select
  every entry.

## Limits

- The base graph reads `node_modules` from the working tree's install, not the base revision's.
- An import whose case differs from the file (`./Foo` for `foo.ts`) resolves on macOS but is not matched
  to the changed path.
- Bundler-only aliases (Vite/webpack `resolve.alias`) are not read; mirror them in tsconfig `paths`.

See the [roadmap](https://github.com/enekos/affected/blob/main/ROADMAP.md) for what is planned.
