#!/usr/bin/env bash
#
# Publish the workspace crates to crates.io, in dependency order, skipping any
# whose current version is already published ("only changed ones"). Idempotent:
# re-running after a no-op merge publishes nothing.
#
# Auth: in CI the job runs with `id-token: write` and each upload uses a fresh
# Trusted Publishing token from scripts/ci/crates_io_token.sh (OIDC — no
# long-lived secret to rotate; every published crate's crates.io settings name
# docling-project/docling.rs + ci.yml as its trusted publisher). Minted per
# crate because a token lives 30 minutes and a cold `cargo publish` verify
# build of the heavier crates can eat into that. An explicit
# CARGO_REGISTRY_TOKEN in the environment (a manual re-publish from a laptop)
# is used as-is instead. The existence checks need no auth.
# Used by .github/workflows/ci.yml on master.
#
# Usage: scripts/ci/ci_publish.sh
set -euo pipefail

cd "$(dirname "$0")/../.."

# Dependency order: a crate must be published before anything that depends on it.
# docling-serve and docling-rag depend on docling; docling-serve must precede
# docling-cli (whose optional `serve` feature depends on it).
CRATES=(docling-core docling-onnx docling-pdf docling-asr docling docling-serve docling-rag docling-cli)

UA="docling.rs-ci (https://github.com/docling-project/docling.rs)"

# Version of a workspace crate, read from `cargo metadata` (handles workspace
# inheritance, so it stays correct if a crate ever pins its own version).
crate_version() {
  cargo metadata --format-version 1 --no-deps |
    python3 -c "import sys, json; n = sys.argv[1]; print(next(p['version'] for p in json.load(sys.stdin)['packages'] if p['name'] == n))" "$1"
}

# 0 (true) if name@version already exists on crates.io.
already_published() {
  local name="$1" version="$2" code
  code="$(curl -fsS -o /dev/null -w '%{http_code}' \
    -H "User-Agent: $UA" \
    "https://crates.io/api/v1/crates/$name/$version" 2>/dev/null || true)"
  [[ "$code" == "200" ]]
}

published_any=0
for crate in "${CRATES[@]}"; do
  version="$(crate_version "$crate")"
  if already_published "$crate" "$version"; then
    echo "✓ $crate@$version already on crates.io — skipping"
    continue
  fi
  echo ">> publishing $crate@$version ..."
  if [[ -n "${CARGO_REGISTRY_TOKEN:-}" ]]; then
    cargo publish -p "$crate"
  else
    token="$(scripts/ci/crates_io_token.sh)"
    CARGO_REGISTRY_TOKEN="$token" cargo publish -p "$crate"
    scripts/ci/crates_io_token.sh revoke "$token"
  fi
  published_any=1
done

if [[ "$published_any" -eq 0 ]]; then
  echo "Nothing to publish — every crate version is already on crates.io."
fi
