#!/usr/bin/env node
import { spawnSync } from 'node:child_process'
import { appendFileSync, mkdtempSync, readFileSync, writeFileSync, renameSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { randomUUID } from 'node:crypto'
import { parseArgs } from 'node:util'
import { affected, compareShadow, detectBase, expandGlobs, isGlob, shadowPlan, type Affected, type EntryInput, type EntryPaths, type TraceEdge } from './index.ts'
import { loadConfig, type AffectedConfig, type Runner } from './config.ts'
import { detectRunner, discoverTests, testEntries } from './tests.ts'
import { FORMATS, firstRoot, githubOutput, githubSummary, render, selectedPaths, substitute, type Format } from './select.ts'

const usage = `Usage:
  affected [options]                  Which entries does the diff reach?
  affected tests [options]            Which test files does it reach? The project’s runner lists them.
  affected [tests] [options] -- <command…>
                                      Run <command> with the selection: {files} and {names}
                                      expand to it, and the files go last without either. An
                                      empty selection skips the command; a failed analysis
                                      runs it on everything.

Entries:
  --entry, -e <name>=<paths>  Entry to check: a tsconfig.json, or files and globs, comma-separated
  --entry, -e <glob>          One entry per matching file, named by its path; repeat for several
  --config <file>             Read entries and defaults (default: affected.config.{ts,mts,js,mjs,json})
  --no-config                 Ignore affected.config.*
  --runner <vitest|jest|playwright>      With tests: the runner to ask (default: the one package.json lists)
  With Playwright, --names emits project-qualified lines. Use --test-list {test-list} after --.
  --runner-config <file>      With tests: the runner's config file

Diff:
  --base, -b <ref>            Compare against the fork point of <ref> (default: auto, the CI target branch)
  --changed, -c <file>        A changed file; repeat for several, or pass - to read lines from stdin
  --committed                 With --base, ignore uncommitted and untracked files
  --no-types                  Do not follow \`import type\`
  --global, -g <glob>         A changed file matching <glob> hits every entry; repeat for several
  --input, -i <name>=<glob>   A changed file matching <glob> hits entry <name>; repeat for several
  --strict                    Select entries with incomplete analysis (default with tests and commands)
  --no-strict                 Do not select entries with incomplete analysis
  --trace <file>              Read traced edges [{from,to,revision?}] from JSON

Output:
  --graph mermaid             Print a fenced PR impact diagram (also included with --github-summary)
  --format, -f <format>       ${FORMATS.join(', ')}.
                              The runner formats print arguments that run exactly the hit test files
  --json, --names             Short for --format json, --format names
  --github-output             Append affected, any, files, matrix, count and total to $GITHUB_OUTPUT
  --github-summary            Append a Markdown table of hits to $GITHUB_STEP_SUMMARY
  --exit-code                 Exit 0 when an entry is hit, 1 when none is
  --shadow-out <file>         Save the proposed test selection for comparison with a full run
  --compare-shadow <file> --results <file>   Compare a plan with full Vitest/Jest JSON results
  --help, -h                  Show this help`

class UsageError extends Error {}

function fail(message: string): never {
  throw new UsageError(message)
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

function cliEntries(specs: readonly string[]): Record<string, EntryPaths> {
  const entries: Record<string, EntryPaths> = Object.create(null)
  for (const spec of specs) {
    if (!spec.includes('=') && isGlob(spec)) {
      const files = expandGlobs(spec)
      if (!files.length) fail(`--entry ${spec} matches no files`)
      for (const file of files) entries[file] = file
      continue
    }
    const [name, list] = pair(spec, '--entry')
    const paths = list.split(',').flatMap(path => isGlob(path) ? expandGlobs(path) : [path])
    if (!paths.length) fail(`--entry ${spec} matches no files`)
    entries[name] = paths
  }
  return entries
}

function writeAtomic(file: string, contents: string) {
  const tmp = file + '.' + randomUUID() + '.tmp'
  try {
    writeFileSync(tmp, contents, { flag: 'wx' })
    renameSync(tmp, file)
  } finally {
    rmSync(tmp, { force: true })
  }
}

function append(variable: 'GITHUB_OUTPUT' | 'GITHUB_STEP_SUMMARY', text: string) {
  const file = process.env[variable]
  if (!file) fail(`--${variable === 'GITHUB_OUTPUT' ? 'github-output' : 'github-summary'} needs $${variable}`)
  appendFileSync(file, text)
}

function exec(command: readonly string[], testList?: readonly string[]): never {
  const [program, ...args] = command
  if (!program) fail('missing command after --')
  // Windows resolves pnpm, npx and friends to .cmd shims, which only a shell can start.
  const directory = testList && mkdtempSync(join(tmpdir(), 'affected-run-'))
  let status: number
  try {
    let argv = args
    if (directory && testList) {
      const path = join(directory, 'tests.txt')
      writeFileSync(path, testList.join('\n') + '\n')
      argv = args.map(arg => arg === '{test-list}' ? path : arg)
    }
    const child = spawnSync(program, argv, { stdio: 'inherit', shell: process.platform === 'win32' })
    if (child.error) throw child.error
    status = child.status ?? 1
  } finally {
    if (directory) rmSync(directory, { recursive: true, force: true })
  }
  process.exit(status)
}

async function main(argv: string[]) {
  const separator = argv.indexOf('--')
  const command = separator === -1 ? undefined : argv.slice(separator + 1)
  const args = separator === -1 ? argv : argv.slice(0, separator)
  const tests = args[0] === 'tests'
  const { values, positionals } = parseArgs({
    args: tests ? args.slice(1) : args,
    allowPositionals: true,
    options: {
      entry: { type: 'string', short: 'e', multiple: true },
      config: { type: 'string' },
      'no-config': { type: 'boolean' },
      runner: { type: 'string' },
      'runner-config': { type: 'string' },
      base: { type: 'string', short: 'b' },
      changed: { type: 'string', short: 'c', multiple: true },
      committed: { type: 'boolean' },
      types: { type: 'boolean' },
      'no-types': { type: 'boolean' },
      strict: { type: 'boolean' },
      'no-strict': { type: 'boolean' },
      trace: { type: 'string' },
      'shadow-out': { type: 'string' },
      'compare-shadow': { type: 'string' },
      results: { type: 'string' },
      global: { type: 'string', short: 'g', multiple: true },
      input: { type: 'string', short: 'i', multiple: true },
      format: { type: 'string', short: 'f' },
      graph: { type: 'string' },
      json: { type: 'boolean' },
      names: { type: 'boolean' },
      'github-output': { type: 'boolean' },
      'github-summary': { type: 'boolean' },
      'exit-code': { type: 'boolean' },
      help: { type: 'boolean', short: 'h' },
    },
  })

  if (values.help) {
    console.log(usage)
    return
  }
  if (positionals.length) fail(`unexpected argument "${positionals[0]}"`)

  if (values.graph !== undefined && values.graph !== 'mermaid') fail('expected --graph mermaid')
  if (values.graph && (values.format || values.json || values.names || command || values['compare-shadow'])) fail('--graph cannot be combined with other output formats or a command')

  if (values['compare-shadow'] || values.results) {
    if (!values['compare-shadow'] || !values.results) fail('--compare-shadow and --results must be used together')
    const report = compareShadow(json(values['compare-shadow']), json(values.results))
    console.log(JSON.stringify(report, null, 2))
    if (report.missed.length || report.missingResults.length) process.exitCode = 1
    return
  }
  if (values['shadow-out'] && (values['exit-code'] || values['github-output'] || command)) fail('--shadow-out cannot gate a full run')
  if (command && (values['exit-code'] || values.format || values.json || values.names)) fail('a command after -- cannot be combined with output flags')

  const format = (values.format ?? (values.json ? 'json' : values.names ? 'names' : 'text')) as Format
  if (!FORMATS.includes(format)) fail(`unknown --format "${values.format}"`)
  const runner = values.runner
  if (runner !== undefined && runner !== 'vitest' && runner !== 'jest' && runner !== 'playwright') fail(`unknown --runner "${runner}"`)

  const config: AffectedConfig = values['no-config'] ? {} : (await loadConfig(process.cwd(), values.config))?.config ?? {}

  const changed = values.changed?.flatMap(file =>
    file === '-' ? readFileSync(0, 'utf8').split(/\r?\n/).filter(Boolean) : [file],
  )
  const base = values.base ?? config.base ?? (changed ? undefined : 'auto')
  let entries: Record<string, EntryInput>
  const globals = [...(config.globals ?? []), ...(values.global ?? [])]
  let suite: Awaited<ReturnType<typeof discoverTests>> | undefined
  if (tests) {
    if (values.entry) fail('tests finds its own entries; drop --entry')
    const chosen: Runner = runner ?? config.tests?.runner ?? detectRunner()
    const runnerConfig = values['runner-config'] ?? config.tests?.config
    try {
      suite = await discoverTests({
        runner: chosen, ...(runnerConfig && { config: runnerConfig }),
        ...(base && { base: base === 'auto' ? detectBase() : base }),
        ...(changed && { changed }), uncommitted: !values.committed,
      })
    } catch (error) {
      if (!command || chosen !== 'playwright') throw error
      const listArg = command.indexOf('{test-list}')
      if (listArg >= 0 && command[listArg - 1] !== '--test-list') throw error
      const full = listArg < 0 ? command : [...command.slice(0, listArg - 1), ...command.slice(listArg + 1)]
      console.error(`affected: ${error instanceof Error ? error.message : String(error)}\naffected: running everything`)
      exec(substitute(full, undefined, undefined))
    }
    entries = testEntries(suite)
    globals.push(...suite.globals)
  } else {
    // A flag entry replaces the config entry of the same name; config and binary CLI agree on this.
    entries = { ...config.entries, ...(values.entry && cliEntries(values.entry)) }
  }
  if (!Object.keys(entries).length) fail('pass at least one --entry, or add entries to affected.config')

  const wantsList = command?.includes('{test-list}')
  if (wantsList && (suite?.runner !== 'playwright' || command?.some(arg => arg === '{files}' || arg === '{names}'))) {
    fail('{test-list} requires Playwright discovery and cannot be mixed with {files} or {names}')
  }
  const inputs: Record<string, string[]> = Object.create(null)
  for (const spec of values.input ?? []) {
    const [name, glob] = pair(spec, '--input')
    if (!(name in entries)) fail(`--input names unknown entry "${name}"`)
    ;(inputs[name] ??= []).push(glob)
  }
  for (const [name, globs] of Object.entries(inputs)) {
    const input = entries[name] as EntryInput
    entries[name] = typeof input === 'object' && 'from' in input
      ? { from: input.from, inputs: [...(input.inputs ?? []), ...globs] }
      : { from: input, inputs: globs }
  }

  // Test runners run everything without file arguments; other commands get every entry.
  const everything = (cmd: readonly string[]) => tests
    ? substitute(cmd, undefined, undefined)
    : substitute(cmd, [...new Set(Object.values(entries).map(firstRoot).filter(path => path !== undefined))], Object.keys(entries))

  const strict = values['no-strict'] ? false : Boolean(values.strict || config.strict || tests || command)

  let result: Affected
  try {
    result = affected({
      entries,
      ...(base && { base }),
      ...(changed && { changed }),
      ...(globals.length && { globals }),
      uncommitted: !values.committed,
      types: values['no-types'] ? false : values.types ?? config.types ?? (suite?.runner !== 'playwright'),
      ...(values.graph && { graph: 'mermaid' }),
      ...(strict && { strict: true }),
      ...(values.trace && { trace: traceEdges(values.trace) }),
    })
  } catch (error) {
    if (!command) throw error
    console.error(`affected: ${error instanceof Error ? error.message : String(error)}\naffected: running everything`)
    if (wantsList) exec(command, Object.keys(entries))
    exec(everything(command))
  }

  if (values['shadow-out'] && suite?.runner === 'playwright') fail('Playwright shadow comparison requires a project-aware results reporter')
  if (values['shadow-out']) writeAtomic(values['shadow-out'], JSON.stringify(shadowPlan(result, entries)) + '\n')
  if (values['github-output']) append('GITHUB_OUTPUT', githubOutput(result, entries))
  if (values['github-summary']) append('GITHUB_STEP_SUMMARY', githubSummary(result, base === 'auto' ? detectBase() : base))

  if (values['github-summary'] && result.mermaid) append('GITHUB_STEP_SUMMARY', '\n' + result.mermaid + '\n')

  if (command) {
    const hit = result.hit.length
    const total = result.entries.length
    if (!hit) {
      console.error(`affected: none of ${total} entries affected; skipping ${command.join(' ')}`)
      return
    }
    console.error(`affected: ${hit} of ${total} entries affected`)
    if (wantsList) exec(command, result.hit)
    exec(hit === total ? everything(command) : substitute(command, selectedPaths(result, entries), result.hit))
  }

  if (format === 'playwright' && suite?.runner === 'playwright') {
    const paths = new Set(selectedPaths(result, entries))
    if (suite.files.some(file => paths.has(file.path) && !result.has(file.name ?? file.path))) {
      fail('This selection differs by project. Use --names for a Playwright --test-list file.')
    }
  }
  const text = result.mermaid ?? render(result, entries, format, tests)
  if (text) console.log(text)
  if (values['exit-code']) process.exitCode = result.any ? 0 : 1
}

main(process.argv.slice(2)).catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error)
  console.error(`affected: ${message}${error instanceof UsageError ? `\n\n${usage}` : ''}`)
  process.exit(2)
})
