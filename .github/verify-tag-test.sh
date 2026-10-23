#!/usr/bin/env bash
# Self-test for verify-tag.sh, run by the scripts lane in ci.yml: a tag on
# main passes, a tag on a commit main does not contain is refused and named,
# and a main ref that does not exist is refused rather than passed.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/verify-tag.sh"
repo=$(mktemp -d)
trap 'rm -rf "$repo"' EXIT
cd "$repo"
git init -q -b main
git -c user.name=t -c user.email=t@t commit -q --allow-empty -m one
on_main=$(git rev-parse HEAD)
git checkout -q -b side
git -c user.name=t -c user.email=t@t commit -q --allow-empty -m two
off_main=$(git rev-parse HEAD)
git checkout -q main

fail() { echo "FAIL: $*"; exit 1; }

bash "$script" v1.0.0 "$on_main" main > out.txt || fail "a tag on main was refused: $(cat out.txt)"

if bash "$script" v1.0.1 "$off_main" main > out.txt; then
  fail "a tag off main passed"
fi
grep -q "v1.0.1 points at $off_main, which main does not contain" out.txt \
  || fail "the refusal does not name the tag, commit and ref: $(cat out.txt)"

if bash "$script" v1.0.2 "$on_main" origin/nowhere > out.txt; then
  fail "a missing main ref passed"
fi

echo "verify-tag.sh: 3 checks passed"
