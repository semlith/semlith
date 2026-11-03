#!/usr/bin/env bash
# Self-test for labels.sh, run by the scripts lane in ci.yml, over pull
# requests written out below: a fork's fix for a bug, a docs-only change, a
# workflow-only change, the owner's and Dependabot's pull requests (left
# alone), and the `waiting on author` swap with the case that keeps it. Each
# case also runs a second time with its labels applied, which must print nothing.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/labels.sh"

# A pull request: the defaults below, with the case's own fields merged over them.
pr() {
  jq -cn '{
    number: 1, author: {login: "someone", type: "User"}, author_association: "CONTRIBUTOR",
    body: "", files: [{path: "src/x.rs"}], labels: [], last_commit_at: "2026-11-01T10:00:00Z",
    reviews: [], closed_issue_labels: []
  } + '"$1"
}
decide() { bash "$script" decide <<< "$1" | sort | tr '\n' ',' | sed 's/,$//'; }
fail() { echo "FAIL: $*"; exit 1; }
# Expects these actions for the pull request, then nothing once they are applied.
check() {
  local name=$1 json=$2 want=$3 got applied
  got=$(decide "$json")
  [ "$got" = "$want" ] || fail "$name: want [$want], got [$got]"
  applied=$(jq -c --arg acts "$got" '
    ($acts | split(",") | map(select(. != "") | capture("^(?<v>add|remove) (?<l>.*)$"))) as $a
    | .labels = ([.labels[].name] + [$a[] | select(.v == "add") | .l]
                 - [$a[] | select(.v == "remove") | .l] | map({name: .}))' <<< "$json")
  got=$(decide "$applied")
  [ -z "$got" ] || fail "$name: a second run is not a no-op, got [$got]"
}

check "a fork's first pull request closing a bug" \
  "$(pr '{author_association: "FIRST_TIME_CONTRIBUTOR", body: "Fixes #12",
          files: [{path: "src/a.rs"}, {path: "Cargo.toml"}, {path: "README.md"}],
          closed_issue_labels: ["bug", "good first issue"]}')" \
  "add bug,add first-time contributor,add rust"

check "a docs-only pull request" \
  "$(pr '{files: [{path: "docs/guide.md"}, {path: "docs/img/a.png"}, {path: "CHANGELOG.md"}]}')" \
  "add documentation"

check "a workflow-only pull request" \
  "$(pr '{files: [{path: ".github/workflows/ci.yml"}]}')" \
  "add github_actions"

check "a Python script" \
  "$(pr '{author_association: "FIRST_TIMER", files: [{path: "scripts/x.py"}]}')" \
  "add first-time contributor,add python"

check "the owner's pull request" \
  "$(pr '{author_association: "OWNER", body: "Closes #3", closed_issue_labels: ["bug"]}')" \
  ""

# The workflow token reads a private org member as CONTRIBUTOR (#334 was
# labelled so on 2026-11-04); a CODEOWNERS handle is core team whatever it says.
check "the owner's pull request, seen as a contributor" \
  "$(pr '{author: {login: "aakashpawar1999", type: "User"}, author_association: "CONTRIBUTOR",
          files: [{path: ".github/workflows/ci.yml"}, {path: "src/a.py"}]}')" \
  ""

check "a Dependabot pull request" \
  "$(pr '{author: {login: "dependabot[bot]", type: "Bot"}, author_association: "NONE",
          files: [{path: "Cargo.lock"}]}')" \
  ""

check "a bot by name alone" \
  "$(pr '{author: {login: "renovate[bot]", type: "User"}}')" \
  ""

check "a bot by type alone" \
  "$(pr '{author: {login: "some-app", type: "Bot"}}')" \
  ""

check "a push after the maintainer's review" \
  "$(pr '{labels: [{name: "rust"}, {name: "waiting on author"}], last_commit_at: "2026-11-02T09:00:00Z",
          reviews: [{author_association: "OWNER", submitted_at: "2026-11-01T12:00:00Z"},
                    {author_association: "NONE", submitted_at: "2026-11-02T10:00:00Z"}]}')" \
  "add waiting on review,remove waiting on author"

check "a review newer than the last commit" \
  "$(pr '{labels: [{name: "rust"}, {name: "waiting on author"}], last_commit_at: "2026-11-01T10:00:00Z",
          reviews: [{author_association: "MEMBER", submitted_at: "2026-11-01T12:00:00Z"}]}')" \
  ""

check "the owner's review, seen as a contributor's, newer than the last commit" \
  "$(pr '{labels: [{name: "rust"}, {name: "waiting on author"}], last_commit_at: "2026-11-01T10:00:00Z",
          reviews: [{login: "aakashpawar1999", author_association: "CONTRIBUTOR", submitted_at: "2026-11-01T12:00:00Z"}]}')" \
  ""

check "a hand-set label stays" \
  "$(pr '{labels: [{name: "rust"}, {name: "enhancement"}]}')" \
  ""

echo "labels.sh: 13 checks passed"
