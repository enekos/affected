---
title: Comparison
nav_order: 5
---

# Compared with other tools

| | affected | Nx / Turborepo `--affected` | `jest --findRelatedTests` / `vitest related` | dependency-cruiser `--affected` |
|---|---|---|---|---|
| Unit | any file set or tsconfig | project / package | test file | module |
| Graph | import graph, both revisions | project graph + file inputs | test runner's module graph | import graph, head only |
| Removed imports / deleted files | yes (fork-point graph) | via project inputs | no | no |
| Dependency bumps | per imported package | lockfile-aware (Nx `auto`) | no | no |
| Globs, assets, `__dirname` reads | yes | via declared inputs | Vite transforms (vitest) | no |
| Why | shortest import chain | `--explain` (Nx) | no | `--reaches` |
| Needs | Node ≥ 20 (prebuilt) | the build system | the test runner | Node |

Use **Nx or Turborepo** when you want task orchestration and caching at package granularity — `affected`
does not run tasks. Use **affected** when the unit you care about is smaller than a package (a prompt
catalog, a test file, a worker entry), when you need the *why*, or when you have no build system at all.
They also combine: feed `affected --names` into `turbo run --filter`.
