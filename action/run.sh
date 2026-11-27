#!/usr/bin/env bash
# The GitHub Action's step. Inputs arrive as INPUT_* variables; see action.yml.
set -euo pipefail

base=$INPUT_BASE
if [ "$base" = auto ] && [ -z "${AFFECTED_BASE:-}" ] && [ -z "${GITHUB_BASE_REF:-}" ] &&
  [ -n "${EVENT_BEFORE:-}" ] && [ "${EVENT_BEFORE//0/}" != "" ]; then
  # A push has no target branch: compare with the commit the branch pointed at before it.
  base=$EVENT_BEFORE
fi

if [ "$INPUT_FETCH" = true ] && [ "$(git rev-parse --is-shallow-repository)" = true ]; then
  # Fetch the base, then deepen both sides until they share a commit.
  case $base in
    auto) branch=${GITHUB_BASE_REF:-} ;;
    origin/*) branch=${base#origin/} ;;
    *) branch= ;;
  esac
  if [ -n "$branch" ]; then
    refspec=("+refs/heads/$branch:refs/remotes/origin/$branch")
    target=origin/$branch
  elif [ "$base" != auto ]; then
    refspec=("$base")
    target=$base
  else
    refspec=()
    target=
  fi
  if [ -n "$target" ]; then
    head=$(git rev-parse HEAD)
    git fetch --no-tags --quiet --depth=1 origin "${refspec[@]}" || true
    for depth in 50 200 1000; do
      git merge-base HEAD "$target" >/dev/null 2>&1 && break
      git fetch --no-tags --quiet --deepen="$depth" origin "$head" "${refspec[@]}" || true
    done
    git merge-base HEAD "$target" >/dev/null 2>&1 || git fetch --no-tags --quiet --unshallow origin || true
  fi
fi

if [ -n "${AFFECTED_BIN:-}" ]; then
  read -ra cli <<<"$AFFECTED_BIN"
else
  version=${INPUT_VERSION:-$(node -p "require(process.env.GITHUB_ACTION_PATH + '/package.json').version")}
  cli=(npx --yes "@enekos/affected@$version")
fi

args=()
[ "$INPUT_ENTRIES" = tests ] && args+=(tests)
args+=(--base "$base" --github-output)
[ "$INPUT_SUMMARY" = true ] && args+=(--github-summary)
read -ra extra <<<"$INPUT_ARGS"
args+=(${extra[@]+"${extra[@]}"})

if [ -n "$INPUT_RUN" ]; then
  read -ra command <<<"$INPUT_RUN"
  exec "${cli[@]}" "${args[@]}" -- "${command[@]}"
fi
exec "${cli[@]}" "${args[@]}"
