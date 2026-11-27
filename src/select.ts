import type { Affected, EntryInput } from './index.ts'

export const FORMATS = ['text', 'json', 'names', 'paths', 'vitest', 'jest', 'playwright'] as const
export type Format = (typeof FORMATS)[number]

/** A filter that matches no test file, so the runner exits 0 having run nothing. */
const NO_TESTS = '__affected_no_tests__'

/** The first root of an entry: the test file of a test entry, the tsconfig of a project entry. */
export function firstRoot(input: EntryInput): string | undefined {
  if (typeof input === 'string') return input
  if ('from' in input) return firstRoot(input.from)
  return input[0]
}

/** The first root of every hit entry, in entry order. */
export function selectedPaths<Name extends string>(result: Affected<Name>, entries: Readonly<Record<Name, EntryInput>>): string[] {
  return [...new Set(result.hit.map(name => firstRoot(entries[name])).filter(path => path !== undefined))]
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

/** Arguments that make a test runner run exactly `paths`, or nothing when `paths` is empty. */
export function runnerArgs(format: 'vitest' | 'jest' | 'playwright', paths: readonly string[]): string[] {
  switch (format) {
    case 'vitest':
      return ['--passWithNoTests', ...(paths.length ? paths : [NO_TESTS])]
    case 'jest':
      return ['--passWithNoTests', '--runTestsByPath', ...(paths.length ? paths : [NO_TESTS])]
    case 'playwright':
      // Playwright filters are regular expressions over the file path.
      return ['--pass-with-no-tests', ...(paths.length ? paths.map(path => `${escapeRegExp(path)}$`) : [NO_TESTS])]
  }
}

/** The selection as text in `format`, one item per line. */
export function render<Name extends string>(result: Affected<Name>, entries: Readonly<Record<Name, EntryInput>>, format: Format, onlyHits = false): string {
  switch (format) {
    case 'json':
      return JSON.stringify(result, null, 2)
    case 'names':
      return result.hit.join('\n')
    case 'paths':
      return selectedPaths(result, entries).join('\n')
    case 'vitest':
    case 'jest':
    case 'playwright':
      return runnerArgs(format, selectedPaths(result, entries)).join('\n')
    case 'text': {
      if (!onlyHits) return result.toString()
      const lines = result.toString().split('\n').filter((_, i) => result.entries[i]?.hit)
      lines.push(`${result.hit.length} of ${result.entries.length} affected`)
      return lines.join('\n')
    }
  }
}

/** `command` with `{files}` and `{names}` replaced; the files go last when neither appears. */
export function substitute(command: readonly string[], files: readonly string[] | undefined, names: readonly string[] | undefined): string[] {
  const placeholder = command.some(arg => arg === '{files}' || arg === '{names}')
  const out = command.flatMap(arg => {
    if (arg === '{files}') return files ?? []
    if (arg === '{names}') return names ?? []
    return [arg]
  })
  return placeholder || !files ? out : [...out, ...files]
}

/** `name=value` lines for $GITHUB_OUTPUT. */
export function githubOutput<Name extends string>(result: Affected<Name>, entries: Readonly<Record<Name, EntryInput>>): string {
  const files = selectedPaths(result, entries)
  return [
    `affected=${JSON.stringify(result.hit)}`,
    `any=${result.any}`,
    `files=${JSON.stringify(files)}`,
    `matrix=${JSON.stringify({ entry: result.hit })}`,
    `count=${result.hit.length}`,
    `total=${result.entries.length}`,
  ].join('\n') + '\n'
}

const SUMMARY_ROWS = 200

function cell(text: string): string {
  return text.replace(/\|/g, '\\|').replace(/\n/g, ' ')
}

/** A Markdown job summary: what was hit and why. */
export function githubSummary<Name extends string>(result: Affected<Name>, base: string | undefined): string {
  const hit = result.entries.filter(entry => entry.hit)
  const unknown = result.entries.filter(entry => entry.status === 'unknown')
  const lines = [
    `### affected: ${hit.length} of ${result.entries.length} selected`,
    '',
    `${result.changed.length} changed file${result.changed.length === 1 ? '' : 's'}${base ? ` since the fork point with \`${base}\`` : ''}.` +
      (unknown.length ? ` ${unknown.length} with incomplete analysis.` : ''),
  ]
  if (hit.length) {
    lines.push('', '| Entry | Why |', '|---|---|')
    for (const entry of hit.slice(0, SUMMARY_ROWS)) {
      const why = entry.status === 'unknown'
        ? `unknown: \`${cell(entry.diagnostics[0]?.expression ?? '')}\` in ${cell(entry.diagnostics[0]?.file ?? '')}`
        : entry.chain.map(path => `\`${cell(path)}\``).join(' → ')
      lines.push(`| \`${cell(entry.name)}\` | ${why} |`)
    }
    if (hit.length > SUMMARY_ROWS) lines.push(`| … | ${hit.length - SUMMARY_ROWS} more |`)
  }
  return lines.join('\n') + '\n'
}
