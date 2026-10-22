#!/usr/bin/env bash
# Regenerate the SHA256SUMS asset of a models release from the assets the
# release hosts right now, and upload it (clobbering the previous one).
#
#   scripts/ci/models_manifest.sh models-v1          # GITHUB_REPOSITORY or docling-project/docling.rs
#
# Every workflow that uploads into a `models-v*` release ends with this
# (publish-models.yml, docling-parse-render.yml, onnxruntime-s390x.yml), so
# the manifest always covers the whole release, whichever workflow touched it
# last; models-manifest.yml runs it alone for a release that predates the
# manifest or after a manual `gh release upload`. The downloaders —
# scripts/install/download_dependencies.sh / .bat and the Python
# `download_models()` — verify each asset they fetch from the release against
# it (a corrupted or truncated transfer, a tampered mirror behind
# DOCLING_RS_MODELS_URL, a swapped asset — `libpdfium.so` and
# `libonnxruntime.so` are executable code loaded into the process).
#
# Hashes come from GitHub's own `digest` of each asset (computed on upload,
# `sha256:<hex>` in the REST API); an asset without one (uploaded before
# GitHub recorded digests) is downloaded and hashed here. Format: the
# `sha256sum` one (`<hex>  <name>`), sorted by name, so `sha256sum -c` reads
# it as is.
set -euo pipefail

TAG="${1:?usage: models_manifest.sh <release-tag>}"
REPO="${GITHUB_REPOSITORY:-docling-project/docling.rs}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

gh api "repos/$REPO/releases/tags/$TAG" \
  --jq '.assets[] | [.name, (.digest // "")] | @tsv' > "$tmp/assets.tsv"

: > "$tmp/unsorted"
while IFS=$'\t' read -r name digest; do
  [ -n "$name" ] || continue
  [ "$name" = "SHA256SUMS" ] && continue
  case "$digest" in
    sha256:*) hash="${digest#sha256:}" ;;
    *)
      echo "no digest recorded for $name — downloading to hash it" >&2
      mkdir -p "$tmp/dl"
      gh release download "$TAG" --repo "$REPO" --pattern "$name" --dir "$tmp/dl" --clobber
      hash="$(sha256sum "$tmp/dl/$name" | cut -d' ' -f1)"
      rm -f "$tmp/dl/$name"
      ;;
  esac
  printf '%s  %s\n' "$hash" "$name" >> "$tmp/unsorted"
done < "$tmp/assets.tsv"

LC_ALL=C sort -k2 "$tmp/unsorted" > "$tmp/SHA256SUMS"
if [ ! -s "$tmp/SHA256SUMS" ]; then
  echo "release $TAG of $REPO has no assets — nothing to write" >&2
  exit 1
fi
echo "SHA256SUMS for $TAG ($(wc -l < "$tmp/SHA256SUMS" | tr -d ' ') assets):"
cat "$tmp/SHA256SUMS"
gh release upload "$TAG" "$tmp/SHA256SUMS" --repo "$REPO" --clobber
