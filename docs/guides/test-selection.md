---
title: Test selection
parent: Guides
nav_order: 2
---

# Run only the tests a diff reaches

Make each test file an entry. Entries share resolution work, so a thousand of them cost little more
than one — 1,183 test files resolve in ~220 ms on a 5,000-file service.

```ts
// scripts/affected-tests.ts
import { execFileSync } from 'node:child_process'
import { affected } from '@enekos/affected'

const tests = execFileSync('git', ['ls-files', '*.test.ts'], { encoding: 'utf8' }).split('\n').filter(Boolean)
const result = affected({
  entries: Object.fromEntries(tests.map(file => [file, [file]])),
  base: 'auto',
  globals: ['package.json', 'pnpm-lock.yaml', 'jest.config.*', 'tsconfig*.json'],
})
console.log(result.hit.join('\n'))
```

```sh
node scripts/affected-tests.ts | xargs --no-run-if-empty npx jest --
# or: npx vitest run $(node scripts/affected-tests.ts)
```

Why `globals`: a change to the test runner's configuration or the lockfile can break any test without
being imported by one.

## Explain a selection

```ts
for (const name of result.hit) console.log(name, '←', result.why(name)?.join(' → '))
```

`why` returns the shortest import chain from the test file to a changed file — paste it in a PR comment
when someone asks why their test ran.
