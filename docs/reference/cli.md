---
title: CLI
parent: Reference
nav_order: 2
---

# CLI

```
affected --entry <name>=<tsconfig.json | file,file…> [options]
```

| Flag | |
|---|---|
| `-e, --entry <name>=<paths>` | An entry; repeat for several. Comma-separate several files. |
| `-b, --base <ref>` | Compare against the fork point of `<ref>`; `auto` picks the CI target branch. |
| `-c, --changed <file>` | A changed file; repeat, or `-` to read lines from stdin. |
| `--committed` | With `--base`, ignore uncommitted and untracked files. |
| `--no-types` | Do not follow `import type`. |
| `-g, --global <glob>` | A changed file matching `<glob>` hits every entry. |
| `-i, --input <name>=<glob>` | A changed file matching `<glob>` hits entry `<name>`. |
| `--strict` | Select entries with unresolved dependencies on a nonempty diff. |
| `--config <file>` | Read entries, globals and defaults from JSON. Defaults to `affected.config.json` in the working directory. |
| `--trace <file>` | Load a JSON array of dependency edges with `from`, `to` and optional `revision` (`head` or `base`). Paths are relative to the working directory. |
| `--shadow-out <file>` | Save the proposed test selection. Use the test file as the first root of each entry. |
| `--compare-shadow <plan> --results <runner.json>` | Compare against a full Vitest or Jest JSON report. Exit 1 for missed failures or missing candidate results, 2 for invalid input, and 0 for agreement. |
| `--json` | Print the full report as JSON. |
| `--names` | Print hit entry names, one per line. |
| `--github-output` | Append `affected=<JSON array>` and `any=<bool>` to `$GITHUB_OUTPUT`. |
| `--exit-code` | Exit `0` when an entry is hit, `1` when none is. |
| `-h, --help` | Help. |
| `-V, --version` | Version (standalone binary only). |

Errors always exit `2`.

## Config file

`affected.config.json` holds what CI and local runs share:

```json
{
  "entries": {
    "api": "apps/api/tsconfig.json",
    "web": ["src/main.ts", "src/worker.ts"],
    "prompts": { "from": "packages/prompts/tsconfig.json", "inputs": ["packages/prompts/**/*.md"] }
  },
  "globals": ["pnpm-lock.yaml"],
  "base": "auto",
  "strict": true
}
```

Every key is optional; `types: false` matches `--no-types`. Paths are relative to the working
directory. Flags win: `--entry` replaces a config entry of the same name (its inputs too) or adds one,
`--input` and `--global` add to the config's, and `--base`, `--strict` and `--no-types` override it.
Unknown keys are errors.

## Examples

```sh
# Human-readable
npx affected -e jobs=packages/jobs/tsconfig.json -e web=src/main.ts -b origin/main
# jobs  ← server/jobs/limits.ts  via packages/jobs/index.ts → packages/jobs/build.ts
# web  unaffected

# Gate a step
status=0
npx affected --strict -e jobs=packages/jobs/tsconfig.json -b auto --exit-code || status=$?
case "$status" in
  0) pnpm eval ;;
  1) : ;;
  *) exit "$status" ;;
esac

# Changed files from anywhere (Bash)
set -o pipefail
git diff --name-only HEAD~3 | npx affected -e jobs=packages/jobs/tsconfig.json -c -

# Feed another tool
npx affected -e a=apps/a/tsconfig.json -e b=apps/b/tsconfig.json -b auto --names | xargs -I{} pnpm --filter {} build
```

## Install without Node

The standalone `affected` binary has the same flags, output and exit codes as `npx affected`, minus
`--shadow-out` and `--compare-shadow`, which stay in the Node CLI. Use it on Bun, Deno or CI images
without Node.

Each [GitHub release](https://github.com/enekos/affected/releases) has `affected-<target>.tar.gz`
(`.zip` on Windows) with a `.sha256` next to it, for `x86_64-unknown-linux-musl` and
`aarch64-unknown-linux-musl` (static, any Linux), `aarch64-apple-darwin`, `x86_64-apple-darwin` and
`x86_64-pc-windows-msvc`.

```sh
# Linux x64, any distribution
version=0.2.0
curl -fsSLO "https://github.com/enekos/affected/releases/download/v$version/affected-x86_64-unknown-linux-musl.tar.gz"
curl -fsSL "https://github.com/enekos/affected/releases/download/v$version/affected-x86_64-unknown-linux-musl.tar.gz.sha256" | sha256sum -c
tar -xzf affected-x86_64-unknown-linux-musl.tar.gz affected && sudo mv affected /usr/local/bin/

# With cargo-binstall: downloads the release binary
cargo binstall --git https://github.com/enekos/affected affected-ts

# From source (Rust 1.98+)
cargo install --locked --git https://github.com/enekos/affected affected-ts
```
