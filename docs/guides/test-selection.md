---
title: Test selection
parent: Guides
nav_order: 2
---

# Run only the tests a diff reaches

`affected tests` makes each test file an entry. It asks the project's own runner which files it would
run: Vitest through `createVitest` (Vitest 3 or later), Jest through `--listTests` and `--showConfig`.
Setup and global-setup files become extra roots of every test entry, so a change behind a setup file
selects every test, and the runner's config file is a global. Entries share resolution work, so a
thousand of them cost little more than one — 1,183 test files resolve in ~220 ms on a 5,000-file service.

```sh
npx affected tests                                   # what would run, and why
npx affected tests -- vitest run {files}             # run it
npx affected tests --runner jest -- jest {files}
vitest run $(npx affected tests --format vitest)     # or splice runner arguments yourself
```

| | |
|---|---|
| Some tests hit | The command runs with `{files}` replaced by them (or the files appended). |
| None hit | The command is skipped and `affected` exits 0. `--format vitest\|jest\|playwright` prints a filter that runs nothing and passes. |
| Every test hit | The command runs without file arguments, which is the full suite. |
| Analysis failed (no merge base, unknown ref) | The command runs on everything. Without a command, `affected` exits 2. |
| An import could not be resolved | Strict is on by default with `tests` and commands: the entry is selected. `--no-strict` turns it off. |

Without Vitest or Jest, use glob entries: `npx affected -e 'test/**/*.test.ts' -b auto -- node --test {files}`.

## From code

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
npx affected tests --shadow-out "$RUNNER_TEMP/selection.json"
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
