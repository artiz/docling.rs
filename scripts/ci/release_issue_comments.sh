#!/usr/bin/env bash
#
# Tell the issues a release shipped that it shipped: for every issue the
# commits of release vX.Y.Z reference, post one "Shipped in vX.Y.Z" comment
# linking the tag's page. Run by the master-only `release` job of
# .github/workflows/ci.yml right after release.sh pushed the tag; GitHub
# renders `/releases/tag/<tag>` for a bare tag, and the GitHub Release
# release.yml cuts when the maintainer publishes the tag lands at the same
# URL, so the link is live when the comment lands and gets richer later.
#
# Where the issue numbers come from — the whole range since the previous
# release tag, not only the merge that triggered this release: a docs- or
# CI-only merge releases nothing (release.sh's source-change gate), so the
# issues it referenced roll into the next release and are announced then.
#   1. `Closes/Fixes/Resolves/Refs #NN` (any tense/case, optional colon,
#      optionally `owner/repo#NN` for THIS repo) in the commit messages, the
#      convention CLAUDE.md asks for.
#   2. The pull requests merged in the range — `Merge pull request #NN from …`
#      (merge commits) and a `(#NN)` subject suffix (squash merges) — resolved
#      through the GitHub API to the issues they link: GitHub's own
#      closing-issue links (the `Closes #NN` in the PR description that
#      auto-closed the issue on merge) plus the same keyword scan over the PR
#      body, which also catches a `Refs #NN` GitHub does not link.
# Numbers that turn out to be pull requests, or do not exist, are dropped.
#
# Idempotent: the comment carries an HTML marker naming the tag, and an issue
# that already has one for this tag is skipped — a forced re-release of the
# same version (ci.yml's `force_version`) does not comment twice. Comments
# are posted as the token's identity (the job passes `github.token`, so as
# github-actions[bot]; the job needs `issues: write`).
#
# Degrades, never blocks: an API failure on one issue is a `::warning::` and
# the loop goes on; only a usage error exits non-zero. The release is already
# tagged and cut by the time this runs, so nothing here may turn it red.
#
# Usage: scripts/ci/release_issue_comments.sh vX.Y.Z
#   GH_TOKEN             gh auth (required unless DRY_RUN)
#   GITHUB_REPOSITORY    owner/repo the issues live in (default
#                        docling-project/docling.rs; ci.yml only runs this step
#                        in that repository, so a fork's CI never comments on
#                        upstream issues)
#   DRY_RUN=1            resolve and print the would-be comments, post nothing
#   PREV_TAG             previous release tag (default: the newest v* tag below
#                        vX.Y.Z — for a run outside the release job)
set -euo pipefail
cd "$(dirname "$0")/../.."

tag="${1:-}"
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 vX.Y.Z" >&2
  exit 2
fi
repo="${GITHUB_REPOSITORY:-docling-project/docling.rs}"
dry="${DRY_RUN:-}"
release_url="https://github.com/$repo/releases/tag/$tag"
marker="<!-- docling-rs-release: $tag -->"

warn() {
  if [[ -n "${GITHUB_ACTIONS:-}" ]]; then echo "::warning::$*"; else echo "warning: $*" >&2; fi
}

# The range: everything after the previous release tag up to and including
# this one (the release chore commit at the tip references nothing).
prev="${PREV_TAG:-}"
if [[ -z "$prev" ]]; then
  prev="$(git tag --list 'v*' --sort=-version:refname | awk -v t="$tag" 'found { print; exit } $0 == t { found = 1 }')"
fi
if ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  echo "tag $tag does not exist locally" >&2
  exit 2
fi
range="${prev:+$prev..}$tag"
echo ">> issues referenced in $range ($repo)"

# `Closes #12`, `fixes: #12`, `Refs docling-project/docling.rs#12`, … → 12.
# The character class before the keyword keeps `other/repo#12` and
# `prefixes#12` out; a bare `#12` without a keyword is deliberately NOT a
# reference (issue numbers in prose — "the #12 fix" — would mis-announce).
ref_re="(^|[^[:alnum:]/._-])(close[sd]?|fix(e[sd])?|resolve[sd]?|refs?)[[:space:]]*:?[[:space:]]*($repo)?#[0-9]+"
issue_refs() { # stdin: text → issue numbers, one per line
  grep -oiE "$ref_re" | grep -oE '[0-9]+$' || true
}

declare -A issues=()  # number → where it was referenced (for the log)
add_issue() { [[ -n "${issues[$1]:-}" ]] || issues[$1]="$2"; }

# 1. Commit messages (merge commits included: a merge subject repeats the PR
#    title, which may carry the reference).
while read -r n; do add_issue "$n" "commit message"; done < <(git log "$range" --format='%B' | issue_refs)

# 2. Pull requests merged in the range.
pr_numbers="$(
  {
    git log "$range" --merges --format='%s' | sed -nE 's/^Merge pull request #([0-9]+) from .*/\1/p'
    git log "$range" --no-merges --format='%s' | sed -nE 's/.*\(#([0-9]+)\)$/\1/p'
  } | sort -un
)"
owner="${repo%%/*}"
name="${repo##*/}"
pr_query='query($owner: String!, $name: String!, $n: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $n) {
      body
      closingIssuesReferences(first: 50) { nodes { number repository { nameWithOwner } } }
    }
  }
}'
for pr in $pr_numbers; do
  if [[ -n "$dry" && -z "${GH_TOKEN:-}" ]]; then
    echo "   (dry run, no token: PR #$pr not resolved)"
    continue
  fi
  if ! out="$(gh api graphql -F owner="$owner" -F name="$name" -F n="$pr" -f query="$pr_query" 2>&1)"; then
    warn "PR #$pr: could not read its linked issues: $out"
    continue
  fi
  while read -r n; do add_issue "$n" "linked by PR #$pr"; done < <(
    jq -r --arg repo "$repo" '.data.repository.pullRequest.closingIssuesReferences.nodes[]? | select(.repository.nameWithOwner == $repo) | .number' <<<"$out"
  )
  while read -r n; do add_issue "$n" "PR #$pr description"; done < <(
    jq -r '.data.repository.pullRequest.body // ""' <<<"$out" | issue_refs
  )
done

if [[ ${#issues[@]} -eq 0 ]]; then
  echo ">> no issue references in $range — nothing to announce"
  exit 0
fi

body="Shipped in [$tag]($release_url).

The crates.io, PyPI, npm and GHCR packages and the CLI binaries follow when that release is published; \`install.sh\` and the GitHub *latest* release pick it up then.

$marker"

posted=0
for n in $(printf '%s\n' "${!issues[@]}" | sort -n); do
  why="${issues[$n]}"
  if [[ -n "$dry" && -z "${GH_TOKEN:-}" ]]; then
    echo "   #$n ($why): would comment (not verified: no token)"
    continue
  fi
  # Must be an existing issue, not a pull request (they share one number space).
  if ! meta="$(gh api "repos/$repo/issues/$n" 2>&1)"; then
    echo "   #$n ($why): not found — skipped"
    continue
  fi
  if [[ "$(jq -r 'has("pull_request")' <<<"$meta")" == "true" ]]; then
    echo "   #$n ($why): is a pull request — skipped"
    continue
  fi
  if gh api --paginate "repos/$repo/issues/$n/comments" --jq '.[].body' 2>/dev/null | grep -qF "$marker"; then
    echo "   #$n ($why): already announced"
    continue
  fi
  if [[ -n "$dry" ]]; then
    echo "   #$n ($why): would comment"
    continue
  fi
  if gh api -X POST "repos/$repo/issues/$n/comments" -f body="$body" >/dev/null 2>&1; then
    echo "   #$n ($why): commented"
    posted=$((posted + 1))
  else
    warn "issue #$n: could not post the release comment"
  fi
done
echo ">> $tag: ${#issues[@]} issue(s) referenced, $posted comment(s) posted"
