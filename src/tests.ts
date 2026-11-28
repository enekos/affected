import { execFileSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs'
import { createRequire } from 'node:module'
import { basename, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { tmpdir } from 'node:os'
import type { PlaywrightInventory } from './playwright-reporter.ts'
import { fileURLToPath, pathToFileURL } from 'node:url'
import type { Runner } from './config.ts'

export interface TestFile {
  /** Playwright’s project-qualified test-list line, relative to its configured test root. */
  readonly name?: string
  readonly project?: string
  /** The test file, relative to `cwd`. */
  readonly path: string
  /** Setup and global-setup files the runner loads with it, relative to `cwd`. */
  readonly setup: readonly string[]
}

export interface TestSuite {
  readonly runner: Runner
  readonly files: readonly TestFile[]
  /** Runner config files: a change to one can change every test. */
  readonly globals: readonly string[]
}

export interface DiscoverOptions {
  cwd?: string
  /** Defaults to the runner the package depends on. */
  runner?: Runner
  /** The runner's config file, when it is not the default. */
  config?: string
  /** The resolved git base and diff mode used to include removed Playwright setup files. */
  base?: string
  changed?: readonly string[]
  uncommitted?: boolean
}

const JEST_CONFIGS = ['jest.config.ts', 'jest.config.mts', 'jest.config.cts', 'jest.config.js', 'jest.config.mjs', 'jest.config.cjs', 'jest.config.json']
const MAX_LIST_BYTES = 256 * 1024 * 1024

function canonical(path: string): string {
  try {
    return realpathSync.native(path)
  } catch {
    return path
  }
}

/** Repository files only: setup code inside node_modules changes through the lockfile. */
function local(cwd: string, path: string): string | undefined {
  const rel = relative(canonical(cwd), canonical(isAbsolute(path) ? path : resolve(cwd, path)))
  if (rel.split(sep).includes('node_modules')) return undefined
  return rel.split(sep).join('/')
}

function manifest(cwd: string): Record<string, unknown> {
  const file = join(cwd, 'package.json')
  return existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : {}
}

/** The runner `cwd`'s package.json depends on; Vitest wins when it lists both. */
export function detectRunner(cwd = process.cwd()): Runner {
  const pkg = manifest(cwd)
  const deps = { ...(pkg.dependencies as object), ...(pkg.devDependencies as object) }
  if ('vitest' in deps) return 'vitest'
  if ('jest' in deps || 'jest' in pkg || JEST_CONFIGS.some(name => existsSync(join(cwd, name)))) return 'jest'
  if ('@playwright/test' in deps || 'playwright' in deps) return 'playwright'
  throw new Error(`no Vitest, Jest or Playwright dependency in ${join(cwd, 'package.json')}; pass --runner`)
}

function requireFrom(cwd: string) {
  return createRequire(join(cwd, 'package.json'))
}

interface VitestSpec {
  moduleId: string
  project: { config: { setupFiles: readonly string[]; globalSetup: readonly string[] }; vite: { config: { configFile?: string } } }
}

interface Vitest {
  globTestSpecifications(): Promise<VitestSpec[]>
  close(): Promise<void>
  vite: { config: { configFile?: string } }
}

async function vitest(cwd: string, config?: string): Promise<TestSuite> {
  let entry: string
  try {
    entry = requireFrom(cwd).resolve('vitest/node')
  } catch {
    throw new Error(`cannot find vitest from ${cwd}; install it or pass --runner jest`)
  }
  const { createVitest } = (await import(pathToFileURL(entry).href)) as {
    createVitest(mode: 'test', options: object, viteOverrides: object, vitestOptions: object): Promise<Vitest>
  }
  // Vitest prints config warnings on stdout, which would corrupt --format output.
  const instance = await createVitest(
    'test',
    { watch: false, root: cwd, reporters: [], ...(config && { config: resolve(cwd, config) }) },
    {},
    { stdout: process.stderr, stderr: process.stderr },
  )
  try {
    const specs = await instance.globTestSpecifications()
    const globals = new Set<string>()
    const files = new Map<string, TestFile>()
    for (const spec of specs) {
      const path = local(cwd, spec.moduleId)
      if (!path) continue
      const { setupFiles, globalSetup } = spec.project.config
      const setup = [...setupFiles, ...globalSetup].map(file => local(cwd, file)).filter(file => file !== undefined)
      const previous = files.get(path)
      files.set(path, { path, setup: [...new Set([...(previous?.setup ?? []), ...setup])] })
      const configFile = spec.project.vite.config.configFile
      if (configFile) globals.add(local(cwd, configFile) ?? configFile)
    }
    const rootConfig = instance.vite.config.configFile
    if (rootConfig) globals.add(local(cwd, rootConfig) ?? rootConfig)
    return { runner: 'vitest', files: [...files.values()].sort((a, b) => a.path < b.path ? -1 : 1), globals: [...globals].sort() }
  } finally {
    await instance.close()
  }
}

interface JestProject {
  rootDir: string
  setupFiles?: string[]
  setupFilesAfterEnv?: string[]
  globalSetup?: string | null
  globalTeardown?: string | null
  testEnvironment?: string
}

function jest(cwd: string, config?: string): TestSuite {
  let bin: string
  try {
    bin = requireFrom(cwd).resolve('jest/bin/jest')
  } catch {
    throw new Error(`cannot find jest from ${cwd}; install it or pass --runner vitest`)
  }
  const run = (flag: string) =>
    execFileSync(process.execPath, [bin, flag, ...(flag === '--listTests' ? ['--json'] : []), ...(config ? ['--config', config] : [])], {
      cwd,
      encoding: 'utf8',
      maxBuffer: MAX_LIST_BYTES,
      stdio: ['ignore', 'pipe', 'inherit'],
    })
  const tests = JSON.parse(run('--listTests')) as string[]
  const shown = JSON.parse(run('--showConfig')) as { configs: JestProject[] }
  // Jest does not report which project a test belongs to; setup files of every project apply.
  const setup = [...new Set(shown.configs.flatMap(project =>
    [...(project.setupFiles ?? []), ...(project.setupFilesAfterEnv ?? []), project.globalSetup, project.globalTeardown]
      .filter(file => typeof file === 'string')
      .map(file => local(cwd, file))
      .filter(file => file !== undefined),
  ))].sort()
  const globals = new Set<string>()
  for (const project of shown.configs) {
    for (const name of JEST_CONFIGS) if (existsSync(join(project.rootDir, name))) globals.add(local(cwd, join(project.rootDir, name)) ?? name)
  }
  if (config) globals.add(local(cwd, config) ?? basename(config))
  const files = [...new Set(tests.map(test => local(cwd, test)).filter(test => test !== undefined))].sort()
  return { runner: 'jest', files: files.map(path => ({ path, setup })), globals: [...globals].sort() }
}

function collectPlaywright(cwd: string, config?: string): PlaywrightInventory {
  const req = requireFrom(cwd)
  let bin: string
  try { bin = req.resolve('@playwright/test/cli') }
  catch { bin = req.resolve('playwright/cli') }
  const extension = import.meta.url.endsWith('.ts') ? 'ts' : 'js'
  const reporter = fileURLToPath(new URL('./playwright-reporter.' + extension, import.meta.url))
  const dir = mkdtempSync(join(tmpdir(), 'affected-playwright-'))
  try {
    const output = join(dir, 'inventory.json')
    execFileSync(process.execPath, [bin, 'test', '--list', '--reporter', reporter, ...(config ? ['--config', config] : [])], {
      cwd, stdio: ['ignore', 'ignore', 'inherit'], env: { ...process.env, AFFECTED_PLAYWRIGHT_INVENTORY: output },
    })
    return JSON.parse(readFileSync(output, 'utf8'))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

// A removed script in a prerequisite project's directory may have supplied setup on base.
function removedScripts(cwd: string, diff: DiscoverOptions): string[] {
  if (!diff.base) return (diff.changed ?? []).filter(file => !existsSync(resolve(cwd, file)))
  // Resolve the ref before passing it to diff. Git emits paths relative to the worktree root.
  const git = (args: string[]) => execFileSync('git', args, { cwd, encoding: 'utf8', maxBuffer: MAX_LIST_BYTES })
  const base = git(['rev-parse', '--verify', '--end-of-options', diff.base + '^{commit}']).trim()
  const root = git(['rev-parse', '--show-toplevel']).trimEnd()
  const args = ['diff', '--merge-base', '--no-renames', '--diff-filter=D', '--name-only', '-z', '--no-relative', base]
  if (diff.uncommitted === false) args.push('HEAD')
  return git([...args, '--']).split('\0').filter(Boolean).map(file => relative(cwd, join(root, file)).split(sep).join('/'))
}

function playwright(cwd: string, config: string | undefined, diff: DiscoverOptions): TestSuite {
  const inventory = collectPlaywright(cwd, config)
  if (inventory.sharedDeclaration) throw new Error('Playwright cannot represent tests declared in a shared helper as an exact spec-file test list: ' + inventory.sharedDeclaration)
  const globals = inventory.config ? [local(cwd, inventory.config) ?? inventory.config] : []
  const removed = inventory.projects.some(p => p.dependencies.length || p.teardown) ? removedScripts(cwd, diff) : []
  const common = [...globals, ...inventory.hooks.map(path => local(cwd, path)).filter(path => path !== undefined)]
  const files = new Map<string, TestFile>()
  for (const project of inventory.projects) {
    const setup = new Set(common)
    const pending = [...project.dependencies, ...(project.teardown ? [project.teardown] : [])]
    const visited = new Set<string>()
    for (const name of pending) {
      if (visited.has(name)) continue
      visited.add(name)
      for (const dependency of inventory.projects.filter(p => p.name === name)) {
        for (const file of removed) {
          const rel = relative(dependency.dir, resolve(cwd, file))
          if (rel === '..' || rel.startsWith('..' + sep) || isAbsolute(rel) || !/\.[cm]?[jt]sx?$/.test(rel)) continue
          if (diff.base) setup.add(file)
          else globals.push(file.replace(/[\[\]{}?*]/g, '?'))
        }
        for (const file of dependency.files) {
          const path = local(cwd, file)
          if (path) setup.add(path)
        }
        pending.push(...dependency.dependencies, ...(dependency.teardown ? [dependency.teardown] : []))
      }
    }
    for (const file of project.files) {
      const path = local(cwd, file)
      if (!path) continue
      const relativeFile = relative(inventory.root, file).split(sep).join('/')
      if (/[\r\n›]/.test(project.name + relativeFile) || relativeFile.trim() !== relativeFile || /:\d+(?::\d+)?$/.test(relativeFile))
        throw new Error('Cannot represent Playwright project or file in a test list: ' + file)
      const name = '[' + project.name + '] › ' + relativeFile
      files.set(name, { name, project: project.name, path, setup: [...new Set([...(files.get(name)?.setup ?? []), ...setup])].sort() })
    }
  }
  return { runner: 'playwright', files: [...files.entries()].sort(([a], [b]) => a < b ? -1 : 1).map(([, file]) => file), globals: [...new Set(globals)] }
}

/** Ask the project's own runner which test files it would run and what it loads with them. */
export async function discoverTests(options: DiscoverOptions = {}): Promise<TestSuite> {
  const { cwd = process.cwd(), runner, config } = options
  const root = resolve(cwd)
  const chosen = runner ?? detectRunner(root)
  let suite: TestSuite
  switch (chosen) {
    case 'vitest': suite = await vitest(root, config); break
    case 'jest': suite = jest(root, config); break
    case 'playwright': suite = playwright(root, config, options); break
  }
  if (!suite.files.length) throw new Error(`${chosen} found no test files in ${root}`)
  return suite
}

/** Setup files are extra roots. Playwright entries retain the configured project. */
export function testEntries(suite: TestSuite): Record<string, string[]> {
  return Object.fromEntries(suite.files.map(file => [file.name ?? file.path, [file.path, ...file.setup.filter(setup => setup !== file.path)]]))
}

