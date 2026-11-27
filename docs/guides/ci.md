---
title: CI
parent: Guides
nav_order: 1
---

# Using affected in CI
{: .no_toc }

1. TOC
{:toc}

## Fetch enough history

`affected` needs the merge base of your branch and its target. A default shallow checkout has none, and
`affected` stops with an error rather than guessing:

```
affected: no merge base with origin/main: this is a shallow clone. Fetch history first …
```

On GitHub Actions use `fetch-depth: 0` (add `filter: blob:none` on large repositories; base blobs are read
on demand).

## GitHub Actions: skip a job

```yaml
jobs:
  changes:
    runs-on: ubuntu-latest
    outputs:
      affected: ${{ steps.affected.outputs.affected }}
      any: ${{ steps.affected.outputs.any }}
    steps:
      - uses: actions/checkout@v4
        with: { fetch-depth: 0 }
      - uses: actions/setup-node@v4
        with: { node-version: 22 }
      - id: affected
        run: >
          npx -y @enekos/affected --strict --base auto --github-output
          -e api=apps/api/tsconfig.json
          -e web=apps/web/tsconfig.json
          -e prompts=packages/prompts/tsconfig.json

  prompt-evals:
    needs: changes
    if: contains(fromJSON(needs.changes.outputs.affected), 'prompts')
    runs-on: ubuntu-latest
    steps:
      - run: echo "prompts changed"
```

`--base auto` uses the pull request's target branch (`GITHUB_BASE_REF`), so the same step works on every
PR. `--github-output` writes `affected` (a JSON array of hit entry names) and `any` (`true`/`false`).

## GitHub Actions: a matrix of hit entries

```yaml
  build:
    needs: changes
    if: needs.changes.outputs.any == 'true'
    strategy:
      matrix:
        app: ${{ fromJSON(needs.changes.outputs.affected) }}
    runs-on: ubuntu-latest
    steps:
      - run: echo "building ${{ matrix.app }}"
```

## GitLab CI

```yaml
prompt-evals:
  variables:
    GIT_DEPTH: 0
  script:
    - |
      status=0
      npx -y @enekos/affected --strict --base auto -e prompts=packages/prompts/tsconfig.json --exit-code || status=$?
      case "$status" in
        0) pnpm eval:prompts ;;
        1) : ;;
        *) exit "$status" ;;
      esac
```

`--base auto` reads `CI_MERGE_REQUEST_TARGET_BRANCH_NAME`. With `--exit-code`, the command exits `0`
when an entry is hit, `1` when none is, and `2` on an error — never treat `2` as "skip".

## Base detection

`--base auto` (or `base: 'auto'`) resolves, in order: `AFFECTED_BASE`, `GITHUB_BASE_REF`,
`CI_MERGE_REQUEST_TARGET_BRANCH_NAME`, `BUILDKITE_PULL_REQUEST_BASE_BRANCH`,
`BITBUCKET_PR_DESTINATION_BRANCH`, `SYSTEM_PULLREQUEST_TARGETBRANCH` — each as `origin/<branch>` — and
falls back to `origin/HEAD`. On pushes to `main` set `AFFECTED_BASE` to the previous commit (for example
`${{ github.event.before }}`).

## Inputs no import reveals

Declare runtime inputs such as generated code or template directories with per-entry globs.

```sh
npx affected -e api=apps/api/tsconfig.json -i 'api=apps/api/templates/**' -g pnpm-lock.yaml -b auto
```

`-g` hits every entry; `-i name=glob` hits one.
