#!/usr/bin/env bash
# Labels outside contributors' pull requests, run by labels.yml every fifteen
# minutes. It reads each pull request through the API and never checks out or
# runs its code.
#
#   labels.sh          label every open pull request in $GITHUB_REPOSITORY
#   labels.sh decide   read one pull request's JSON on stdin and print the
#                      changes it needs, one `add <label>` or
#                      `remove <label>` per line (labels-test.sh feeds it)
#
# The JSON `decide` reads is the shape `fetch` below builds:
#   {number, author: {login, type}, author_association, body,
#    files: [{path}], labels: [{name}], last_commit_at,
#    reviews: [{author_association, submitted_at}], closed_issue_labels: [..]}
#
# It only adds labels, so one set by hand stays, with one exception: a pull
# request marked `waiting on author` whose newest commit is later than the
# newest maintainer review becomes `waiting on review`. Every label it would
# add is checked against the ones already there, so a second run prints nothing.
set -euo pipefail

decide() {
  jq -r '
    def maintainer: . == "OWNER" or . == "MEMBER" or . == "COLLABORATOR";
    [.labels[].name] as $have
    | [.files[].path] as $paths
    | if (.author_association | maintainer)
        or .author.type == "Bot" or (.author.login | endswith("[bot]"))
      then empty
      else
        ( [ ( [.closed_issue_labels[] | select(. == "bug" or . == "enhancement" or . == "documentation")]
              | if length > 0 then .[]
                elif ($paths | length > 0) and ($paths | all(startswith("docs/") or endswith(".md")))
                then "documentation"
                else empty end ),
            (if any($paths[]; endswith(".rs") or . == "Cargo.toml" or . == "Cargo.lock"
                              or endswith("/Cargo.toml") or endswith("/Cargo.lock"))
             then "rust" else empty end),
            (if any($paths[]; startswith(".github/workflows/")) then "github_actions" else empty end),
            (if any($paths[]; endswith(".py")) then "python" else empty end),
            (if .author_association == "FIRST_TIME_CONTRIBUTOR" or .author_association == "FIRST_TIMER"
             then "first-time contributor" else empty end)
          ]
          + ( ([.reviews[] | select(.submitted_at != null and (.author_association | maintainer)) | .submitted_at]
               | max) as $reviewed
              | if ($have | index("waiting on author")) and $reviewed != null
                   and .last_commit_at != null and .last_commit_at > $reviewed
                then ["waiting on review"] else [] end )
        ) as $want
        | ($want | unique | map(select(. as $l | $have | index($l) | not)) | .[] | "add \(.)"),
          (if $want | index("waiting on review") then "remove waiting on author" else empty end)
      end
  '
}

# One pull request's JSON, read from the API, in the shape `decide` takes.
fetch() {
  local pr=$1 n issues issue_labels='[]' files commit reviews
  n=$(jq -r .number <<< "$pr")
  # Only `#N` in this repository; `owner/repo#N` names another one.
  issues=$(jq -r '.body // "" | [scan("(?i)\\b(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?):?\\s+#([0-9]+)")[0]] | unique | .[]' <<< "$pr")
  for i in $issues; do
    issue_labels=$(jq -c --argjson more "$(gh api "repos/$repo/issues/$i" --jq '[.labels[].name]' 2> /dev/null || echo '[]')" \
      '. + $more' <<< "$issue_labels")
  done
  files=$(gh api --paginate "repos/$repo/pulls/$n/files?per_page=100" --jq '.[] | {path: .filename}' | jq -sc .)
  commit=$(gh api --paginate "repos/$repo/pulls/$n/commits?per_page=100" --jq '.[].commit.committer.date' | jq -Rsc 'split("\n") | map(select(. != "")) | max')
  reviews=$(gh api --paginate "repos/$repo/pulls/$n/reviews?per_page=100" --jq '.[] | {author_association, submitted_at}' | jq -sc .)
  jq -c --argjson files "$files" --argjson commit "$commit" --argjson reviews "$reviews" --argjson issues "$issue_labels" \
    '{number, author: {login: .user.login, type: .user.type}, author_association, body,
      labels: [.labels[] | {name}], files: $files, last_commit_at: $commit,
      reviews: $reviews, closed_issue_labels: $issues}' <<< "$pr"
}

main() {
  local pr n verb label
  repo=${GITHUB_REPOSITORY:?set GITHUB_REPOSITORY to owner/name}
  gh api --paginate "repos/$repo/pulls?state=open&per_page=100" --jq '.[] | @json' | while IFS= read -r pr; do
    n=$(jq -r .number <<< "$pr")
    fetch "$pr" | decide | while read -r verb label; do
      echo "#$n: $verb $label"
      case $verb in
        add) gh api -X POST "repos/$repo/issues/$n/labels" -f "labels[]=$label" > /dev/null ;;
        remove) gh api -X DELETE "repos/$repo/issues/$n/labels/$(jq -rn --arg l "$label" '$l | @uri')" > /dev/null ;;
      esac
    done
  done
}

if [ "${1:-}" = decide ]; then decide; else main; fi
