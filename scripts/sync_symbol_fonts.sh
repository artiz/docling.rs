#!/usr/bin/env bash
# Refresh crates/docling/src/backend/symbol_fonts.json from docling-core
# (#742): the symbol-font table is docling-core's
# `docling_core/utils/symbol_fonts.json`, shared with Python docling and
# docling-parse, and kept byte-identical here. `--check` exits 1 when the
# vendored copy differs from upstream instead of rewriting it.
#
# Usage: scripts/sync_symbol_fonts.sh [--check] [REF]   (REF defaults to main)
set -euo pipefail
cd "$(dirname "$0")/.."
check=false
if [[ "${1:-}" == "--check" ]]; then check=true; shift; fi
ref="${1:-main}"
url="https://raw.githubusercontent.com/docling-project/docling-core/${ref}/docling_core/utils/symbol_fonts.json"
dest="crates/docling/src/backend/symbol_fonts.json"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp"
if $check; then
  if cmp -s "$tmp" "$dest"; then echo "symbol_fonts.json is up to date with docling-core@${ref}"; exit 0; fi
  echo "symbol_fonts.json differs from docling-core@${ref}: run scripts/sync_symbol_fonts.sh ${ref}" >&2
  exit 1
fi
mv "$tmp" "$dest"
trap - EXIT
echo "updated $dest from docling-core@${ref}"
