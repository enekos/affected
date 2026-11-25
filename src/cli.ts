#!/usr/bin/env node
import { readFileSync } from 'node:fs'
import { parseArgs } from 'node:util'
import { affected, type EntryInput } from './index.ts'

const usage = `Usage: affected --entry <name>=<tsconfig.json | file,file…> [options]

Options:
  --entry, -e <name>=<paths>  Entry to check; repeat for several
  --base, -b <ref>            Compare against the fork point of <ref>
  --changed, -c <file>        A changed file; repeat for several, or pass - to read lines from stdin
  --committed                 With --base, ignore uncommitted and untracked files
  --no-types                  Do not follow \`import type\`
  --json                      Print the full report as JSON
  --exit-code                 Exit 0 when an entry is hit, 1 when none is
  --help, -h                  Show this help`

function fail(message: string): never {
  console.error(`affected: ${message}\n\n${usage}`)
  process.exit(2)
}

const { values } = parseArgs({
  options: {
    entry: { type: 'string', short: 'e', multiple: true },
    base: { type: 'string', short: 'b' },
    changed: { type: 'string', short: 'c', multiple: true },
    committed: { type: 'boolean' },
    types: { type: 'boolean', default: true },
    json: { type: 'boolean' },
    'exit-code': { type: 'boolean' },
    help: { type: 'boolean', short: 'h' },
  },
  allowNegative: true,
})

if (values.help) {
  console.log(usage)
  process.exit(0)
}

const entries: Record<string, EntryInput> = {}
for (const spec of values.entry ?? []) {
  const separator = spec.indexOf('=')
  if (separator <= 0) fail(`expected --entry <name>=<paths>, got "${spec}"`)
  entries[spec.slice(0, separator)] = spec.slice(separator + 1).split(',')
}
if (!Object.keys(entries).length) fail('pass at least one --entry')

const changed = values.changed?.flatMap(file =>
  file === '-' ? readFileSync(0, 'utf8').split(/\r?\n/).filter(Boolean) : [file],
)
if (!values.base && !changed) fail('pass --base or --changed')

try {
  const result = affected({
    entries,
    ...(values.base && { base: values.base }),
    ...(changed && { changed }),
    uncommitted: !values.committed,
    types: values.types,
  })
  console.log(values.json ? JSON.stringify(result, null, 2) : result.toString())
  if (values['exit-code']) process.exitCode = result.any ? 0 : 1
} catch (error) {
  console.error(`affected: ${error instanceof Error ? error.message : String(error)}`)
  process.exit(2)
}
