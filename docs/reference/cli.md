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
| `--trace <file>` | Load a JSON array of dependency edges with `from`, `to` and optional `revision` (`head` or `base`). Paths are relative to the working directory. |
| `--shadow-out <file>` | Save the proposed test selection. Use the test file as the first root of each entry. |
| `--compare-shadow <plan> --results <runner.json>` | Compare against a full Vitest or Jest JSON report. Exit 1 for missed failures or missing candidate results, 2 for invalid input, and 0 for agreement. |
| `--json` | Print the full report as JSON. |
| `--names` | Print hit entry names, one per line. |
| `--github-output` | Append `affected=<JSON array>` and `any=<bool>` to `$GITHUB_OUTPUT`. |
| `--exit-code` | Exit `0` when an entry is hit, `1` when none is. |
| `-h, --help` | Help. |

Errors always exit `2`.

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
