import { realpathSync } from 'node:fs'
import { basename, dirname, join, relative, resolve, sep } from 'node:path'
import type { Affected, EntryInput } from './index.ts'
import { firstRoot } from './select.ts'

export interface ShadowPlan {
  version: 1
  entries: { name: string; path: string; selected: boolean; reason: string | null; diagnostics?: readonly { file: string; expression: string; revision: string }[] }[]
}

// Runners can report /var/... while process.cwd() uses /private/var/... on macOS.
function canonical(path: string): string {
  try {
    return realpathSync.native(path)
  } catch (error) {
    if (!(error instanceof Error) || !('code' in error) || error.code !== 'ENOENT') throw error
    const parent = dirname(path)
    if (parent === path) throw error
    return join(canonical(parent), basename(path))
  }
}

function pathFrom(cwd: string, path: string): string {
  return relative(canonical(resolve(cwd)), canonical(resolve(cwd, path))).split(sep).join('/')
}

/** The first source root of each entry is its test file. Additional roots are supporting inputs. */
export function shadowPlan<Name extends string>(result: Affected<Name>, entries: Record<Name, EntryInput>, cwd = process.cwd()): ShadowPlan {
  const tests = result.entries.map(entry => {
    const path = firstRoot(entries[entry.name])
    if (!path || path.endsWith('.json')) throw new Error('Shadow comparison needs a test file as the first root of ' + entry.name)
    return { name: entry.name, path: pathFrom(cwd, path), selected: entry.hit, reason: entry.reason ?? null,
      ...(entry.status === 'unknown' && { diagnostics: entry.diagnostics }) }
  })
  return { version: 1, entries: tests }
}

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

/** Compare a saved selection with the JSON reporter output of a full Vitest or Jest run. */
export function compareShadow(plan: unknown, results: unknown, cwd = process.cwd()) {
  if (!record(plan) || plan.version !== 1 || !Array.isArray(plan.entries)) throw new Error('Invalid shadow plan')
  const candidates = new Set<string>()
  const selected = new Set<string>()
  for (const entry of plan.entries) {
    if (!record(entry) || typeof entry.path !== 'string' || !entry.path || typeof entry.selected !== 'boolean') {
      throw new Error('Invalid shadow entry')
    }
    const path = pathFrom(cwd, entry.path)
    candidates.add(path)
    if (entry.selected) selected.add(path)
  }
  if (!record(results) || !Array.isArray(results.testResults) || !results.testResults.length) {
    throw new Error('Expected nonempty JSON testResults from a full test run')
  }
  const failed = new Set<string>()
  const seen = new Set<string>()
  for (const suite of results.testResults) {
    if (!record(suite) || typeof suite.name !== 'string' || !suite.name ||
        !['failed', 'passed', 'skipped', 'pending', 'disabled'].includes(String(suite.status))) {
      throw new Error('Incomplete suite in runner JSON')
    }
    const path = pathFrom(cwd, suite.name)
    if (suite.status === 'failed' || suite.status === 'passed') seen.add(path)
    // Suite setup can fail before the runner collects individual assertions.
    if (suite.status === 'failed' || (Array.isArray(suite.assertionResults) &&
        suite.assertionResults.some(a => record(a) && a.status === 'failed'))) failed.add(path)
  }
  if ((results.success === false || Number(results.numFailedTests) > 0 || Number(results.numFailedTestSuites) > 0) && !failed.size) {
    throw new Error('Runner reported a failure without identifying a failing suite')
  }
  return { candidates: candidates.size, selected: selected.size, executedSuites: seen.size, failed: [...failed].sort(),
    missed: [...failed].filter(path => !selected.has(path)).sort(),
    missingResults: [...candidates].filter(path => !seen.has(path)).sort() }
}
