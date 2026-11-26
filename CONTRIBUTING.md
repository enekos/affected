# Contributing

Thanks for helping. Issues and pull requests are welcome; for anything larger than a fix, open an
issue first so we can agree on the shape.

## Layout

| Path | |
|---|---|
| `crates/affected-core` | The analysis: tsconfig loading, import extraction (oxc), resolution (oxc_resolver), git, graphs. |
| `crates/affected-node` | napi-rs bindings. Thin. |
| `src/` | TypeScript wrapper (`index.ts`) and CLI (`cli.ts`). |
| `npm/` | Per-platform packages that carry the prebuilt `.node` binaries. |
| `docs/` | The documentation site (Jekyll, deployed to GitHub Pages). |

## Setup

Rust (stable, MSRV 1.98), Node ≥ 22 and pnpm.

```sh
pnpm install
pnpm build          # native module + TypeScript
```

## Before you open a pull request

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
pnpm build && pnpm typecheck && pnpm test
```

- **A behaviour change needs a test.** Resolution and diff semantics are tested against real git
  repositories in `crates/affected-core/tests/analyze.rs`; the JS API and CLI in `test/`.
- **Check the oracle when you touch resolution.** On a real repository, an entry's `files` must contain
  every in-repo file `tsc --listFilesOnly -p <tsconfig>` lists:

  ```sh
  node -e "import('./dist/index.js').then(({affected}) => console.log(affected({ cwd: process.argv[1], entries: { e: 'tsconfig.json' }, changed: [] }).entry('e').files.join('\n')))" /path/to/repo
  ```
- **Fail safe.** When unsure whether a change can reach an entry, report a hit. A wrong "no" ships
  untested code; a wrong "yes" costs CI minutes.
- Add a line to `CHANGELOG.md` under *Unreleased*.

## Releasing

1. Bump the version in `Cargo.toml` (`workspace.package`) and run `pnpm version <x.y.z> --no-git-tag-version`
   (napi updates `npm/*`).
2. Move *Unreleased* in `CHANGELOG.md` to a `## [x.y.z] - YYYY-MM-DD` section.
3. Merge, then tag: `git tag vx.y.z && git push origin vx.y.z`.
4. Wait for the `Release` workflow, then publish from a clean checkout of the tag:
   `git checkout vx.y.z && scripts/publish.sh <npm 2FA code>`.

The `Release` workflow checks the versions, runs CI, and creates the GitHub release with the binaries
attached. It does not publish to npm. `scripts/publish.sh` downloads those binaries, packs the seven
platform packages and the root package, and publishes them in that order. It skips packages already on
npm, so if the 2FA code expires part-way, run it again with a fresh code.
