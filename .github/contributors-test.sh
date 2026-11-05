#!/usr/bin/env bash
# Self-test for contributors.sh's render step, run by the scripts lane in ci.yml,
# over releases written out below: an outside contributor's first pull request,
# a returning one, an outside issue closed by the owner's pull request, the
# owner's and Dependabot's pull requests, and a release with nobody to credit.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/contributors.sh"
fail() { echo "FAIL: $*"; exit 1; }
render() { bash "$script" render <<< "$1"; }
pr() { jq -cn "{bot: false, association: \"CONTRIBUTOR\", closes: [], first: false} + $1"; }
issue() { jq -cn "{bot: false, association: \"NONE\"} + $1"; }

owner=$(pr '{number: 10, title: "Release 0.9.0", login: "owner", association: "OWNER"}')
first=$(pr '{number: 11, title: "fix(report): bytes", login: "ada", first: true, closes: [5]}')
again=$(pr '{number: 12, title: "docs: typo", login: "lin"}')
bot=$(pr '{number: 13, title: "build(deps): bump zip", login: "dependabot[bot]", bot: true}')
bot2=$(pr '{number: 14, title: "build(deps): bump ureq", login: "dependabot[bot]", bot: true}')
reported=$(issue '{number: 5, title: "bytes differ", login: "ada"}')
by_owner=$(issue '{number: 6, title: "lane locked", login: "owner", association: "OWNER"}')
outside_issue=$(issue '{number: 7, title: "scan misses a password", login: "kim"}')

out=$(render "$(jq -cn --argjson a "[$owner,$first,$again,$bot,$bot2]" --argjson i "[$reported,$by_owner,$outside_issue]" '{prs: $a, issues: $i}')")
want='### Contributors

Thank you to everyone outside the core team whose work is in this release.

- @ada: fix(report): bytes (#11); reported bytes differ (#5)
- @kim: reported scan misses a password (#7)
- @lin: docs: typo (#12)

New contributors:

- @ada made their first contribution in #11

@dependabot kept dependencies current in #13, #14.'
[ "$out" = "$want" ] || fail "a release with outside work:
--- want
$want
--- got
$out"

# The owner's own release credits nobody and prints nothing.
out=$(render "$(jq -cn --argjson a "[$owner]" --argjson i "[$by_owner]" '{prs: $a, issues: $i}')")
[ -z "$out" ] || fail "a release with only the owner's work printed: $out"

# A CODEOWNERS handle is core team even when the workflow token reads the
# private org member as CONTRIBUTOR.
me=$(pr '{number: 16, title: "Release 0.9.0", login: "aakashpawar1999", association: "CONTRIBUTOR", first: true, closes: [8]}')
mine=$(issue '{number: 8, title: "x", login: "aakashpawar1999", association: "CONTRIBUTOR"}')
out=$(render "$(jq -cn --argjson a "[$me]" --argjson i "[$mine]" '{prs: $a, issues: $i}')")
[ -z "$out" ] || fail "the owner, read as a contributor, was credited: $out"

# A collaborator is core team, not an outside contributor.
collab=$(pr '{number: 15, title: "feat: x", login: "sam", association: "COLLABORATOR", first: true}')
out=$(render "$(jq -cn --argjson a "[$collab]" '{prs: $a, issues: []}')")
[ -z "$out" ] || fail "a collaborator was credited as an outside contributor: $out"

# Only Dependabot: its line alone, no thank-you list and no new contributors.
out=$(render "$(jq -cn --argjson a "[$bot]" '{prs: $a, issues: []}')")
want='### Contributors

@dependabot kept dependencies current in #13.'
[ "$out" = "$want" ] || fail "a Dependabot-only release: got [$out]"

echo "contributors.sh: 5 checks passed"
