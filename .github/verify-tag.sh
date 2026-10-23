#!/usr/bin/env bash
# Whether the commit a release tag points at is on main, for release.yml.
#
#   verify-tag.sh <tag> <commit> <main ref>
#
# A release is cut only from a commit merged to main, and main takes only
# reviewed pull requests from develop. A tag anywhere else is refused before
# anything is built or signed (SEC-001, 2026-10-23 audit). The refusal names
# the tag, the commit and the ref it was checked against.
set -euo pipefail

tag=${1:?usage: verify-tag.sh <tag> <commit> <main ref>}
commit=${2:?usage: verify-tag.sh <tag> <commit> <main ref>}
main=${3:?usage: verify-tag.sh <tag> <commit> <main ref>}

if ! git rev-parse --verify --quiet "$main^{commit}" > /dev/null; then
  echo "::error::$main is not a commit here; cannot check that $tag ($commit) is on it"
  exit 1
fi
if git merge-base --is-ancestor "$commit" "$main"; then
  echo "$tag ($commit) is on $main"
else
  echo "::error::$tag points at $commit, which $main does not contain; a release is cut only from a commit merged to main"
  exit 1
fi
