import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test, { type TestContext } from 'node:test'
import { fileURLToPath } from 'node:url'
import { affected, discoverTests, globEntries, githubSummary, parseConfig, runnerArgs, substitute, testEntries } from '../src/index.ts'

const cli = fileURLToPath(new URL('../src/cli.ts', import.meta.url))

function repo(t: TestContext, files: Record<string, string>) {
  const root = mkdtempSync(join(tmpdir(), 'affected-cli-'))
  t.after(() => rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 }))
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
  const run = (args: string[], env: Record<string, string> = {}) =>
    spawnSync(process.execPath, [cli, ...args], { cwd: root, encoding: 'utf8', env: { ...process.env, ...env } })
  return { root, write, run }
}

const suite = {
  'src/a.ts': "import { b } from './b'\nexport const a = b",
  'src/b.ts': 'export const b = 1',
  'src/c.ts': 'export const c = 1',
  'src/a.test.ts': "import { a } from './a'",
  'src/c.test.ts': "import { c } from './c'",
  'src/[id].test.ts': "import { c } from './c'",
  'test/setup.ts': "import './helpers'",
  'test/helpers.ts': 'export {}',
  '.gitignore': 'ignored/\n',
  'ignored/x.test.ts': '',
}

// Stand-ins for the runners' discovery interfaces, so tests run without installing them.
const fakeVitest = {
  'node_modules/vitest/package.json': JSON.stringify({ name: 'vitest', exports: { './node': './node.mjs' } }),
  'node_modules/vitest/node.mjs': `import { join } from 'node:path'
export async function createVitest(mode, options) {
  const root = options.root
  const project = { config: { setupFiles: [join(root, 'test/setup.ts')], globalSetup: [] }, vite: { config: { configFile: join(root, 'vitest.config.ts') } } }
  return {
    vite: project.vite,
    async globTestSpecifications() {
      return ['src/a.test.ts', 'src/c.test.ts', 'src/[id].test.ts'].map(file => ({ moduleId: join(root, file), project }))
    },
    async close() {},
  }
}`,
  'vitest.config.ts': 'export default {}',
  'package.json': JSON.stringify({ devDependencies: { vitest: '5.0.0' } }),
}

const fakeJest = {
  'node_modules/jest/package.json': JSON.stringify({ name: 'jest' }),
  'node_modules/jest/bin/jest.js': `const { join } = require('node:path')
const root = process.cwd()
if (process.argv.includes('--listTests')) console.log(JSON.stringify(['src/a.test.ts', 'src/c.test.ts'].map(f => join(root, f))))
else console.log(JSON.stringify({ configs: [{ rootDir: root, setupFiles: [], setupFilesAfterEnv: [join(root, 'test/setup.ts'), join(root, 'node_modules/jest-extended/all.js')] }] }))`,
  'jest.config.js': 'module.exports = {}',
  'package.json': JSON.stringify({ devDependencies: { jest: '30.0.0' } }),
}

test('glob entries make one entry per tracked or untracked file, named by path', t => {
  const { root, write, run } = repo(t, suite)
  write({ 'src/new.test.ts': '' })
  assert.deepEqual(Object.keys(globEntries('src/**/*.test.ts', root)), ['src/[id].test.ts', 'src/a.test.ts', 'src/c.test.ts', 'src/new.test.ts'])
  assert.deepEqual(Object.keys(globEntries(['**/*.test.ts', '!src/new.test.ts', '!src/?id?.test.ts'], root)), ['src/a.test.ts', 'src/c.test.ts'])
  write({ 'src/b.ts': 'export const b = 2' })
  assert.equal(run(['-e', 'src/*.test.ts', '-b', 'main', '--names']).stdout, 'src/a.test.ts\nsrc/new.test.ts\n', 'a new test file is changed')
  assert.equal(run(['-e', 'group=src/a.test.ts,src/c.*', '-b', 'main', '--names']).stdout, 'group\n')
  assert.match(run(['-e', 'nothing/**/*.ts', '-b', 'main']).stderr, /matches no files/)
})

test('runner formats select exactly the hit files, or nothing', t => {
  const { write, run } = repo(t, suite)
  const args = ['-e', 'src/*.test.ts', '-b', 'main']
  assert.equal(run([...args, '-f', 'vitest']).stdout, '--passWithNoTests\n__affected_no_tests__\n')
  write({ 'src/c.ts': 'export const c = 2' })
  assert.equal(run([...args, '-f', 'paths']).stdout, 'src/[id].test.ts\nsrc/c.test.ts\n')
  assert.equal(run([...args, '-f', 'jest']).stdout, '--passWithNoTests\n--runTestsByPath\nsrc/[id].test.ts\nsrc/c.test.ts\n')
  assert.equal(run([...args, '-f', 'playwright']).stdout, '--pass-with-no-tests\nsrc/\\[id\\]\\.test\\.ts$\nsrc/c\\.test\\.ts$\n')
  assert.equal(run([...args, '-f', 'yaml']).status, 2)
  assert.deepEqual(runnerArgs('vitest', ['a.test.ts']), ['--passWithNoTests', 'a.test.ts'])
})

test('a command after -- gets the selection, skips on none and runs everything when analysis fails', t => {
  const { write, run } = repo(t, suite)
  const echo = ['--', process.execPath, '-e', 'console.log(JSON.stringify(process.argv.slice(1)))']
  const args = ['-e', 'src/*.test.ts', '-b', 'main']

  const none = run([...args, ...echo, '{files}'])
  assert.equal(none.status, 0)
  assert.equal(none.stdout, '')
  assert.match(none.stderr, /none of 3 entries affected; skipping/)

  write({ 'src/b.ts': 'export const b = 2' })
  assert.equal(run([...args, ...echo, 'run', '{files}']).stdout, '["run","src/a.test.ts"]\n')
  assert.equal(run([...args, ...echo]).stdout, '["src/a.test.ts"]\n')
  assert.equal(run([...args, ...echo, '{names}']).stdout, '["src/a.test.ts"]\n')

  const failed = run(['-e', 'src/*.test.ts', '-b', 'no-such-ref', ...echo, '{names}'])
  assert.equal(failed.status, 0)
  assert.match(failed.stderr, /running everything/)
  assert.equal(failed.stdout, '["src/[id].test.ts","src/a.test.ts","src/c.test.ts"]\n')

  const exit = run([...args, '--', process.execPath, '-e', 'process.exit(3)'])
  assert.equal(exit.status, 3)
  assert.equal(run([...args, '--names', ...echo]).status, 2)
})

test('affected tests asks Vitest for test files and setup files', async t => {
  const { root, write, run } = repo(t, { ...suite, ...fakeVitest })
  const found = await discoverTests({ cwd: root })
  assert.equal(found.runner, 'vitest')
  assert.deepEqual(found.globals, ['vitest.config.ts'])
  assert.deepEqual(testEntries(found)['src/a.test.ts'], ['src/a.test.ts', 'test/setup.ts'])

  assert.equal(run(['tests', '-b', 'main', '-f', 'paths']).stdout, '')
  write({ 'test/helpers.ts': 'export const h = 1' })
  assert.equal(run(['tests', '-b', 'main', '-f', 'paths']).stdout, 'src/[id].test.ts\nsrc/a.test.ts\nsrc/c.test.ts\n')
  const echo = ['--', process.execPath, '-e', 'console.log(JSON.stringify(process.argv.slice(1)))']
  assert.equal(run(['tests', '-b', 'main', ...echo, 'run', '{files}']).stdout, '["run"]\n', 'all hit runs unfiltered')

  write({ 'test/helpers.ts': 'export {}', 'src/b.ts': 'export const b = 2' })
  const text = run(['tests', '-b', 'main'])
  assert.equal(text.stdout, 'src/a.test.ts  ← src/b.ts  via src/a.test.ts → src/a.ts\n1 of 3 affected\n')
  write({ 'src/b.ts': 'export const b = 1', 'vitest.config.ts': 'export default { test: {} }' })
  assert.equal(run(['tests', '-b', 'main', '--names']).stdout.split('\n').length, 4)
})

test('affected tests asks Jest for test files and in-repo setup files', async t => {
  const { root, write, run } = repo(t, { ...suite, ...fakeJest })
  const found = await discoverTests({ cwd: root })
  assert.equal(found.runner, 'jest')
  assert.deepEqual(found.files.map(file => file.path), ['src/a.test.ts', 'src/c.test.ts'])
  assert.deepEqual(found.files[0]?.setup, ['test/setup.ts'])
  assert.deepEqual(found.globals, ['jest.config.js'])
  write({ 'src/c.ts': 'export const c = 2' })
  assert.equal(run(['tests', '-b', 'main', '-f', 'jest']).stdout, '--passWithNoTests\n--runTestsByPath\nsrc/c.test.ts\n')
  assert.match(run(['tests', '--runner', 'mocha']).stderr, /unknown --runner/)
})

test('affected.config supplies entries and defaults, and flags win', t => {
  const { write, run } = repo(t, {
    ...suite,
    'affected.config.json': JSON.stringify({ entries: { a: 'src/a.test.ts', c: ['src/c.test.ts'] }, base: 'main', globals: ['ci.yml'] }),
    'ci.yml': '',
  })
  write({ 'src/c.ts': 'export const c = 2' })
  assert.equal(run(['--names']).stdout, 'c\n')
  assert.equal(run(['-e', 'c=src/a.test.ts', '--names']).stdout, '', 'a flag replaces the config entry')
  assert.equal(run(['-e', 'extra=src/c.test.ts', '--names']).stdout, 'c\nextra\n')
  assert.equal(run(['--no-config', '--names']).status, 2)
  write({ 'ci.yml': 'changed' })
  assert.equal(run(['--names']).stdout, 'a\nc\n')
  write({ 'affected.config.json': '{"entries":{},"nope":1}' })
  assert.match(run(['--names']).stderr, /unknown key "nope"/)
  write({ 'other.mjs': "export default { entries: { b: 'src/a.test.ts' }, base: 'main' }" })
  assert.equal(run(['--config', 'other.mjs', '--names']).stdout, '')
  assert.throws(() => parseConfig({ entries: { a: 1 } }, 'x.json'), /entry "a" must be/)
  assert.throws(() => parseConfig({ tests: { runner: 'mocha' } }, 'x.json'), /tests.runner/)
})

test('GitHub summary and substitution helpers', t => {
  const { root, write } = repo(t, suite)
  write({ 'src/b.ts': 'export const b = 2' })
  const result = affected({ cwd: root, entries: { a: 'src/a.test.ts', 'c|d': 'src/c.test.ts' }, base: 'main' })
  const summary = githubSummary(result, 'origin/main')
  assert.match(summary, /^### affected: 1 of 2 selected\n\n1 changed file since the fork point with `origin\/main`\./)
  assert.match(summary, /\| `a` \| `src\/a.test.ts` → `src\/a.ts` → `src\/b.ts` \|/)
  assert.deepEqual(substitute(['x', '{files}', 'y'], ['f'], ['n']), ['x', 'f', 'y'])
  assert.deepEqual(substitute(['x'], undefined, undefined), ['x'])
  assert.deepEqual(readFileSync(join(root, 'src/b.ts'), 'utf8'), 'export const b = 2')
})

test('the GitHub Action deepens a shallow pull request checkout and runs the selection', { skip: process.platform === 'win32' }, t => {
  const { root: origin, write } = repo(t, { ...suite, ...fakeVitest })
  const git = (cwd: string, ...args: string[]) => execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8', stdio: 'pipe' })
  git(origin, 'checkout', '-qb', 'feature')
  for (let i = 0; i < 60; i++) {
    write({ [`notes/${i}.md`]: String(i) })
    git(origin, 'add', '-A')
    git(origin, 'commit', '-qm', `note ${i}`)
  }
  write({ 'src/b.ts': 'export const b = 2' })
  git(origin, 'commit', '-qam', 'change b')
  git(origin, 'checkout', '-q', 'main')
  write({ 'main.md': 'moves on' })
  git(origin, 'add', '-A')
  git(origin, 'commit', '-qm', 'main moves on')

  const clone = mkdtempSync(join(tmpdir(), 'affected-action-'))
  t.after(() => rmSync(clone, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 }))
  execFileSync('git', ['clone', '-q', '--depth=1', '--branch', 'feature', `file://${origin}`, clone], { stdio: 'pipe' })
  const output = join(clone, '.github-output')
  const summary = join(clone, '.github-summary')
  const action = fileURLToPath(new URL('../action/run.sh', import.meta.url))
  const env = {
    ...process.env, GITHUB_BASE_REF: 'main', GITHUB_ACTION_PATH: fileURLToPath(new URL('..', import.meta.url)),
    GITHUB_OUTPUT: output, GITHUB_STEP_SUMMARY: summary, AFFECTED_BIN: `${process.execPath} ${cli}`,
    INPUT_RUN: `${process.execPath} -e console.log(JSON.stringify(process.argv.slice(1))) {files}`,
    INPUT_ENTRIES: 'tests', INPUT_ARGS: '', INPUT_BASE: 'auto', INPUT_SUMMARY: 'true', INPUT_FETCH: 'true', INPUT_VERSION: '', EVENT_BEFORE: '',
  }
  const run = spawnSync('bash', [action], { cwd: clone, encoding: 'utf8', env })
  assert.equal(run.status, 0, run.stderr)
  assert.equal(run.stdout, '["src/a.test.ts"]\n', run.stderr)
  assert.match(readFileSync(output, 'utf8'), /^affected=\["src\/a.test.ts"\]\nany=true\nfiles=\["src\/a.test.ts"\]\n/)
  assert.match(readFileSync(summary, 'utf8'), /1 of 3 selected/)
})

test('Mermaid shares dependency nodes, labels base edges and accounts for every changed input', t => {
  const { root, run, write } = repo(t, { ...suite, 'second.ts': "import './src/a'", 'old.ts': 'export {}',
    'removed.ts': "import './old'" })
  write({ 'removed.ts': 'export {}', 'old.ts': 'export const n = 1', 'src/b.ts': 'export const b = 2',
    'readme.md': 'docs', 'any.ts': "import(variable)" })
  const args = ['-e', 'alpha=src/a.test.ts', '-e', 'beta=second.ts', '-e', 'old=removed.ts',
    '-e', 'strict=any.ts', '-e', 'label"\u0060#=src/c.test.ts', '-i', 'label"\u0060#=src/b.ts']
  // Give the uncertain entry an existing root, so selection comes from strict mode.
  execFileSync('git', ['-C', root, 'add', 'any.ts'])
  execFileSync('git', ['-C', root, 'commit', '-qm', 'add unknown entry'])
  const summary = join(root, 'summary.md')
  const result = run([...args, '-b', 'HEAD', '--strict', '--graph', 'mermaid', '--github-summary'], { GITHUB_STEP_SUMMARY: summary })
  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /^```mermaid\nflowchart LR/)
  assert.equal(result.stdout.match(/Changed · src\/b.ts/g)?.length, 1)
  const baseOnly = run(['-e', 'old=removed.ts', '-b', 'HEAD', '-c', 'old.ts', '--graph', 'mermaid'])
  assert.equal(baseOnly.status, 0, baseOnly.stderr)
  assert.match(baseOnly.stdout, /\|base\|/)
  assert.match(result.stdout, /configured input/)
  assert.match(result.stdout, /Incomplete analysis/)
  assert.match(result.stdout, /No known consumer/)
  assert.match(result.stdout, /label#34;#96;#35;/)
  assert.ok(readFileSync(summary, 'utf8').endsWith(result.stdout))
  assert.equal(run([...args, '-b', 'HEAD', '--graph', 'dot']).status, 2)
})

test('Mermaid bounds fan-out while the JSON report keeps the whole selection', t => {
  const { run } = repo(t, suite)
  const args = Array.from({length: 25}, (_, i) => ['-e', 'app' + i + '=src/a.ts']).flat()
  const graph = run([...args, '-c', 'src/b.ts', '--graph', 'mermaid'])
  assert.equal(graph.status, 0, graph.stderr)
  assert.match(graph.stdout, /5 additional entries/)
  assert.equal(JSON.parse(run([...args, '-c', 'src/b.ts', '--json']).stdout).entries.length, 25)
})

test('real Playwright collection preserves project identity, transitive setup and teardown, and global hooks', async t => {
  const { root, run } = repo(t, {
    'package.json': '{"devDependencies":{"@playwright/test":"*"}}',
    'playwright.config.cjs': `module.exports = {
      testDir: './specs', globalSetup: ['./first.cjs', './second.cjs'],
      projects: [{name: 'seed', testMatch: /seed.spec.js/, teardown: 'cleanup'},
        {name: 'cleanup', testMatch: /cleanup.spec.js/},
        {name: 'login', dependencies: ['seed'], testMatch: /login.spec.js/},
        {name: 'private', dependencies: ['login'], testMatch: /shared.spec.js/},
        {name: 'public', testMatch: /shared.spec.js/}]
    }`,
    'first.cjs': 'module.exports = () => {}',
    'second.cjs': "require('./global-helper.cjs');module.exports = () => {}",
    'global-helper.cjs': '',
    ...Object.fromEntries(['seed', 'cleanup', 'login', 'shared'].map(name =>
      ['specs/' + name + '.spec.js', "const {test}=require('@playwright/test'); test('example',()=>{})"])),
  })
  const { symlinkSync } = await import('node:fs')
  const { createRequire } = await import('node:module')
  const require = createRequire(import.meta.url)
  mkdirSync(join(root, 'node_modules/@playwright'), { recursive: true })
  symlinkSync(dirname(require.resolve('@playwright/test/package.json')), join(root, 'node_modules/@playwright/test'), 'junction')
  const choose = (file: string) => {
    const result = run(['tests', '--changed', file, '--names'])
    assert.equal(result.status, 0, result.stderr)
    return result.stdout.trim().split('\n')
  }
  const selection = choose('specs/cleanup.spec.js')
  assert.ok(selection.some(line => line.startsWith('[private]')))
  assert.ok(!selection.some(line => line.startsWith('[public]')))
  assert.equal(choose('global-helper.cjs').length, 5)
  assert.equal(choose('playwright.config.cjs').length, 5)
  const invocation = run(['tests', '-c', 'specs/cleanup.spec.js', '--', process.execPath, require.resolve('@playwright/test/cli'), 'test', '--test-list', '{test-list}', '--reporter=line'])
  assert.equal(invocation.status, 0, invocation.stderr)
  assert.match(invocation.stdout, /4 passed/)
  assert.doesNotMatch(invocation.stdout, /\[public\]/)
  assert.equal(run(['tests', '-c', 'specs/cleanup.spec.js', '-f', 'playwright']).status, 2)
  const list = join(root, 'selected.txt')
  writeFileSync(list, selection.join('\n'))
  const actual = execFileSync(process.execPath, [require.resolve('@playwright/test/cli'), 'test', '--list', '--test-list', list], { cwd: root, encoding: 'utf8' })
  assert.match(actual, /\[private\] .*shared.spec.js/)
  assert.doesNotMatch(actual, /\[public\]/)
  rmSync(join(root, 'specs/seed.spec.js'))
  const deleted = run(['tests', '-b', 'HEAD', '--names'])
  assert.equal(deleted.status, 0, deleted.stderr)
  if (!deleted.stdout.includes('[private]')) {
    const inventory = await discoverTests({ cwd: root, runner: 'playwright', base: 'HEAD' })
    const report = affected({ cwd: root, entries: testEntries(inventory), base: 'HEAD', types: false, strict: true })
    assert.fail(JSON.stringify({ changed: report.changed, private: inventory.files.find(file => file.project === 'private'), output: deleted.stdout }))
  }
  assert.doesNotMatch(deleted.stdout, /\[public\]/)

  // Playwright matches the spec for collection and the declaration file for each test.
  writeFileSync(join(root, 'specs/factory.js'), "const {test}=require('@playwright/test'); exports.create=()=>test('from factory',()=>{})")
  writeFileSync(join(root, 'specs/shared.spec.js'), "require('./factory').create()")
  const unlistable = run(['tests', '-c', 'specs/factory.js', '--names'])
  assert.equal(unlistable.status, 2)
  assert.match(unlistable.stderr, /declared in a shared helper/)
  const fallback = run(['tests', '-c', 'specs/factory.js', '--', process.execPath, require.resolve('@playwright/test/cli'), 'test', '--test-list', '{test-list}', '--reporter=line'])
  assert.equal(fallback.status, 0, fallback.stderr)
  assert.match(fallback.stderr, /running everything/)
  assert.match(fallback.stdout, /4 passed/)
  assert.match(fallback.stdout, /\[public\]/)
})
