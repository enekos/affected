#!/usr/bin/env node
import { appendFileSync, readFileSync } from 'node:fs'
import { parseArgs } from 'node:util'
import { affected, type EntryPaths } from './index.ts'

const usage = `Usage: affected --entry <name>=<tsconfig.json | file,file…> [options]

Options:
  --entry, -e <name>=<paths>  Entry to check; repeat for several
  --base, -b <ref>            Compare against the fork point of <ref>; "auto" picks the CI target branch
  --changed, -c <file>        A changed file; repeat for several, or pass - to read lines from stdin
  --committed                 With --base, ignore uncommitted and untracked files
  --no-types                  Do not follow \`import type\`
  --global, -g <glob>         A changed file matching <glob> hits every entry; repeat for several
  --input, -i <name>=<glob>   A changed file matching <glob> hits entry <name>; repeat for several
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

const { values } = parseArgs({
  options: {
    entry: { type: 'string', short: 'e', multiple: true },
    base: { type: 'string', short: 'b' },
    changed: { type: 'string', short: 'c', multiple: true },
    committed: { type: 'boolean' },
    types: { type: 'boolean', default: true },
    global: { type: 'string', short: 'g', multiple: true },
    input: { type: 'string', short: 'i', multiple: true },
    json: { type: 'boolean' },
    names: { type: 'boolean' },
    'github-output': { type: 'boolean' },
    'exit-code': { type: 'boolean' },
    help: { type: 'boolean', short: 'h' },
  },
  allowNegative: true,
})

if (values.help) {
  console.log(usage)
  process.exit(0)
}

const paths: Record<string, EntryPaths> = {}
for (const spec of values.entry ?? []) {
  const [name, list] = pair(spec, '--entry')
  paths[name] = list.split(',')
}
if (!Object.keys(paths).length) fail('pass at least one --entry')

const inputs: Record<string, string[]> = {}
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

try {
  const result = affected({
    entries,
    ...(values.base && { base: values.base }),
    ...(changed && { changed }),
    ...(values.global && { globals: values.global }),
    uncommitted: !values.committed,
    types: values.types,
  })
  if (values.json) console.log(JSON.stringify(result, null, 2))
  else if (values.names) result.hit.forEach(name => console.log(name))
  else console.log(result.toString())
  if (values['github-output']) {
    const file = process.env.GITHUB_OUTPUT
    if (!file) fail('--github-output needs $GITHUB_OUTPUT')
    appendFileSync(file, `affected=${JSON.stringify(result.hit)}\nany=${result.any}\n`)
  }
  if (values['exit-code']) process.exitCode = result.any ? 0 : 1
} catch (error) {
  console.error(`affected: ${error instanceof Error ? error.message : String(error)}`)
  process.exit(2)
}
