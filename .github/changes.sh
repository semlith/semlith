#!/usr/bin/env bash
# Whether a push touched anything but prose, for ci.yml and native-smoke.yml.
#
# Prose is a Markdown file at the top level or directly under .github/,
# anything under docs/ or assets/, and the licence files -- except a file
# src/ pulls into the binary with include_str! or include_bytes! (the agent
# rules, skill and clients guide `semlith setup` writes), which is code
# wherever it lives. Everything else -- src/, tests/ (whose Markdown includes
# fixtures), workflows, Cargo files -- is code. A rename counts as both of its
# paths, so moving code under docs/ is a code change (SEC-005, SEC-006,
# 2026-10-23 audit).
#
# What it is compared with: on a new push to a pull request, the previous
# head, but only if this workflow's run on that head, on this branch, finished
# green with its heavy lanes run -- a run that was cancelled, failed, was a
# draft's fast lane or belongs to another branch proved nothing, so the whole
# pull request is compared with its base instead. On a push to a branch, the
# commit before it, on the same terms; failing them, the push is code. With no
# usable base it is code.
#
# Writes `code` and `heavy` to $GITHUB_OUTPUT; `heavy` is `code` except on a
# draft pull request, which gets the fast lane only.
set -euo pipefail

# Every path src/ includes, from the repository root.
embedded() {
  git grep -oE 'include_(str|bytes)!\("[^"]+"\)' -- src | while IFS= read -r hit; do
    file=${hit%%:*}
    rel=${hit#*\(\"}
    rel=${rel%\"\)}
    out=()
    IFS=/ read -ra parts <<< "$(dirname "$file")/$rel"
    for part in "${parts[@]}"; do
      case $part in
        '' | .) ;;
        ..) out=("${out[@]:0:${#out[@]}-1}") ;;
        *) out+=("$part") ;;
      esac
    done
    (IFS=/; echo "${out[*]}")
  done
}
shipped=$(embedded)

prose() {
  printf '%s\n' "$1" | grep -Eq '^(docs|assets)/|^(LICENSE|NOTICE|THIRD-PARTY-NOTICES)$|^[^/]+\.md$|^\.github/[^/]+\.md$' \
    && ! printf '%s\n' "$shipped" | grep -Fxq -- "$1"
}

usable() { [ -n "${1:-}" ] && ! echo "$1" | grep -q '^0*$' && git cat-file -e "$1^{commit}" 2>/dev/null; }

event=${GITHUB_EVENT_NAME:-pull_request}
branch=${GITHUB_HEAD_REF:-${GITHUB_REF_NAME:-}}

# Whether this workflow's run of the same event on $1, on this branch, finished
# green and printed heavy=true from this job.
heavy_green() {
  local run job
  [ -n "$branch" ] || return 1
  run=$(gh run list -R "$GITHUB_REPOSITORY" --workflow "$WORKFLOW" --branch "$branch" --commit "$1" \
    --json databaseId,conclusion,event \
    --jq "[.[] | select(.event == \"$event\" and .conclusion == \"success\")][0].databaseId // \"\"" 2>/dev/null) || return 1
  [ -n "$run" ] || return 1
  job=$(gh run view "$run" -R "$GITHUB_REPOSITORY" --json jobs \
    --jq "[.jobs[] | select(.name == \"$GITHUB_JOB\")][0].databaseId // \"\"" 2>/dev/null) || return 1
  [ -n "$job" ] || return 1
  gh run view "$run" -R "$GITHUB_REPOSITORY" --job "$job" --log 2>/dev/null | grep -q 'code=true heavy=true'
}

from="${BASE:-}"
to=HEAD
if [ "${ACTION:-}" = synchronize ] && usable "${BEFORE:-}"; then
  if heavy_green "$BEFORE"; then
    echo "this workflow on the previous head $BEFORE ran its heavy lanes green"
    from=$BEFORE
    to=$HEAD_SHA
  else
    echo "this workflow on the previous head $BEFORE did not run its heavy lanes green; comparing with the base"
  fi
elif [ "$event" = push ] && usable "$from"; then
  if ! heavy_green "$from"; then
    echo "this workflow on the commit before, $from, did not run its heavy lanes green; the push is code"
    from=
  fi
fi

code=true
if usable "$from"; then
  code=false
  echo "compared: $from..$to"
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    echo "  $file"
    prose "$file" || code=true
  done < <(git diff --name-only --no-renames "$from" "$to")
fi

heavy=$code
[ "${DRAFT:-false}" = true ] && heavy=false
echo "code=$code heavy=$heavy"
{
  echo "code=$code"
  echo "heavy=$heavy"
} >> "${GITHUB_OUTPUT:-/dev/stdout}"
