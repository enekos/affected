import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test, { type TestContext } from 'node:test'
import { fileURLToPath } from 'node:url'
import { affected, affectedAsync, detectBase, fromNftTrace, shadowPlan, compareShadow } from '../src/index.ts'

const cli = fileURLToPath(new URL('../src/cli.ts', import.meta.url))

function repo(t: TestContext, files: Record<string, string>) {
  const root = mkdtempSync(join(tmpdir(), 'affected-'))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const git = (...args: string[]) => execFileSync('git', ['-C', root, ...args], { stdio: 'pipe' })
  const write = (changes: Record<string, string>) => {
    for (const [path, contents] of Object.entries(changes)) {
      mkdirSync(dirname(join(root, path)), { recursive: true })
      writeFileSync(join(root, path), contents)
    }
  }
  git('init', '-q', '-b', 'main')
  git('config', 'user.email', 'test@localhost')
  git('config', 'user.name', 'Test')
  write(files)
  git('add', '-A')
  git('commit', '-qm', 'base')
  return { root, write }
}

const app = {
  'jobs/tsconfig.json': '{ "include": ["prompts"] }',
  'jobs/prompts/index.ts': "export * from './build.js'",
  'jobs/prompts/build.ts': "import { limit } from '../limits.js'\nexport const build = () => limit",
  'jobs/limits.ts': 'export const limit = 1',
  'web/main.ts': "import './view'",
  'web/view.ts': 'export {}',
}

test('reports which entries a diff reaches and why', t => {
  const { root, write } = repo(t, app)
  write({ 'jobs/limits.ts': 'export const limit = 2' })
  const result = affected({
    cwd: root,
    entries: { jobs: 'jobs/tsconfig.json', web: ['web/main.ts'] },
    base: 'main',
  })
  assert.deepEqual(result.hit, ['jobs'])
  assert.equal(result.any, true)
  assert.equal(result.has('web'), false)
  assert.deepEqual(result.why('jobs'), ['jobs/prompts/build.ts', 'jobs/limits.ts'])
  assert.equal(result.why('web'), undefined)
  assert.deepEqual(result.changed, ['jobs/limits.ts'])
  assert.deepEqual(result.entry('web').files, ['web/main.ts', 'web/view.ts'])
  assert.equal(
    result.toString(),
    'jobs  ← jobs/limits.ts  via jobs/prompts/build.ts\nweb  unaffected',
  )
  // @ts-expect-error entry names are checked at compile time
  assert.throws(() => result.has('jbos'), /Unknown entry "jbos"/)
})

test('accepts explicit changes and runs off the main thread', async t => {
  const { root } = repo(t, app)
  const result = await affectedAsync({ cwd: root, entries: { web: 'web/main.ts' }, changed: ['web/view.ts'] })
  assert.deepEqual(result.why('web'), ['web/main.ts', 'web/view.ts'])
})

test('surfaces errors as exceptions', t => {
  const { root } = repo(t, app)
  assert.throws(() => affected({ cwd: root, entries: { x: 'missing/tsconfig.json' }, base: 'main' }), /entry not found/)
  assert.throws(() => affected({ cwd: root, entries: { x: 'web/main.ts' } }), /pass `base` or `changed`/)
  assert.throws(() => affected({ cwd: root, entries: { x: 'web/main.ts' }, base: 'nope' }), /merge-base/)
})

test('the CLI prints one line per entry and signals hits through the exit code', t => {
  const { root, write } = repo(t, app)
  write({ 'web/view.ts': 'export const v = 1' })
  const run = (...args: string[]) => spawnSync(process.execPath, [cli, ...args], { cwd: root, encoding: 'utf8' })

  const human = run('-e', 'jobs=jobs/tsconfig.json', '-e', 'web=web/main.ts', '-b', 'main', '--exit-code')
  assert.equal(human.status, 0)
  assert.equal(human.stdout, 'jobs  unaffected\nweb  ← web/view.ts  via web/main.ts\n')

  const json = JSON.parse(run('-e', 'web=web/main.ts', '-b', 'main', '--json').stdout)
  assert.deepEqual(json.entries[0].hits, ['web/view.ts'])

  const none = run('-e', 'jobs=jobs/tsconfig.json', '-c', 'README.md', '--exit-code')
  assert.equal(none.status, 1)

  const stdin = spawnSync(process.execPath, [cli, '-e', 'jobs=jobs/tsconfig.json', '-c', '-'], {
    cwd: root,
    encoding: 'utf8',
    input: 'jobs/limits.ts\n',
  })
  assert.match(stdin.stdout, /^jobs {2}← jobs\/limits\.ts/)

  const usage = run('-b', 'main')
  assert.equal(usage.status, 2)
  assert.match(usage.stderr, /pass at least one --entry/)
})

test('inputs and globals hit entries no import reaches', t => {
  const { root, write } = repo(t, { ...app, 'jobs/templates/mail.hbs': 'hi', 'pnpm-lock.yaml': 'v1' })
  write({ 'jobs/templates/mail.hbs': 'hello' })
  const entries = { jobs: { from: 'jobs/tsconfig.json', inputs: ['jobs/templates/**'] }, web: 'web/main.ts' } as const
  const result = affected({ cwd: root, entries, base: 'main' })
  assert.deepEqual(result.hit, ['jobs'])
  assert.deepEqual(result.why('jobs'), ['jobs/templates/mail.hbs'])

  write({ 'pnpm-lock.yaml': 'v2' })
  const all = affected({ cwd: root, entries, base: 'main', globals: ['pnpm-lock.yaml'] })
  assert.deepEqual(all.hit, ['jobs', 'web'])
})

test("base 'auto' follows the CI's pull request target", () => {
  assert.equal(detectBase({ AFFECTED_BASE: 'origin/release' }), 'origin/release')
  assert.equal(detectBase({ GITHUB_BASE_REF: 'main' }), 'origin/main')
  assert.equal(detectBase({ CI_MERGE_REQUEST_TARGET_BRANCH_NAME: 'develop' }), 'origin/develop')
  assert.equal(detectBase({ SYSTEM_PULLREQUEST_TARGETBRANCH: 'refs/heads/main' }), 'origin/main')
  assert.equal(detectBase({}), 'origin/HEAD')
})

test('the CLI lists hit names and writes GitHub outputs', t => {
  const { root, write } = repo(t, app)
  write({ 'web/view.ts': 'export const v = 1', 'global.yml': 'v2' })
  const output = join(root, 'github-output')
  const run = (...args: string[]) =>
    spawnSync(process.execPath, [cli, ...args], { cwd: root, encoding: 'utf8', env: { ...process.env, GITHUB_OUTPUT: output } })

  const names = run('-e', 'jobs=jobs/tsconfig.json', '-e', 'web=web/main.ts', '-b', 'main', '--names', '--github-output')
  assert.equal(names.stdout, 'web\n')
  assert.equal(readFileSync(output, 'utf8'), 'affected=["web"]\nany=true\n')

  const global = run('-e', 'jobs=jobs/tsconfig.json', '-b', 'main', '-g', 'global.yml', '--names')
  assert.equal(global.stdout, 'jobs\n')

  const input = run('-e', 'jobs=jobs/tsconfig.json', '-b', 'main', '-i', 'jobs=web/**', '--names')
  assert.equal(input.stdout, 'jobs\n')
  assert.match(run('-e', 'jobs=jobs/tsconfig.json', '-b', 'main', '-i', 'nope=x').stderr, /unknown entry "nope"/)
})

test('strict selection and revision-specific trace explanations', async t => {
  const { root, write } = repo(t, { 'entry.ts': 'export const load = (id: string) => import(id)', 'real.ts': 'export {}', 'other.ts': '' })
  write({ 'real.ts': 'export const changed = 1' })
  const options = { cwd: root, entries: { app: 'entry.ts', other: 'other.ts' }, base: 'main' }
  const advisory = affected(options)
  assert.deepEqual(advisory.hit, [])
  assert.equal(advisory.entry('app').status, 'unknown')
  assert.ok(advisory.entry('app').diagnostics.some(d => d.file === 'entry.ts' && d.expression === 'import(id)' && d.revision === 'head'))
  assert.deepEqual(affected({ ...options, strict: true }).hit, ['app'])
  assert.deepEqual(affected({ ...options, strict: true, changed: [] }).hit, [])
  const trace = fromNftTrace({ reasons: new Map([['real.ts', { parents: new Set(['entry.ts']) }]]) }, 'base')
  const result = await affectedAsync({ ...options, trace })
  assert.deepEqual(result.why('app'), ['entry.ts', 'real.ts'])
  const why = result.why('app', { details: true })
  assert.equal(why.reason, 'trace')
  assert.equal(why.revision, 'base')
  assert.deepEqual(why.traceSteps, ['real.ts'])
  assert.deepEqual(affected({ cwd: root, entries: options.entries, changed: ['real.ts'], trace }).hit, [])
})

test('shadow comparison finds skipped failures and reports incomplete full runs', t => {
  const { root } = repo(t, { 'a.test.ts': 'import "./leaf"', 'b.test.ts': '', 'leaf.ts': '' })
  const entries = { alpha: 'a.test.ts', beta: ['b.test.ts'] }
  const plan = shadowPlan(affected({ cwd: root, entries, changed: ['leaf.ts'] }), entries, root)
  const full = { success: false, testResults: [
    { name: join(root, 'a.test.ts'), status: 'passed' },
    { name: join(root, 'b.test.ts'), status: 'failed' },
  ] }
  assert.deepEqual(compareShadow(plan, full, root), { candidates: 2, selected: 1, executedSuites: 2,
    failed: ['b.test.ts'], missed: ['b.test.ts'], missingResults: [] })
  assert.deepEqual(compareShadow(plan, { testResults: full.testResults.slice(1) }, root).missingResults, ['a.test.ts'])
  assert.throws(() => compareShadow(plan, { success: false, testResults: full.testResults.slice(0, 1) }, root), /without identifying/)
  assert.throws(() => compareShadow({}, full, root), /Invalid shadow plan/)
  const run = (...args: string[]) => spawnSync(process.execPath, [cli, ...args], { cwd: root, encoding: 'utf8' })
  assert.equal(run('-e','a=a.test.ts','-e','b=b.test.ts','-c','leaf.ts','--shadow-out','plan.json').status, 0)
  writeFileSync(join(root,'results.json'),JSON.stringify(full))
  const compare = run('--compare-shadow','plan.json','--results','results.json')
  assert.equal(compare.status, 1)
  assert.deepEqual(JSON.parse(compare.stdout).missed, ['b.test.ts'])
  assert.equal(run('-e','a=a.test.ts','--misspelled').status, 2)
})
