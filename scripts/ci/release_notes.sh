#!/usr/bin/env bash
#
# The GitHub Release notes of a vX.Y.Z tag: the commits since the previous
# release tag, the version-bump chore excluded, and the compare link. Written
# to stdout (or to the file given second). Run by release.yml's `cut` job,
# which creates the GitHub Release for the tag the maintainer publishes —
# ci.yml only bumps and tags on merge, so the notes are assembled from the
# tags when the release is actually made, not stored at tag time.
#
# Usage: scripts/ci/release_notes.sh vX.Y.Z [notes-file]
#   GITHUB_REPOSITORY   owner/repo for the compare link (default
#                       docling-project/docling.rs)
# Needs the tags fetched (`fetch-depth: 0`).
set -euo pipefail

tag="${1:?usage: release_notes.sh vX.Y.Z [notes-file]}"
out="${2:-/dev/stdout}"
repo="${GITHUB_REPOSITORY:-docling-project/docling.rs}"

git rev-parse -q --verify "refs/tags/$tag" >/dev/null || {
  echo "release_notes.sh: no tag $tag" >&2
  exit 1
}
# The previous release tag: the newest `v*` tag older than this one by
# version, so a backport or a forced re-release lists its own range.
prev="$(git tag --list 'v*' --sort=-version:refname |
  grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' |
  awk -v t="$tag" 'found { print; exit } $0 == t { found = 1 }' || true)"
range="${prev:+$prev..}$tag"
{
  echo "## What's changed"
  echo
  # The bump commit (`chore(release): vX.Y.Z [skip ci]`) is the tag's own
  # commit, not a change.
  git log "$range" --no-merges --format='- %s (%h)' | grep -v '^- chore(release): ' || true
  if [[ -n "$prev" ]]; then
    echo
    echo "**Full changelog**: https://github.com/$repo/compare/$prev...$tag"
  fi
} >"$out"
