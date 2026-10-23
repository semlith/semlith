#!/usr/bin/env bash
# Self-test for changes.sh, run by the scripts lane in ci.yml, over a scratch
# repository and a stand-in `gh` that answers from environment variables:
# prose stays prose; a doc src/ includes, a rename out of src/ and a previous
# run that never ran its heavy lanes (a draft, another branch, a cancelled
# push) are all code.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/changes.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# The stand-in: `run list` succeeds only for the branch and commit in
# FAKE_BRANCH and FAKE_COMMIT, `run view --json` names job 7, and the job's
# log is FAKE_LOG.
mkdir "$work/bin"
cat > "$work/bin/gh" <<'EOF'
#!/usr/bin/env bash
args=" $* "
case "$args" in
  *" run list "*)
    case "$args" in *" --branch $FAKE_BRANCH "*"--commit $FAKE_COMMIT "*) echo 41 ;; *) echo "" ;; esac ;;
  *" --log "*) echo "$FAKE_LOG" ;;
  *" run view "*) echo 7 ;;
esac
EOF
chmod +x "$work/bin/gh"
# The runner's own values would leak into every case below.
unset GITHUB_HEAD_REF GITHUB_REF_NAME GITHUB_EVENT_NAME ACTION BASE BEFORE HEAD_SHA DRAFT
export PATH="$work/bin:$PATH" GITHUB_REPOSITORY=o/r WORKFLOW=ci.yml GITHUB_JOB=changes

repo="$work/repo"
mkdir -p "$repo/src" "$repo/docs/skill"
cd "$repo"
git init -q -b main
printf 'const SKILL: &str = include_str!("../docs/skill/SKILL.md");\n' > src/clients.rs
echo 'fn x() {}' > src/x.rs
echo skill > docs/skill/SKILL.md
echo guide > docs/guide.md
git add -A
git -c user.name=t -c user.email=t@t commit -q -m base
base=$(git rev-parse HEAD)

commit() { git add -A; git -c user.name=t -c user.email=t@t commit -q -m "$1"; git rev-parse HEAD; }
# Runs changes.sh and prints the `code` it wrote.
code() {
  out="$work/out"
  : > "$out"
  env GITHUB_OUTPUT="$out" "$@" bash "$script" > "$work/log" 2>&1 || { cat "$work/log"; return 1; }
  sed -n 's/^code=//p' "$out"
}
fail() { echo "FAIL: $*"; cat "$work/log"; exit 1; }
pr() { code GITHUB_EVENT_NAME=pull_request GITHUB_HEAD_REF=feat/x ACTION=opened BASE="$base" HEAD_SHA=HEAD "$@"; }

git checkout -q -b feat/x
echo more >> docs/guide.md
prose_head=$(commit prose)
[ "$(pr)" = false ] || fail "a docs/ edit is code"

echo changed >> docs/skill/SKILL.md
commit skill > /dev/null
[ "$(pr)" = true ] || fail "an edit to a doc src/ includes is prose"

git reset -q --hard "$prose_head"
git mv src/x.rs docs/x.md
commit rename > /dev/null
[ "$(pr)" = true ] || fail "moving src/x.rs to docs/x.md is prose"

# A pull request with code, then a prose push on top.
git reset -q --hard "$base"
echo 'fn y() {}' >> src/x.rs
code_head=$(commit code)
echo again >> docs/guide.md
commit prose-again > /dev/null
sync() { pr ACTION=synchronize BEFORE="$code_head" "$@"; }

[ "$(sync FAKE_BRANCH=feat/x FAKE_COMMIT="$code_head" FAKE_LOG='code=true heavy=true')" = false ] \
  || fail "a prose push after a heavy-green head is code"
[ "$(sync FAKE_BRANCH=feat/x FAKE_COMMIT="$code_head" FAKE_LOG='code=true heavy=false')" = true ] \
  || fail "a draft's green fast lane let the code skip its heavy lanes"
[ "$(sync FAKE_BRANCH=feat/other FAKE_COMMIT="$code_head" FAKE_LOG='code=true heavy=true')" = true ] \
  || fail "another branch's green run let the code skip its heavy lanes"

# A push to a branch: the commit before must have run its heavy lanes green.
push() { code GITHUB_EVENT_NAME=push GITHUB_REF_NAME=develop BASE="$code_head" HEAD_SHA=HEAD "$@"; }
[ "$(push FAKE_BRANCH=develop FAKE_COMMIT="$code_head" FAKE_LOG='code=true heavy=true')" = false ] \
  || fail "a prose push after a heavy-green push is code"
[ "$(push FAKE_BRANCH=develop FAKE_COMMIT=none FAKE_LOG='')" = true ] \
  || fail "a prose push after a cancelled or missing run is prose"

echo "changes.sh: 8 checks passed"
