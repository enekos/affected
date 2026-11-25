import { createRequire } from 'node:module'
import type * as Native from '../native.cjs'

const native: typeof Native = createRequire(import.meta.url)('../native.cjs')

/** A tsconfig path, or one or more source files resolved with their nearest tsconfig. */
export type EntryInput = string | readonly string[]

export interface AffectedOptions<Name extends string = string> {
  /** Named starting points of the import graph. */
  entries: Readonly<Record<Name, EntryInput>>
  /** Compare against the fork point of this ref. Changed files come from git unless `changed` is set. */
  base?: string
  /** The changed files, relative to `cwd`. Required without `base`. */
  changed?: readonly string[]
  /** Directory paths are relative to. Defaults to `process.cwd()`. */
  cwd?: string
  /** With `base`, also count uncommitted and untracked files. Defaults to `true`. */
  uncommitted?: boolean
  /** Follow `import type` edges. Defaults to `true`. */
  types?: boolean
}

export interface Entry<Name extends string = string> {
  readonly name: Name
  /** Whether any changed file reaches this entry. */
  readonly hit: boolean
  /** The changed files this entry reaches. */
  readonly hits: readonly string[]
  /** The shortest import chain from an entry root to a changed file. Empty when not hit. */
  readonly chain: readonly string[]
  /** Every file the entry reaches on either side of the diff. */
  readonly files: readonly string[]
}

export class Affected<Name extends string = string> {
  /** The changed files considered, relative to `cwd`. */
  readonly changed: readonly string[]
  readonly entries: readonly Entry<Name>[]
  readonly #byName: ReadonlyMap<Name, Entry<Name>>

  constructor(report: Native.NativeReport) {
    this.changed = report.changed
    this.entries = report.entries.map(entry => ({
      name: entry.name as Name,
      hit: entry.hits.length > 0,
      hits: entry.hits,
      chain: entry.chain,
      files: entry.files,
    }))
    this.#byName = new Map(this.entries.map(entry => [entry.name, entry]))
  }

  /** The names of the entries the diff reaches, in declaration order. */
  get hit(): Name[] {
    return this.entries.filter(entry => entry.hit).map(entry => entry.name)
  }

  /** Whether the diff reaches any entry. */
  get any(): boolean {
    return this.entries.some(entry => entry.hit)
  }

  has(name: Name): boolean {
    return this.entry(name).hit
  }

  /** The import chain explaining why `name` is hit, or `undefined` when it is not. */
  why(name: Name): readonly string[] | undefined {
    const { chain } = this.entry(name)
    return chain.length ? chain : undefined
  }

  entry(name: Name): Entry<Name> {
    const entry = this.#byName.get(name)
    if (!entry) throw new Error(`Unknown entry "${name}"`)
    return entry
  }

  /** One line per entry: `name ← changed file (via a → b)`. */
  toString(): string {
    return this.entries.map(describe).join('\n')
  }
}

function describe(entry: Entry): string {
  if (!entry.hit) return `${entry.name}  unaffected`
  const changed = entry.chain.at(-1)
  const via = entry.chain.slice(0, -1)
  const others = entry.hits.length > 1 ? ` (+${entry.hits.length - 1} more)` : ''
  return `${entry.name}  ← ${changed}${others}${via.length ? `  via ${via.join(' → ')}` : ''}`
}

function toNative({ cwd, entries, base, changed, uncommitted, types }: AffectedOptions): Native.NativeOptions {
  return {
    cwd: cwd ?? process.cwd(),
    entries: Object.entries<EntryInput>(entries).map(([name, input]) => ({
      name,
      paths: typeof input === 'string' ? [input] : [...input],
    })),
    ...(base !== undefined && { base }),
    ...(changed !== undefined && { changed: [...changed] }),
    ...(uncommitted !== undefined && { uncommitted }),
    ...(types !== undefined && { types }),
  }
}

/**
 * Which entries does the diff reach?
 *
 * ```ts
 * const r = affected({ entries: { jobs: 'packages/jobs/tsconfig.json' }, base: 'origin/main' })
 * if (r.has('jobs')) console.log(r.why('jobs'))
 * ```
 */
export function affected<const Name extends string>(options: AffectedOptions<Name>): Affected<Name> {
  return new Affected(native.analyze(toNative(options)))
}

/** {@link affected} off the main thread. */
export async function affectedAsync<const Name extends string>(options: AffectedOptions<Name>): Promise<Affected<Name>> {
  return new Affected(await native.analyzeAsync(toNative(options)))
}
