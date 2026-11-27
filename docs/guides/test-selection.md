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
import { execFileSync, spawnSync } from 'node:child_process'
import { affected } from '@enekos/affected'

const tests = execFileSync('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard', '*.test.ts'], { encoding: 'utf8' }).split('\0').filter(Boolean)
const result = affected({
  entries: Object.fromEntries(tests.map(file => [file, [file]])),
  base: 'auto',
  strict: true,
  globals: ['jest.config.*', 'tsconfig*.json'],
})
if (result.any) {
  const run = spawnSync('pnpm', ['exec', 'jest', '--runTestsByPath', ...result.hit], { stdio: 'inherit' })
  if (run.error) throw run.error
  process.exitCode = run.status ?? 1
}
```

## Verify selection while running the full suite

Capture one entry per test file. The first root is the path matched against the runner report. In CI store both artifacts outside the working tree and compare results in the same checkout.

```sh
npx affected --strict -b auto -e auth=test/auth.test.ts -e billing=test/billing.test.ts \
  --shadow-out "$RUNNER_TEMP/selection.json"
```

Run the full suite with its JSON reporter. Run the comparison even if the tests fail. In GitHub Actions put each command in a separate step and use `if: always()` on the comparison step.

```sh
pnpm exec vitest run --reporter=json --outputFile="$RUNNER_TEMP/full.json"
npx affected --compare-shadow "$RUNNER_TEMP/selection.json" --results "$RUNNER_TEMP/full.json"
```

The comparison prints candidate and selection counts, failing files missed by the selector, and candidate files missing from the full run. It exits 1 for either disagreement. The full test step retains its own exit status. For programmatic use, call `shadowPlan(result, entries, cwd)` and `compareShadow(plan, runnerJson, cwd)`.

## Explain a selection

```ts
for (const name of result.hit) console.log(name, '←', result.why(name)?.join(' → '))
```

`why` returns the shortest import chain from the test file to a changed file — paste it in a PR comment
when someone asks why their test ran.
