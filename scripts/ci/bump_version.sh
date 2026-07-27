#!/usr/bin/env bash
#
# Decide the next workspace version from the commit messages since the last
# release tag, and print it to stdout. Prints NOTHING (exit 0) when the range
# is empty, so the caller can skip the release.
#
#   only fix:/perf:/revert: commits in the range   -> patch
#   anything else                                  -> minor (the 0.x "major")
#
# Releases deliberately do NOT depend on conventional-commit prefixes: the
# repo's de-facto style is bare area prefixes ("pdf: …", "wasm: …"), and
# requiring feat:/fix: silently stopped every release after v0.49.0. Any
# non-fix merge now bumps 0.49 -> 0.50; a docs/CI-only merge still releases
# nothing because release.sh's source-change gate sees no publishable crate
# source in the diff.
#
# There is intentionally NO automatic semver-major: v1.0.0 is a milestone the
# maintainer cuts by hand (FORCE_VERSION=1.0.0 via the CI dispatch input) —
# "когда 100 звёзд дадут". Until then everything stays 0.x.
#
# Pure: reads git history + the root Cargo.toml; writes nothing.
# Usage: scripts/ci/bump_version.sh
set -euo pipefail
cd "$(dirname "$0")/../.."

current="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"

last_tag="$(git tag --list 'v*' --sort=-version:refname | head -n1)"
if [[ -n "$last_tag" ]]; then
  range="$last_tag..HEAD"
else
  range="HEAD" # no release tag yet: consider the whole history
fi

# Subjects decide the bump.
subjects="$(git log "$range" --no-merges --format='%s')"

bump=""
if grep -vE '^(fix|perf|revert)(\([^)]*\))?:' <<<"$subjects" | grep -q '[^[:space:]]'; then
  bump="minor"
elif grep -q '[^[:space:]]' <<<"$subjects"; then
  bump="patch"
fi

[[ -z "$bump" ]] && exit 0

IFS=. read -r major minor patch <<<"$current"
case "$bump" in
minor)
  minor=$((minor + 1))
  patch=0
  ;;
patch) patch=$((patch + 1)) ;;
esac
echo "$major.$minor.$patch"
