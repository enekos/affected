#!/usr/bin/env bash
# Publish the version in package.json to npm from this machine.
#
#   scripts/publish.sh <otp>
#
# Run it on a clean checkout of tag v<version>, after the Release workflow has created the GitHub
# release with the binaries attached. Packages already on npm are skipped, so when the 2FA code
# expires part-way, run it again with a fresh one.
set -euo pipefail

otp=${1:?usage: scripts/publish.sh <npm 2FA code>}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

version=$(node -p "require('./package.json').version")
tag="v$version"
dist_tag=latest
[[ $version == *-* ]] && dist_tag=next

test -z "$(git status --porcelain)" || { echo "working tree is not clean" >&2; exit 1; }
test "$(git rev-parse HEAD)" = "$(git rev-parse "$tag^{commit}")" || { echo "HEAD is not $tag" >&2; exit 1; }

work=$(mktemp -d)
trap 'git checkout -q -- package.json npm; rm -f ./*.node; rm -rf "$work"' EXIT

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
  npm publish "$tgz" --access public --tag "$dist_tag" --otp "$otp"
done

echo "published @enekos/affected@$version ($dist_tag)"
