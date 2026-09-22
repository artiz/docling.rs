#!/usr/bin/env bash
#
# Mint a short-lived crates.io publish token through Trusted Publishing (OIDC),
# printing it on stdout. No marketplace action: the exchange is two HTTP calls
# (the docling-project org allowlists only GitHub-owned / vetted actions, so
# `rust-lang/crates-io-auth-action` is out — same reason npm-publish.yml drives
# the npm CLI itself). Requires the job to run with `id-token: write`, which
# gives it ACTIONS_ID_TOKEN_REQUEST_URL / _TOKEN.
#
#   1. Ask GitHub for an OIDC JWT with audience `crates.io`.
#   2. POST it to crates.io, which checks it against the trusted-publisher
#      config of the crates being published (repository + workflow file) and
#      answers with a `cio_tp_…` token valid for 30 minutes.
#
# Each JWT is single-use, so call this once per token you need. `revoke` deletes
# a token before its expiry (crates.io does that on its own at 30 minutes; we
# just don't leave one lying around in the runner's environment longer than
# the publish it was minted for).
#
# Usage: scripts/ci/crates_io_token.sh            -> prints a token
#        scripts/ci/crates_io_token.sh revoke TOK -> revokes TOK
set -euo pipefail

UA="docling.rs-ci (https://github.com/docling-project/docling.rs)"
API="https://crates.io/api/v1/trusted_publishing/tokens"

if [[ "${1:-}" == "revoke" ]]; then
  curl -fsS -o /dev/null -X DELETE -H "User-Agent: $UA" \
    -H "Authorization: Bearer $2" "$API" ||
    echo "warning: token revoke failed (it expires on its own)" >&2
  exit 0
fi

: "${ACTIONS_ID_TOKEN_REQUEST_URL:?not running with id-token: write}"
: "${ACTIONS_ID_TOKEN_REQUEST_TOKEN:?not running with id-token: write}"

jwt="$(curl -fsS -H "Authorization: bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN" \
  "${ACTIONS_ID_TOKEN_REQUEST_URL}&audience=crates.io" |
  python3 -c 'import sys, json; print(json.load(sys.stdin)["value"])')"

# The JWT must never reach the log; only the crates.io reply is parsed.
curl -fsS -X POST -H "User-Agent: $UA" -H "Content-Type: application/json" \
  --data "$(python3 -c 'import sys, json; print(json.dumps({"jwt": sys.argv[1]}))' "$jwt")" \
  "$API" | python3 -c 'import sys, json; print(json.load(sys.stdin)["token"])'
