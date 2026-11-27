import { existsSync, readFileSync } from 'node:fs'
import { extname, join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import type { EntryInput } from './index.ts'

export type Runner = 'vitest' | 'jest'

export interface AffectedConfig<Name extends string = string> {
  /** Named starting points of the import graph, as in `affected({ entries })`. */
  entries?: Readonly<Record<Name, EntryInput>>
  /** Globs whose changes hit every entry. */
  globals?: readonly string[]
  /** The default base ref; `'auto'` picks the CI target branch. */
  base?: string
  strict?: boolean
  types?: boolean
  /** How `affected tests` finds test files. */
  tests?: { readonly runner?: Runner; readonly config?: string }
}

/** Type the config file: entry names stay literal, so `r.has('jbos')` is still a type error. */
export function defineConfig<const Name extends string>(config: AffectedConfig<Name>): AffectedConfig<Name> {
  return config
}

export const CONFIG_FILES = [
  'affected.config.ts',
  'affected.config.mts',
  'affected.config.js',
  'affected.config.mjs',
  'affected.config.json',
] as const

const KEYS = new Set(['entries', 'globals', 'base', 'strict', 'types', 'tests'])

function strings(value: unknown): value is string[] {
  return Array.isArray(value) && value.every(item => typeof item === 'string')
}

function entryInput(value: unknown): value is EntryInput {
  if (typeof value === 'string' || strings(value)) return true
  if (typeof value !== 'object' || value === null) return false
  const { from, inputs, ...rest } = value as Record<string, unknown>
  return (typeof from === 'string' || strings(from)) && (inputs === undefined || strings(inputs)) && !Object.keys(rest).length
}

/** Check a loaded config's shape; `file` names it in errors. */
export function parseConfig(value: unknown, file: string): AffectedConfig {
  const invalid = (message: string): never => {
    throw new Error(`${file}: ${message}`)
  }
  if (typeof value !== 'object' || value === null || Array.isArray(value)) invalid('expected an object')
  const config = value as Record<string, unknown>
  for (const key of Object.keys(config)) if (!KEYS.has(key)) invalid(`unknown key "${key}"`)
  const { entries, globals, base, strict, types, tests } = config
  if (entries !== undefined) {
    if (typeof entries !== 'object' || entries === null || Array.isArray(entries)) invalid('"entries" must be an object')
    for (const [name, input] of Object.entries(entries as object)) {
      if (!entryInput(input)) invalid(`entry "${name}" must be a path, a list of paths or { from, inputs }`)
    }
  }
  if (globals !== undefined && !strings(globals)) invalid('"globals" must be a list of globs')
  if (base !== undefined && typeof base !== 'string') invalid('"base" must be a string')
  if (strict !== undefined && typeof strict !== 'boolean') invalid('"strict" must be a boolean')
  if (types !== undefined && typeof types !== 'boolean') invalid('"types" must be a boolean')
  if (tests !== undefined) {
    if (typeof tests !== 'object' || tests === null) invalid('"tests" must be an object')
    const { runner, config: runnerConfig, ...rest } = tests as Record<string, unknown>
    if (runner !== undefined && runner !== 'vitest' && runner !== 'jest') invalid('"tests.runner" must be "vitest" or "jest"')
    if (runnerConfig !== undefined && typeof runnerConfig !== 'string') invalid('"tests.config" must be a path')
    for (const key of Object.keys(rest)) invalid(`unknown key "tests.${key}"`)
  }
  return config as AffectedConfig
}

/** The config file in `cwd`, if there is one. */
export function findConfig(cwd = process.cwd()): string | undefined {
  return CONFIG_FILES.map(name => join(cwd, name)).find(path => existsSync(path))
}

/** Load `file`, or the `affected.config.*` in `cwd`. Resolves to `undefined` when there is none. */
export async function loadConfig(cwd = process.cwd(), file?: string): Promise<{ path: string; config: AffectedConfig } | undefined> {
  const path = file ? resolve(cwd, file) : findConfig(cwd)
  if (!path) return undefined
  if (extname(path) === '.json') return { path, config: parseConfig(JSON.parse(readFileSync(path, 'utf8')), path) }
  let module: { default?: unknown; config?: unknown }
  try {
    module = await import(pathToFileURL(path).href)
  } catch (error) {
    if (error instanceof Error && 'code' in error && error.code === 'ERR_UNKNOWN_FILE_EXTENSION') {
      throw new Error(`${path}: this Node.js cannot load TypeScript; use Node >= 22.18 or affected.config.mjs/.json`)
    }
    throw error
  }
  return { path, config: parseConfig(module.default ?? module.config, path) }
}
