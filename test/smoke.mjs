// Smoke test of the compiled package: runs on any supported Node without type stripping.
import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { affected, affectedAsync, globEntries } from '../dist/index.js'

const cli = fileURLToPath(new URL('../dist/cli.js', import.meta.url))
const root = mkdtempSync(join(tmpdir(), 'affected-smoke-'))
try {
  const write = files => {
    for (const [path, contents] of Object.entries(files)) {
      mkdirSync(dirname(join(root, path)), { recursive: true })
      writeFileSync(join(root, path), contents)
    }
  }
  const git = (...args) => execFileSync('git', ['-C', root, ...args], { stdio: 'pipe' })
  git('init', '-q', '-b', 'main')
  git('config', 'user.email', 'smoke@localhost')
  git('config', 'user.name', 'Smoke')
  write({ 'tsconfig.json': '{ "files": ["src/index.ts"] }', 'src/index.ts': "import './a'", 'src/a.ts': 'export {}', 'other.ts': '' })
  git('add', '-A')
  git('commit', '-qm', 'base')
  write({ 'src/a.ts': 'export const a = 1' })

  const result = affected({ cwd: root, entries: { app: 'tsconfig.json', other: ['other.ts'] }, base: 'main' })
  assert.deepEqual(result.hit, ['app'])
  assert.deepEqual(result.why('app'), ['src/index.ts', 'src/a.ts'])
  assert.deepEqual((await affectedAsync({ cwd: root, entries: { app: 'tsconfig.json' }, changed: ['src/a.ts'] })).hit, ['app'])

  const run = spawnSync(process.execPath, [cli, '-e', 'app=tsconfig.json', '-b', 'main', '--names', '--exit-code'], {
    cwd: root,
    encoding: 'utf8',
  })
  assert.equal(run.status, 0, run.stderr)
  assert.equal(run.stdout, 'app\n')
  assert.deepEqual(Object.keys(globEntries('src/*.ts', root)), ['src/a.ts', 'src/index.ts'])
  const paths = spawnSync(process.execPath, [cli, '-e', 'src/*.ts', '-b', 'main', '-f', 'vitest'], { cwd: root, encoding: 'utf8' })
  assert.equal(paths.stdout, '--passWithNoTests\nsrc/a.ts\nsrc/index.ts\n', paths.stderr)
  console.log(`smoke ok: node ${process.version} ${process.platform}-${process.arch}`)
} finally {
  rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 })
}
