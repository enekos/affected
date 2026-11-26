# Roadmap

`affected` answers one question: which named entry points can a diff change? A wrong "no" ships an
untested change, while a wrong "yes" only costs CI time. So correctness comes before reach, and reach
comes before speed.

## Where it stands (2026-11-26)

- **Head graph matches `tsc --listFilesOnly`.** Across 32 tsconfigs in three production monorepos
  (data, gateway, hris), no tsc file was missing. The only extras are assets an app really loads
  (CSS, images, files read through `__dirname`) and workspace manifests.
- **Speed:** 6–8× faster than `tsc --listFilesOnly` (TS 7) on a single entry. 1,183 entries (one per
  test file) take ~220 ms with the fork point.
- **Edge-case suite:** 26 of 27 scenarios pass (renames, deletions, workspace symlinks, package
  `extends`, project references, globs, assets, SFCs, mocks, dependency bumps, shallow clones, inputs).
  The one failure is below.

## 0.1 — ship

- [ ] Green CI on all 7 napi targets, then tag `v0.1.0` and publish `@enekos/affected` from a local
      machine (`scripts/publish.sh`).
- [ ] Publish `affected-core` to crates.io, plus a `affected` Rust binary (`cargo binstall`) for
      non-Node CI.
- [ ] A GitHub Actions example in the README: `fetch-depth: 0`, `--base auto`, `--github-output`.

## 0.2 — say "unknown" instead of guessing (fail closed)

Bazel's target-determinator rule: missing evidence is "unknown", never "unaffected".

- [ ] **Diagnostics in the report:** relative imports that don't resolve, `import(variable)` with no
      static part, globs with no static directory, and case mismatches (`./Foo` resolving to `foo.ts`
      on macOS or Windows, which breaks on Linux CI and is the one scenario that still fails).
- [ ] **`unknown` per entry** when one of its files has an unresolved dynamic edge. `--strict` counts
      unknown as a hit; `--exit-code` gets a distinct code for it.
- [ ] **Submodules:** a gitlink change counts as "everything below the submodule changed".
- [ ] **Blobless clones** (`filter: blob:none`): prefetch the base blobs the walk needs in one
      `git cat-file --batch` round, not a lazy fetch per object.
- [ ] **Lockfile-aware dependencies:** diff `pnpm-lock.yaml`, `package-lock.json` and `yarn.lock` per
      importer, and hit the `<manifest>#<dep>` node when a resolved version changes, even when the range
      in package.json doesn't. This is Nx `projectsAffectedByDependencyUpdates: auto` and Rush's
      shrinkwrap slices. Today lockfile-only changes need `globals`.
- [ ] **Base-side install:** the fork-point graph reads `node_modules` from the working tree. Resolve
      workspace packages from `pnpm-workspace.yaml` and package.json `workspaces` at the base revision
      instead.

## 0.3 — test selection and CI ergonomics

- [ ] **Glob entries:** `--entries '**/*.test.ts'` makes one entry per match, named by path. This is the
      `jest --findRelatedTests` / `vitest related` use case without listing files.
- [ ] **Runner output:** `--format jest|vitest|paths` prints the hit test files as runner arguments, like
      moon `--affected`.
- [ ] **Config file:** `affected.config.{ts,json}` holds entries, globals and inputs, so CI and local runs
      share one definition (Nx `namedInputs`, Turbo `globalDependencies`).
- [ ] **Matrix sharding:** `--github-matrix [--shards N]` splits hit entries into balanced shards.
- [ ] **Last-green base:** `--base last-green` (Nx `nx-set-shas`) asks the GitHub API for the last
      successful run on the target branch.
- [ ] **Explain:** `--explain <entry>` prints every chain to every hit, not just the shortest. Add
      `--graph dot|mermaid|json` export (dependency-cruiser, madge).

## 0.4 — monorepo scale

- [ ] **Workspace auto-entries:** every pnpm/npm/yarn workspace package becomes an entry, from its
      `exports`/`main`/`bin` and its tsconfig. Add Turbo-style selectors (`...pkg` for dependents).
- [ ] **Persistent parse cache keyed by blob OID** (skott `.skott`, jest haste map), so unchanged files
      parse once across runs and across head and base.
- [ ] **Watch mode:** keep the graph warm (Nx daemon) and re-answer on file events, for editor and
      pre-commit use.
- [ ] **Path interning:** `u32` node ids instead of `PathBuf` keys. The 1,183-entry case spends most of
      its time hashing paths.

## Later: more edges

- [ ] CSS `@import`/`url()`, Sass `@use`, GraphQL `#import`, HTML `<script src>` (Vite `index.html` as an
      entry). esbuild and Vite cover these through loaders.
- [ ] Bundler aliases outside tsconfig (`vite.config` `resolve.alias`, webpack `resolve.alias`), read
      from config, not by running it.
- [ ] `compilerOptions.types` packages and ambient `.d.ts` files as inputs of every root, as in
      `tsc --explainFiles`.
- [ ] `fs.readFile('./x')` relative to `process.cwd()` and `@vercel/nft`-style traced reads, behind a
      flag since they guess.
- [ ] Yarn PnP.

## Non-goals

- **Running tasks or caching outputs.** That's Nx, Turbo and moon. `affected` feeds them, or a plain
  `if`.
- **Type-level precision** (`tsc --build` signature skipping). A change that cannot alter a `.d.ts` still
  counts. Over-selecting is the safe side.
- **Project granularity.** Entries are files and tsconfigs, not packages. Callers group entries the way
  they need.

## Inspiration

Nx affected (inputs, lockfile awareness, `--explain`), Turborepo (`--affected`, SCM base env vars),
jest/vitest related-tests, target-determinator and bazel-diff (both revisions, fail closed), Pants
`--changed-dependents`, dependency-cruiser `--affected`, rev-dep (entry points reaching a file), fallow
and knip (globs, assets, entry discovery on oxc), `@vercel/nft` (traced file reads).
