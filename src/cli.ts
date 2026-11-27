#!/usr/bin/env node
import { appendFileSync, readFileSync, writeFileSync, renameSync, rmSync } from 'node:fs'
import { randomUUID } from 'node:crypto'
import { parseArgs } from 'node:util'
import { affected, compareShadow, shadowPlan, type EntryPaths, type TraceEdge } from './index.ts'

const usage = `Usage: affected --entry <name>=<tsconfig.json | file,file…> [options]

Options:
  --entry, -e <name>=<paths>  Entry to check; repeat for several
  --base, -b <ref>            Compare against the fork point of <ref>; "auto" picks the CI target branch
  --changed, -c <file>        A changed file; repeat for several, or pass - to read lines from stdin
  --committed                 With --base, ignore uncommitted and untracked files
  --no-types                  Do not follow \`import type\`
  --global, -g <glob>         A changed file matching <glob> hits every entry; repeat for several
  --input, -i <name>=<glob>   A changed file matching <glob> hits entry <name>; repeat for several
  --strict                    Select entries with incomplete analysis when any files changed
  --trace <file>              Read traced edges [{from,to,revision?}] from JSON
  --shadow-out <file>         Save the proposed test selection for comparison with a full run
  --compare-shadow <file> --results <file>   Compare a plan with full Vitest/Jest JSON results
  --json                      Print the full report as JSON
  --names                     Print the names of the hit entries, one per line
  --github-output             Append \`affected\` (JSON array of hit names) and \`any\` to $GITHUB_OUTPUT
  --exit-code                 Exit 0 when an entry is hit, 1 when none is
  --help, -h                  Show this help`

function fail(message: string): never {
  console.error(`affected: ${message}\n\n${usage}`)
  process.exit(2)
}

function pair(spec: string, flag: string): [string, string] {
  const separator = spec.indexOf('=')
  if (separator <= 0) fail(`expected ${flag} <name>=<value>, got "${spec}"`)
  return [spec.slice(0, separator), spec.slice(separator + 1)]
}

function json(file: string): unknown {
  return JSON.parse(readFileSync(file, 'utf8'))
}

function traceEdges(file: string): TraceEdge[] {
  const edges = json(file)
  if (!Array.isArray(edges) || !edges.every(e => e && typeof e.from === 'string' && typeof e.to === 'string' &&
      (e.revision === undefined || e.revision === 'head' || e.revision === 'base'))) {
    throw new Error('Expected trace array of {from, to, revision?}')
  }
  return edges
}

function main() {
  const { values } = parseArgs({
    options: {
      entry: { type: 'string', short: 'e', multiple: true },
      base: { type: 'string', short: 'b' },
      changed: { type: 'string', short: 'c', multiple: true },
      committed: { type: 'boolean' },
      types: { type: 'boolean' },
      'no-types': { type: 'boolean' },
      strict: { type: 'boolean' },
      trace: { type: 'string' },
      'shadow-out': { type: 'string' },
      'compare-shadow': { type: 'string' },
      results: { type: 'string' },
      global: { type: 'string', short: 'g', multiple: true },
      input: { type: 'string', short: 'i', multiple: true },
      json: { type: 'boolean' },
      names: { type: 'boolean' },
      'github-output': { type: 'boolean' },
      'exit-code': { type: 'boolean' },
      help: { type: 'boolean', short: 'h' },
    },
  })

  if (values.help) {
    console.log(usage)
    process.exit(0)
  }

  if (values['compare-shadow'] || values.results) {
    if (!values['compare-shadow'] || !values.results) fail('--compare-shadow and --results must be used together')
    const report = compareShadow(json(values['compare-shadow']), json(values.results))
    console.log(JSON.stringify(report, null, 2))
    if (report.missed.length || report.missingResults.length) process.exitCode = 1
    return
  }
  if (values['shadow-out'] && (values['exit-code'] || values['github-output'])) fail('--shadow-out cannot gate a full run')

  const paths: Record<string, EntryPaths> = Object.create(null)
  for (const spec of values.entry ?? []) {
    const [name, list] = pair(spec, '--entry')
    paths[name] = list.split(',')
  }
  if (!Object.keys(paths).length) fail('pass at least one --entry')

  const inputs: Record<string, string[]> = Object.create(null)
  for (const spec of values.input ?? []) {
    const [name, glob] = pair(spec, '--input')
    if (!(name in paths)) fail(`--input names unknown entry "${name}"`)
    ;(inputs[name] ??= []).push(glob)
  }
  const entries = Object.fromEntries(
    Object.entries(paths).map(([name, from]) => [name, inputs[name] ? { from, inputs: inputs[name] } : from]),
  )

  const changed = values.changed?.flatMap(file =>
    file === '-' ? readFileSync(0, 'utf8').split(/\r?\n/).filter(Boolean) : [file],
  )
  if (!values.base && !changed) fail('pass --base or --changed')

  const result = affected({
    entries,
    ...(values.base && { base: values.base }),
    ...(changed && { changed }),
    ...(values.global && { globals: values.global }),
    uncommitted: !values.committed,
    types: !values['no-types'],
    ...(values.strict && { strict: true }),
    ...(values.trace && { trace: traceEdges(values.trace) }),
  })
  if (values['shadow-out']) {
    const file = values['shadow-out']
    const tmp = file + '.' + randomUUID() + '.tmp'
    try {
      writeFileSync(tmp, JSON.stringify(shadowPlan(result, entries)) + '\n', { flag: 'wx' })
      renameSync(tmp, file)
    } finally {
      rmSync(tmp, { force: true })
    }
  }
  if (values.json) console.log(JSON.stringify(result, null, 2))
  else if (values.names) result.hit.forEach(name => console.log(name))
  else console.log(result.toString())
  if (values['github-output']) {
    const file = process.env.GITHUB_OUTPUT
    if (!file) fail('--github-output needs $GITHUB_OUTPUT')
    appendFileSync(file, `affected=${JSON.stringify(result.hit)}\nany=${result.any}\n`)
  }
  if (values['exit-code']) process.exitCode = result.any ? 0 : 1
}

try {
  main()
} catch (error) {
  console.error(`affected: ${error instanceof Error ? error.message : String(error)}`)
  process.exit(2)
}
