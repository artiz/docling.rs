#!/usr/bin/env bash
#
# Master-only release step (run by .github/workflows/ci.yml after the lint/test
# gates pass). Computes the next version from conventional commits; if there is
# one, it bumps the workspace version, commits + tags it, pushes to master, and
# publishes the changed crates. A clean no-op when no release-worthy commit
# landed since the last tag.
#
# The release commit is pushed with the workflow's GITHUB_TOKEN and carries
# `[skip ci]`, so it does not re-trigger CI (no release loop).
#
# Requires: CARGO_REGISTRY_TOKEN (publish) and push access to master.
# Usage: scripts/release.sh
set -euo pipefail
cd "$(dirname "$0")/.."

new="$(scripts/bump_version.sh)"
if [[ -z "$new" ]]; then
  echo "No release-worthy commits since the last tag — nothing to release."
  exit 0
fi

current="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
echo ">> releasing v$new (was v$current)"

# Bump the single version key in the root manifest's [workspace.package].
sed -i -E "0,/^version = \"[^\"]+\"/ s//version = \"$new\"/" Cargo.toml

git config user.name "github-actions[bot]"
git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
git add Cargo.toml
git commit -m "chore(release): v$new [skip ci]"
git tag -a "v$new" -m "v$new"
git push origin HEAD:master
git push origin "v$new"

# Publish every crate at the new version (idempotent: skips any already on
# crates.io), in dependency order.
scripts/ci_publish.sh

echo ">> released v$new"
