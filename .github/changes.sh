#!/usr/bin/env bash
# Whether a push touched anything but prose, for ci.yml and native-smoke.yml.
#
# Prose is a Markdown file at the top level or directly under .github/,
# anything under docs/ or assets/, and the licence files. Everything else --
# src/, tests/ (whose Markdown includes fixtures), workflows, Cargo files --
# is code.
#
# What it is compared with: on a new push to a pull request, the previous
# head, but only if this workflow's run on that head finished green -- a run
# that was cancelled by this push, or failed, proved nothing, so the whole
# pull request is compared with its base instead. On a push to a branch, the
# commit before it. With no usable base it is code.
#
# Writes `code` and `heavy` to $GITHUB_OUTPUT; `heavy` is `code` except on a
# draft pull request, which gets the fast lane only.
set -euo pipefail

prose() {
  grep -Eq '^(docs|assets)/|^(LICENSE|NOTICE|THIRD-PARTY-NOTICES)$|^[^/]+\.md$|^\.github/[^/]+\.md$'
}

usable() { [ -n "${1:-}" ] && ! echo "$1" | grep -q '^0*$' && git cat-file -e "$1^{commit}" 2>/dev/null; }

from="${BASE:-}"
to=HEAD
if [ "${ACTION:-}" = synchronize ] && usable "${BEFORE:-}"; then
  previous=$(gh run list -R "$GITHUB_REPOSITORY" --workflow "$WORKFLOW" --commit "$BEFORE" \
    --json conclusion,event --jq '[.[] | select(.event == "pull_request")][0].conclusion // ""' 2>/dev/null || true)
  echo "this workflow on the previous head $BEFORE: ${previous:-no run}"
  if [ "$previous" = success ]; then
    from=$BEFORE
    to=$HEAD_SHA
  fi
fi

code=true
if usable "$from"; then
  code=false
  echo "compared: $from..$to"
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    echo "  $file"
    printf '%s\n' "$file" | prose || code=true
  done < <(git diff --name-only "$from" "$to")
fi

heavy=$code
[ "${DRAFT:-false}" = true ] && heavy=false
echo "code=$code heavy=$heavy"
{
  echo "code=$code"
  echo "heavy=$heavy"
} >> "${GITHUB_OUTPUT:-/dev/stdout}"
