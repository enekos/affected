import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test, { type TestContext } from 'node:test'
import { fileURLToPath } from 'node:url'
import { affected, affectedAsync } from '../src/index.ts'

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
