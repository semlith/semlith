#!/usr/bin/env bash
# Who contributed to a release, from the pull requests merged between two refs.
#
#   contributors.sh section <from> [<to>]   print the CHANGELOG "### Contributors"
#                                           section for <from>..<to> (default HEAD)
#   contributors.sh thank <tag>             comment "released in" on each outside
#                                           contributor's pull request and on each
#                                           issue of theirs it closed (release.yml)
#   contributors.sh render                  read collected JSON on stdin, print the
#                                           section (contributors-test.sh feeds it)
#
# A pull request counts when it reached the range as a merge commit ("Merge pull
# request #N", the only way this repository merges one). Its author is an outside
# contributor unless GitHub calls them OWNER, MEMBER or COLLABORATOR; bots are
# counted on one line. An issue counts when an outside contributor opened it and a
# pull request in the range closes it, whoever wrote that pull request. Reads go
# through `gh` and need no write access; only `thank` writes, and it skips any
# pull request or issue that already carries this release's marker.
set -euo pipefail

repo=${GITHUB_REPOSITORY:-semlith/semlith}

# The collected JSON: {prs: [...], issues: [...]}, one entry per pull request in
# the range and per issue those pull requests close.
# The pull requests merged in <from>..<to>: "Merge pull request #N" commits, and
# a branch folded in by hand ("Merge remote-tracking branch 'origin/<b>'", as a
# release folds in Dependabot's) resolved to the pull request whose head it was.
merged_prs() {
  local subjects branch
  subjects=$(git log --merges --format=%s "$1..$2")
  sed -n 's/^Merge pull request #\([0-9][0-9]*\) .*/\1/p' <<< "$subjects"
  sed -n "s/^Merge \(remote-tracking \)\{0,1\}branch '\(origin\/\)\{0,1\}\([^']*\)'.*/\3/p" <<< "$subjects" \
    | while IFS= read -r branch; do
        [ "$branch" = develop ] || [ "$branch" = main ] && continue
        gh pr list --repo "$repo" --state all --head "$branch" --json number --jq '.[0].number // empty'
      done
}

collect() {
  local from=$1 to=$2 n prs='[]' issues='[]' pr closes login first
  for n in $(merged_prs "$from" "$to" | sort -n -u); do
    pr=$(gh api "repos/$repo/pulls/$n" --jq '{number, title, login: .user.login, bot: (.user.type == "Bot"), association: .author_association, url: .html_url}')
    closes=$(gh pr view "$n" --repo "$repo" --json closingIssuesReferences --jq '[.closingIssuesReferences[].number]')
    login=$(jq -r .login <<< "$pr")
    # First contribution: their oldest merged pull request here is this one.
    # Asked only for outside authors: search is rate limited hard.
    first=0
    if jq -e '(.bot | not) and (.association as $a | ["OWNER", "MEMBER", "COLLABORATOR"] | index($a) | not)' <<< "$pr" > /dev/null; then
      first=$(gh api -X GET search/issues -f q="repo:$repo is:pr is:merged author:$login" -f sort=created -f order=asc -f per_page=1 \
        --jq '.items[0].number // 0')
    fi
    prs=$(jq -c --argjson pr "$pr" --argjson closes "$closes" --argjson first "$first" \
      '. + [$pr + {closes: $closes, first: ($first == $pr.number)}]' <<< "$prs")
    for i in $(jq -r '.[]' <<< "$closes"); do
      jq -e --argjson i "$i" 'any(.[]; .number == $i)' <<< "$issues" > /dev/null && continue
      issues=$(jq -c --argjson issue "$(gh api "repos/$repo/issues/$i" \
        --jq '{number, title, login: .user.login, bot: (.user.type == "Bot"), association: .author_association, url: .html_url}')" \
        '. + [$issue]' <<< "$issues")
    done
  done
  jq -cn --argjson prs "$prs" --argjson issues "$issues" '{prs: $prs, issues: $issues}'
}

render() {
  jq -r '
    def outside: (.bot | not) and (.association as $a | ["OWNER", "MEMBER", "COLLABORATOR"] | index($a) | not);
    ([.prs[] | select(outside)]) as $prs
    | ([.issues[] | select(outside)]) as $issues
    | ([.prs[] | select(.bot)]) as $bots
    | ([$prs[].login] + [$issues[].login] | unique) as $people
    | if ($people | length) == 0 and ($bots | length) == 0 then empty else
        "### Contributors\n",
        (if ($people | length) > 0 then
          "Thank you to everyone outside the core team whose work is in this release.\n",
          ($people[] as $who
           | ([$prs[] | select(.login == $who) | "\(.title) (#\(.number))"]
              + [$issues[] | select(.login == $who) | "reported \(.title) (#\(.number))"]) as $did
           | "- @\($who): \($did | join("; "))"),
          ""
        else empty end),
        ([$prs[] | select(.first)] as $new
         | if ($new | length) > 0 then
             "New contributors:\n",
             ($new[] | "- @\(.login) made their first contribution in #\(.number)"),
             ""
           else empty end),
        (if ($bots | length) > 0 then
           ($bots | group_by(.login)[] | "@\(.[0].login | sub("\\[bot\\]$"; "")) kept dependencies current in \([.[].number | "#\(.)"] | join(", ")).\n")
         else empty end)
      end
  '
}

# The tag before <tag>: the newest release tag it descends from, never a
# release candidate.
previous_tag() {
  git describe --tags --abbrev=0 --match 'v[0-9]*' --exclude '*-*' "$1^"
}

thank() {
  local tag=$1 from data marker release
  from=$(previous_tag "$tag")
  data=$(collect "$from" "$tag")
  marker="<!-- semlith-released:$tag -->"
  release="https://github.com/$repo/releases/tag/$tag"
  # One comment per pull request and per issue, never twice for one release.
  jq -c '
    def outside: (.bot | not) and (.association as $a | ["OWNER", "MEMBER", "COLLABORATOR"] | index($a) | not);
    (.prs[] | select(outside) | {number, login, kind: "pr"}),
    (.issues[] | select(outside) | {number, login, kind: "issue"})
  ' <<< "$data" | while IFS= read -r item; do
    local n login kind body
    n=$(jq -r .number <<< "$item")
    login=$(jq -r .login <<< "$item")
    kind=$(jq -r .kind <<< "$item")
    if gh api --paginate "repos/$repo/issues/$n/comments" --jq '.[].body' | grep -qF "$marker"; then
      echo "#$n: already thanked for $tag"
      continue
    fi
    if [ "$kind" = pr ]; then
      body="Released in [Semlith ${tag#v}]($release), and credited in its release notes. Thank you for the contribution, @$login!"
    else
      body="The fix for this is released in [Semlith ${tag#v}]($release). Thank you for reporting it, @$login!"
    fi
    gh api -X POST "repos/$repo/issues/$n/comments" -f body="$body

$marker" > /dev/null
    echo "#$n: thanked @$login for $tag"
  done
}

case ${1:-} in
  section) collect "$2" "${3:-HEAD}" | render ;;
  thank) thank "$2" ;;
  render) render ;;
  *) echo "usage: contributors.sh section <from> [<to>] | thank <tag> | render" >&2; exit 2 ;;
esac
