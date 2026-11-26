---
title: Home
nav_order: 1
---

# affected
{: .fs-9 }

Which TypeScript entry points does this diff reach?
{: .fs-6 .fw-300 }

[Get started](#quick-start){: .btn .btn-primary .fs-5 .mb-4 .mb-md-0 .mr-2 }
[View on GitHub](https://github.com/enekos/affected){: .btn .fs-5 .mb-4 .mb-md-0 }

---

`affected` builds the import graph of each entry you name — a `tsconfig.json`, or a list of files — on
**both sides of a git diff**, and tells you which entries reach a changed file, with the shortest import
chain as proof. Use it to skip CI jobs, select tests, or rebuild only what changed.

```ts
import { affected } from '@enekos/affected'

const r = affected({
  entries: { api: 'apps/api/tsconfig.json', prompts: 'packages/prompts/tsconfig.json' },
  base: 'auto',
})

r.hit           // ['prompts']
r.why('prompts') // ['packages/prompts/index.ts', 'packages/prompts/build.ts', 'shared/limits.ts']
```

## Why

- **Exact.** On three production monorepos it lists the same files as `tsc --listFilesOnly` for every
  tsconfig tried — plus the assets, globs and manifests tsc does not see.
- **Both revisions.** The fork point is read straight from git objects, so imports a PR *removes* still
  count, and an entry the PR deletes still hits.
- **Fast.** A Rust graph (oxc parser + oxc resolver), built in parallel: 6–8× faster than
  `tsc --listFilesOnly` on one entry, and 1,183 entries in ~220 ms.
- **Typed.** Entry names are checked at compile time: `r.has('pormpts')` does not compile.

## Quick start

```sh
pnpm add -D @enekos/affected        # or npm i -D / yarn add -D
```

```sh
npx affected -e prompts=packages/prompts/tsconfig.json -b origin/main
# prompts  ← shared/limits.ts  via packages/prompts/index.ts → packages/prompts/build.ts
```

Next: [use it in CI](guides/ci.html), [select tests](guides/test-selection.html), or read
[how it works](how-it-works.html).
