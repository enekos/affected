#!/usr/bin/env bash
# Publish the version in package.json to npm from this machine.
#
#   scripts/publish.sh [otp]
#
# Run it on a clean checkout of tag v<version>, after the Release workflow has created the GitHub
# release with the binaries attached. Without a 2FA code, each npm login page opens in the browser;
# approve it and publishing continues. Packages already on npm are skipped, so if a login times out,
# run it again.
set -euo pipefail

otp_args=()
[[ -n ${1:-} ]] && otp_args=(--otp "$1")
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

version=$(node -p "require('./package.json').version")
tag="v$version"
dist_tag=latest
[[ $version == *-* ]] && dist_tag=next

test -z "$(git status --porcelain)" || { echo "working tree is not clean" >&2; exit 1; }
test "$(git rev-parse HEAD)" = "$(git rev-parse "$tag^{commit}")" || { echo "HEAD is not $tag" >&2; exit 1; }

work=$(mktemp -d)
log="$work/npm.log"
touch "$log"

# npm waits for a browser login only on a terminal: publish under script(1) and open each login link.
# shellcheck disable=SC2013
( opened=" "
  while sleep 1; do
    for url in $(grep -o 'https://www.npmjs.com/auth/cli/[A-Za-z0-9_-]*' "$log" | sort -u); do
      [[ $opened == *" $url "* ]] && continue
      open "$url" 2>/dev/null || xdg-open "$url" 2>/dev/null || echo "log in to npm: $url" >&2
      opened="$opened$url "
    done
  done ) &
watcher=$!
trap 'kill "$watcher" 2>/dev/null; git checkout -q -- package.json npm; rm -f ./*.node; rm -rf "$work"' EXIT

gh release download "$tag" --pattern '*.node' --dir "$work/bindings"
pnpm install --frozen-lockfile
pnpm napi artifacts --output-dir "$work/bindings" --npm-dir npm
pnpm build:ts

# shellcheck disable=SC2016
node -e '
  const fs = require("fs");
  const pkg = JSON.parse(fs.readFileSync("package.json"));
  pkg.optionalDependencies = Object.fromEntries(
    fs.readdirSync("npm").sort().map((dir) => [require(`./npm/${dir}/package.json`).name, pkg.version]),
  );
  fs.writeFileSync("package.json", JSON.stringify(pkg, null, 2) + "\n");
'

mkdir -p "$work/platform" "$work/root"
for dir in npm/*/; do npm pack "./$dir" --pack-destination "$work/platform" >/dev/null; done
npm pack --ignore-scripts --pack-destination "$work/root" >/dev/null

# Platform packages first: the root package's optionalDependencies must resolve.
for tgz in "$work"/platform/*.tgz "$work"/root/*.tgz; do
  name=$(tar -xOzf "$tgz" package/package.json | node -p 'JSON.parse(require("fs").readFileSync(0)).name')
  if npm view "$name@$version" version >/dev/null 2>&1; then
    echo "skip $name@$version (already published)"
    continue
  fi
  # npm stages a new version for minutes before npm view sees it, so trust its exit code.
  script -q /dev/null npm publish "$tgz" --access public --tag "$dist_tag" --auth-type web \
    ${otp_args[@]+"${otp_args[@]}"} </dev/null >>"$log" 2>&1 && continue
  tail -40 "$log" | grep -q 'previously published' && continue
  tail -20 "$log" >&2
  echo "publishing $name failed; run again" >&2
  exit 1
done

echo "published @enekos/affected@$version ($dist_tag)"
