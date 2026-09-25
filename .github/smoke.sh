#!/usr/bin/env bash
# shellcheck disable=SC2329,SC2016,SC1003
# SC2329: every `c_*` function is called by name through `check`, which the
# linter cannot follow. SC2016: the conditions handed to `rc_until eval`
# are single-quoted so they are re-read on every poll. SC1003: '\\?\' is the
# literal Windows verbatim prefix, not an attempt to escape a quote.
#
# Every user-facing command, against an installed binary, on whatever operating
# system this runs on.
#
# Three rules this file exists to enforce, learned from the first version of it
# that passed on all three platforms while two real bugs sat in its own log:
#
#   1. A check asserts a property, never just an exit status. Every command on
#      Windows exited 0 while printing `\\?\D:\a\...` for every path.
#   2. A check that is known to fail says so, by id, in known-failures.txt. An
#      unexpected failure fails the job; a known one does not; a known one that
#      starts passing fails the job, so a fix cannot land silently.
#   3. A check whose precondition did not hold is skipped, not failed. A
#      harness that blames the product for its own breakage is worse than none.
#
#   4. A harness may not write outside its own working directory. This one did,
#      for four releases, and issue #106 is the record of it: SEMLITH_HOME
#      redirects the store home, the model cache and the agent key, and nothing
#      else — a *client* configuration lives under the user's own home, and
#      `home::user_home` reads HOME to find it. So `cli/setup/idempotent`, run
#      on a developer's machine, rewrote that developer's real Claude Code,
#      Cursor and Codex registrations. HOME is redirected below for that reason,
#      and `cli/harness/leaves-the-real-home-alone` proves the redirect held.
#
# SEMLITH_HOME is set here deliberately. The native resolution of the home
# directory is what portal-check.ps1 tests, in the native shell, without it;
# pinning it here means one unresolved bug does not mask the other fifty checks.
#
# Bash 3.2 compatible, because that is what macOS ships. No associative arrays.

set -u

here=$(cd "$(dirname "$0")" && pwd)
known_file=${KNOWN_FAILURES:-$here/known-failures.txt}

case "$(uname -s)" in
  Linux*)               platform=linux;   family=unix ;;
  Darwin*)              platform=macos;   family=unix ;;
  MINGW*|MSYS*|CYGWIN*) platform=windows; family=windows ;;
  *)                    platform=unknown; family=unknown ;;
esac

work=$(mktemp -d)
export SEMLITH_HOME="$work/home"

# Issue #106. `user_home` reads HOME before anything else on every platform
# including Windows, so redirecting HOME is the whole of the fix: every client
# configuration `semlith setup` writes now lands under $work.
#
# The real home is remembered, and only so that the check below can prove
# nothing under it moved. Nothing else reads it.
real_home=${HOME:-}
export HOME="$work/user-home"
mkdir -p "$HOME"
corpus="$work/corpus"
side="$work/side"
repo_root=$PWD

passes=0
fails=0
xfails=0
xpasses=0
skips=0
failed_ids=""

# The issue number when `id` is a known failure on this platform, and a
# non-zero status when it is not.
known_issue() {
  awk -v id="$1" -v p="$platform" -v f="$family" '
    /^[[:space:]]*#/ { next }
    /^[[:space:]]*$/ { next }
    $1 == id && ($2 == "all" || $2 == p || $2 == f) { print $3; found = 1; exit }
    END { exit !found }
  ' "$known_file"
}

# `check <id> <description> <function>`. Output is captured and printed only
# when it matters, so the log stays readable at a hundred checks.
check() {
  id=$1
  desc=$2
  fn=$3
  out=$("$fn" 2>&1)
  rc=$?
  issue=$(known_issue "$id")
  is_known=$?
  if [ $rc -eq 0 ] && [ $is_known -ne 0 ]; then
    passes=$((passes + 1))
    printf 'ok       %-34s %s\n' "$id" "$desc"
  elif [ $rc -ne 0 ] && [ $is_known -eq 0 ]; then
    xfails=$((xfails + 1))
    printf 'xfail    %-34s %s (#%s)\n' "$id" "$desc" "$issue"
  elif [ $rc -eq 0 ] && [ $is_known -eq 0 ]; then
    xpasses=$((xpasses + 1))
    failed_ids="$failed_ids $id"
    printf 'XPASS    %-34s %s\n' "$id" "$desc"
    printf '         #%s is fixed. Remove this id from known-failures.txt.\n' "$issue"
  else
    fails=$((fails + 1))
    failed_ids="$failed_ids $id"
    printf 'FAIL     %-34s %s\n' "$id" "$desc"
    printf '%s\n' "$out" | head -25 | sed 's/^/         /'
  fi
}

skip() {
  skips=$((skips + 1))
  printf 'skip     %-34s %s\n' "$1" "$2"
}

# --------------------------------------------------------------- preconditions

echo "platform : $platform ($(uname -s))"
echo "semlith  : $(command -v semlith || echo '<not on PATH>')"
echo "home     : $SEMLITH_HOME"
echo "HOME     : $HOME  (real home: ${real_home:-<unset>})"
echo "work     : $work"

command -v semlith >/dev/null || { echo "semlith is not on PATH"; exit 1; }
command -v jq >/dev/null || { echo "jq is not on PATH"; exit 1; }
echo "jq       : $(jq --version)"
echo

# ------------------------------------------- the real home, before and after

# The client configuration paths `semlith setup` writes, read out of
# `docs/clients.md`, which is the file `src/clients.rs` parses and therefore the
# only list that cannot drift from what setup actually touches. Resolved against
# the *real* home, which is the thing that must not move.
#
# `cksum` rather than a size, because a registration rewritten in place is the
# failure mode and it is often the same length.
#
# SC2088: the `~` is the literal text docs/clients.md spells, expanded below
# against the real home rather than the redirected HOME.
# shellcheck disable=SC2088
client_config_fingerprint() {
  [ -n "$real_home" ] || return 0
  {
    grep -o 'config \(os=[a-z]* \)\?path=[^ `]*' "$repo_root/docs/clients.md" 2>/dev/null |
      sed -e 's/.*path=//' -e 's/^"//' -e 's/"$//'
    echo '~/.claude.json'
    echo '~/.claude/settings.json'
  } | sort -u | while read -r target; do
    case "$target" in
      *%*) continue ;;                       # %APPDATA%, which this shell cannot expand
      "~/"*) target="$real_home/${target#\~/}" ;;
      "~") continue ;;
    esac
    [ -f "$target" ] || continue
    printf '%s %s\n' "$target" "$(cksum < "$target" 2>/dev/null)"
  done
}

home_before=$(client_config_fingerprint)

# ----------------------------------------------------------------- the corpus

setup_corpus() {
  # A clone rather than the checkout, so .git and .gitignore are present and
  # the corpus is not the directory the harness is running out of. git is not
  # what this file tests, though, so a failed clone falls back to a copy rather
  # than costing all seventy checks their coverage.
  git clone --depth 1 --quiet "file://$repo_root" "$corpus" 2>&1
  if [ ! -d "$corpus/src" ]; then
    echo "the clone produced nothing; copying the checkout instead"
    mkdir -p "$corpus"
    cp -R "$repo_root/src" "$corpus/src" || return 1
    # The fixtures exercise every document reader, so the corpus is poorer
    # without them even though indexing still works.
    mkdir -p "$corpus/tests"
    [ -d "$repo_root/tests/fixtures" ] && cp -R "$repo_root/tests/fixtures" "$corpus/tests/fixtures"
    for f in README.md CHANGELOG.md; do
      [ -f "$repo_root/$f" ] && cp "$repo_root/$f" "$corpus/"
    done
  fi
  [ -d "$corpus/src" ] || { echo "no corpus at $corpus"; return 1; }
  # Two awkward names, because a path with a space and a path outside ASCII are
  # both ordinary on a real machine and both break naive quoting.
  mkdir -p "$work/odd names/a repo" "$work/ünïcode" || return 1
  cp "$corpus/src/lib.rs" "$work/odd names/a repo/lib.rs" || return 1
  cp "$corpus/src/lib.rs" "$work/ünïcode/lib.rs" || return 1
  return 0
}

corpus_ready=1
if setup_corpus > "$work/clone.log" 2>&1; then
  echo "corpus   : $corpus"
else
  corpus_ready=0
  echo "corpus   : FAILED to prepare"
  sed 's/^/         /' "$work/clone.log"
fi
echo

# ------------------------------------------------------------------ inventory

c_version()   { semlith --version | grep -q '^semlith [0-9]'; }
c_help()      { semlith --help | grep -q 'index' && semlith --help | grep -q 'search'; }
c_languages() { semlith languages | grep -qi 'rust'; }
c_models()    { semlith models | grep -qi 'granite\|bge\|model'; }

check cli/version              "--version names a version"          c_version
check cli/help                 "--help lists the commands"          c_help
check cli/languages            "languages includes rust"            c_languages
check cli/models               "models lists at least one model"    c_models

# -------------------------------------------------------------------- indexing

c_index_fresh() {
  out=$(semlith index "$corpus/src" "$corpus/tests/fixtures" 2>&1) || { echo "$out"; return 1; }
  echo "$out" | grep -qE 'indexed [0-9]+ file' || { echo "$out"; return 1; }
  echo "$out" | grep -qE 'indexed 0 files' && { echo "nothing was indexed"; echo "$out"; return 1; }
  return 0
}

c_index_idempotent() {
  out=$(semlith index "$corpus/src" "$corpus/tests/fixtures" 2>&1) || { echo "$out"; return 1; }
  # Re-running must re-embed nothing; that is the promise the README makes.
  echo "$out" | grep -qE 'indexed 0 files' || { echo "a second index re-embedded files:"; echo "$out"; return 1; }
}

c_index_quiet() {
  loud=$(semlith index "$corpus/src" 2>&1 | wc -l)
  quiet=$(semlith index --quiet "$corpus/src" 2>&1 | wc -l)
  [ "$quiet" -le "$loud" ] || { echo "--quiet printed more ($quiet) than the default ($loud)"; return 1; }
}

c_index_missing() {
  out=$(semlith --store "$side-missing" index "$work/there-is-no-such-directory" 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  echo "$out" | grep -qiE 'no such|not found|unreadable|cannot' || {
    echo "no clear message for a missing path:"; echo "$out"; return 1; }
}

c_index_spaces() {
  out=$(semlith --store "$side-spaces" index "$work/odd names/a repo" 2>&1) || { echo "$out"; return 1; }
  echo "$out" | grep -qE 'indexed [1-9]' || { echo "$out"; return 1; }
}

c_index_unicode() {
  out=$(semlith --store "$side-unicode" index "$work/ünïcode" 2>&1) || { echo "$out"; return 1; }
  echo "$out" | grep -qE 'indexed [1-9]' || { echo "$out"; return 1; }
}

c_index_registers_nothing() {
  # A path that cannot be read should leave no store and no registry entry.
  out=$(semlith --store "$side-typo" index "$work/definitely-not-here" 2>&1)
  if [ -d "$side-typo" ]; then
    echo "a store was created for a path that does not exist:"
    echo "$out"
    return 1
  fi
}

c_index_airgap() {
  # The weights are cached by now, so --airgap must succeed without the network.
  semlith index --airgap --quiet "$corpus/src" > /dev/null 2>&1
}

if [ $corpus_ready -eq 1 ]; then
  check cli/index/fresh        "a first index embeds files"         c_index_fresh
  check cli/index/idempotent   "a second index re-embeds nothing"   c_index_idempotent
  check cli/index/quiet        "--quiet is not louder"              c_index_quiet
  check cli/index/missing-path "a missing path is a clear message"  c_index_missing
  check cli/index/spaces       "a path with a space indexes"        c_index_spaces
  check cli/index/unicode      "a path outside ASCII indexes"       c_index_unicode
  check cli/index/airgap       "--airgap works once weights cached" c_index_airgap
  check cli/index/missing-registers-nothing "a bad path leaves no store" c_index_registers_nothing
else
  for id in fresh idempotent quiet missing-path spaces unicode airgap missing-registers-nothing; do
    skip "cli/index/$id" "the corpus could not be prepared"
  done
fi

# ------------------------------------------------------------------- the store

c_stats() {
  out=$(semlith stats 2>&1) || { echo "$out"; return 1; }
  for field in files chunks model vectors; do
    echo "$out" | grep -qiE "^ *$field +[^ ]" || {
      echo "stats names no $field:"; echo "$out"; return 1; }
  done
  # The counts must be real, not zeroes standing in for an unopened store.
  n=$(echo "$out" | awk '$1 == "files" { print $2 }')
  [ "${n:-0}" -gt 0 ] || { echo "stats reports $n files:"; echo "$out"; return 1; }
}

c_files_nonempty() { [ "$(semlith files | wc -l)" -gt 10 ]; }

c_files_no_verbatim() {
  semlith files > "$work/files.out"
  if grep -qF '\\?\' "$work/files.out"; then
    echo "verbatim paths in the file list:"
    grep -m 3 -F '\\?\' "$work/files.out"
    return 1
  fi
}

c_files_resolve() {
  # Whatever form the listing uses, every entry has to name a file that is
  # there. No `head` here: a truncated pipe panics until #75 is fixed, and
  # that panic would be charged to this check.
  semlith files > "$work/files.all"
  missing=0
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    [ -e "$f" ] || { echo "listed but not on disk: $f"; missing=$((missing + 1)); }
    [ "$missing" -ge 3 ] && break
  done < "$work/files.all"
  [ "$missing" -eq 0 ]
}

c_pipe_survives() {
  # The output has to exceed the pipe buffer, or the writer finishes before the
  # reader closes and nothing is proved either way. `files` over a small corpus
  # is a few kilobytes, which is why this check passed on one macOS machine and
  # failed on another. A --json search carries the chunk text, so it is large.
  size=$(semlith search "store" -k 300 --json | wc -c)
  [ "${size:-0}" -gt 262144 ] || {
    echo "only $size bytes of output, too little to close a pipe against"; return 1; }
  semlith search "store" -k 300 --json 2> "$work/sigpipe.err" | head -c 200 > /dev/null
  if grep -q 'panicked at' "$work/sigpipe.err"; then
    # The whole of it: the message on the line after the location differs by
    # platform (EPIPE on unix, a pipe-ended error on Windows) and naming it is
    # the useful part of the report.
    head -4 "$work/sigpipe.err"
    return 1
  fi
}

check cli/stats/shape          "stats names files and chunks"       c_stats
check cli/files/nonempty       "files lists the corpus"             c_files_nonempty
check cli/files/no-verbatim-paths "no \\\\?\\ paths in the listing"    c_files_no_verbatim
check cli/files/resolve        "every listed path exists"           c_files_resolve
check cli/pipe/survives-truncation "a truncated pipe does not panic" c_pipe_survives

# --------------------------------------------------------------------- search

c_search_basic() { semlith search "how does the store lock work" -k 3 | grep -qE '[0-9]+\.[0-9]+'; }

c_search_json() {
  out=$(semlith search "embedding model" -k 3 --json) || { echo "$out"; return 1; }
  echo "$out" | jq -e 'type == "array" and length > 0' > /dev/null \
    || { echo "not a non-empty array:"; echo "$out" | head -5; return 1; }
  echo "$out" | jq -e '.[0] | has("path") and has("start_line") and has("end_line") and has("score")' > /dev/null \
    || { echo "a hit is missing a field:"; echo "$out" | jq -c '.[0]'; return 1; }
}

# Every filter check runs through this, so a filter that returns nothing at all
# is a failure rather than a silent pass. That is how --lang, --ext and --path
# were all green while jq was erroring on the wrong shape.
filtered_paths() {
  out=$(semlith search "$@" --json) || { echo "search failed:"; echo "$out"; return 1; }
  n=$(printf '%s' "$out" | jq 'length')
  [ "${n:-0}" -gt 0 ] || { echo "the filter returned no hits at all, so it proves nothing"; return 1; }
  printf '%s' "$out" | jq -r '.[].path'
}

c_search_k() {
  n=$(semlith search "store" -k 2 --json | jq 'length')
  [ -n "$n" ] || { echo "-k 2 produced no parseable output"; return 1; }
  [ "$n" -le 2 ] || { echo "-k 2 returned $n hits"; return 1; }
  [ "$n" -gt 0 ] || { echo "-k 2 returned nothing"; return 1; }
}

c_search_lang() {
  paths=$(filtered_paths "parse" --lang rust -k 8) || { echo "$paths"; return 1; }
  bad=$(printf '%s\n' "$paths" | grep -v '\.rs$' | head -3)
  [ -z "$bad" ] || { echo "--lang rust returned non-Rust paths:"; echo "$bad"; return 1; }
}

c_search_ext() {
  paths=$(filtered_paths "the" --ext md -k 5) || { echo "$paths"; return 1; }
  bad=$(printf '%s\n' "$paths" | grep -v '\.md$' | head -3)
  [ -z "$bad" ] || { echo "--ext md returned other extensions:"; echo "$bad"; return 1; }
}

c_search_path_glob() {
  paths=$(filtered_paths "lock" --path 'src/*' -k 5) || { echo "$paths"; return 1; }
  bad=$(printf '%s\n' "$paths" | grep -v 'src' | head -3)
  [ -z "$bad" ] || { echo "--path src/* reached outside src:"; echo "$bad"; return 1; }
}

c_search_prefer() {
  semlith search "how it works" --prefer docs -k 3 --json | jq -e 'type == "array"' > /dev/null
}

c_search_repeatable() {
  # --lang, --ext and --path are all documented as repeatable.
  paths=$(filtered_paths "parse" --lang rust --lang markdown -k 5) || { echo "$paths"; return 1; }
  bad=$(printf '%s\n' "$paths" | grep -vE '\.(rs|md)$' | head -3)
  [ -z "$bad" ] || { echo "two --lang values admitted a third language:"; echo "$bad"; return 1; }
}

c_search_empty() {
  out=$(semlith search "" 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  return 0
}

c_search_unicode() {
  out=$(semlith search "wie funktioniert die Sperre — 日本語" -k 2 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  return 0
}

c_search_quotes() {
  out=$(semlith search "a \"quoted\" phrase with 'both' kinds" -k 2 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  return 0
}

c_search_no_verbatim() {
  out=$(semlith search "store lock" -k 5 --json | jq -r '.[].path')
  if printf '%s' "$out" | grep -qF '\\?\'; then
    echo "verbatim paths among the hits:"; printf '%s\n' "$out" | head -3
    return 1
  fi
}

check cli/search/basic         "search returns scored hits"         c_search_basic
check cli/search/json          "--json carries path and line range" c_search_json
check cli/search/k             "-k bounds the hit count"            c_search_k
check cli/search/lang          "--lang narrows to that language"    c_search_lang
check cli/search/ext           "--ext narrows to that extension"    c_search_ext
check cli/search/path-glob     "--path narrows to that subtree"     c_search_path_glob
check cli/search/prefer        "--prefer is accepted"               c_search_prefer
check cli/search/repeatable    "--lang may be repeated"             c_search_repeatable
check cli/search/empty-query   "an empty query does not panic"      c_search_empty
check cli/search/unicode       "a non-ASCII query does not panic"   c_search_unicode
check cli/search/quotes        "a quoted query does not panic"      c_search_quotes
check cli/search/no-verbatim-paths "no \\\\?\\ paths among hits"       c_search_no_verbatim

# ----------------------------------------------------------------------- read

# The snapshot's own copy, named by a suffix only it has. From 0.22.0 the
# repository carries a pinned snapshot of itself at
# `tests/fixtures/retrieval/corpus`, so a clone holds both `<corpus>/src/main.rs`
# and `<corpus>/tests/fixtures/retrieval/corpus/src/main.rs`. Every suffix of
# the first is also a suffix of the second, so only the second can be named
# unambiguously by suffix at all — `read` refuses the first by name, which is
# the documented behaviour and the right one.
#
# A suffix and not the absolute path, which was tried and works on Linux and
# macOS and not on Windows: under Git Bash `$corpus` is a POSIX path and the
# store recorded a Windows one, so nothing matched and the check said "nothing
# indexed at that span". A suffix is the same string on all three.
READ_TARGET="retrieval/corpus/src/main.rs"
c_read_span() { semlith read "$READ_TARGET:28-40" | grep -q '.'; }
c_read_line() { semlith read "$READ_TARGET:30" | grep -q '.'; }
c_read_symbol() { semlith read "main" | grep -q '.'; }

c_read_round_trip() {
  # The property the whole two-stage design rests on: a locator search printed
  # must be one `read` accepts. This is what a verbatim path breaks.
  loc=$(semlith search "store lock" -k 1 --json \
        | jq -r '.[0] | "\(.path):\(.start_line)-\(.end_line)"')
  [ -n "$loc" ] && [ "$loc" != "null" ] || { echo "search produced no locator"; return 1; }
  out=$(semlith read "$loc" 2>&1) || { echo "read $loc failed:"; echo "$out" | head -5; return 1; }
  [ -n "$out" ] || { echo "read $loc returned nothing"; return 1; }
}

c_read_missing() {
  out=$(semlith read "src/there-is-no-such-file.rs:1-2" 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  return 0
}

c_read_json() { semlith read "$READ_TARGET:28-40" --json | jq -e '.' > /dev/null; }

check cli/read/span            "read takes path:start-end"          c_read_span
check cli/read/line            "read takes path:line"               c_read_line
check cli/read/symbol          "read takes a symbol name"           c_read_symbol
check cli/read/locator-round-trip "a search locator reads back"     c_read_round_trip
check cli/read/missing         "a bad target does not panic"        c_read_missing
check cli/read/json            "--json is valid JSON"               c_read_json

# ------------------------------------------------------------------ code graph

c_symbol_known()   { semlith symbol main | grep -q '.'; }
c_symbol_unknown() {
  out=$(semlith symbol zzz_no_such_symbol_zzz 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  return 0
}
c_neighbors()      { semlith neighbors main | grep -q '.'; }
c_neighbors_json() { semlith neighbors main --json | jq -e '.' > /dev/null; }
c_neighbors_kind() { semlith neighbors main --kind calls --json | jq -e '.' > /dev/null; }
c_symbol_json()    { semlith symbol main --json | jq -e '.' > /dev/null; }
c_path_pair() {
  out=$(semlith path main open_store --depth 4 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  return 0
}
c_pattern_rust() { semlith pattern '(function_item name: (identifier) @name)' --lang rust | grep -q '@name'; }
c_pattern_no_lang() {
  out=$(semlith pattern '(function_item)' 2>&1)
  # --lang is required, so this must be a usage error and not a panic.
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  [ -n "$out" ] || { echo "no message at all for a missing --lang"; return 1; }
}
c_pattern_bad_query() {
  out=$(semlith pattern '(this is not a valid query' --lang rust 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  [ -n "$out" ] || { echo "no message for a malformed pattern"; return 1; }
}

check cli/symbol/known         "symbol finds a definition"          c_symbol_known
check cli/symbol/unknown       "an unknown symbol does not panic"   c_symbol_unknown
check cli/neighbors/known      "neighbors answers"                  c_neighbors
check cli/neighbors/json       "--json is valid JSON"               c_neighbors_json
check cli/neighbors/kind       "--kind follows one edge kind"       c_neighbors_kind
check cli/symbol/json          "symbol --json is valid JSON"        c_symbol_json
check cli/path/pair            "path answers for a pair"            c_path_pair
check cli/pattern/rust         "a tree-sitter pattern matches"      c_pattern_rust
check cli/pattern/no-lang      "a missing --lang is a clear error"  c_pattern_no_lang
check cli/pattern/bad-query    "a malformed pattern is an error"    c_pattern_bad_query

# ------------------------------------------------------------- forget and add

c_forget_removes() {
  # The listing prints platform paths -- `C:\\...\\src\\lock.rs` on Windows, where
  # this shell's own `$corpus` is an MSYS path. Compare on the tail with
  # separators normalised, so the check is about forgetting rather than about
  # which shell wrote the path.
  suffix="src/lock.rs"
  target=$(semlith files | tr '\\' '/' | grep -F "$suffix" | head -1)
  semlith files | tr '\\' '/' > "$work/before.txt"
  [ -n "$target" ] || {
    echo "the target is not in the listing, so this proves nothing"; return 1; }
  out=$(semlith forget "$target" 2>&1)
  semlith files | tr '\\' '/' > "$work/after.txt"
  before=$(wc -l < "$work/before.txt")
  after=$(wc -l < "$work/after.txt")
  if grep -qF "$target" "$work/after.txt"; then
    echo "still listed after forget. It said:"
    echo "$out"
    echo "count $before then $after"
    semlith index --quiet "$corpus/src" > /dev/null 2>&1
    return 1
  fi
  semlith index --quiet "$corpus/src" > /dev/null 2>&1
}

c_forget_missing() {
  out=$(semlith forget "$corpus/src/there-is-no-such-file.rs" 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  return 0
}

c_add_url() {
  out=$(semlith add "https://raw.githubusercontent.com/semlith/semlith/main/LICENSE" 2>&1) || {
    echo "$out"; return 1; }
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  return 0
}

c_add_bad_scheme() {
  out=$(semlith add "ftp://example.com/x" 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  [ -n "$out" ] || { echo "no message for a non-https URL"; return 1; }
}

check cli/forget/removes       "forget drops a file"                c_forget_removes
check cli/forget/missing       "forgetting an unknown path is calm" c_forget_missing
check cli/add/url              "add fetches one https URL"          c_add_url
check cli/add/bad-scheme       "a non-https URL is refused"         c_add_bad_scheme

# -------------------------------------------------------------------- ledger

c_ledger_prints() { semlith ledger --last 5 > /dev/null 2>&1; }
c_ledger_verify() { semlith ledger --verify > /dev/null 2>&1; }
c_ledger_json()   { semlith ledger --last 5 --json | jq -e '.' > /dev/null; }

check cli/ledger/prints        "ledger prints rows"                 c_ledger_prints
check cli/ledger/verify        "--verify walks the hash chain"      c_ledger_verify
check cli/ledger/json          "--json is valid JSON"               c_ledger_json

# ----------------------------------------------------------------------- keys

c_key_show() { semlith key show | grep -qE 'sml_[0-9a-f]+'; }
c_key_rotate() {
  before=$(semlith key show | grep -oE 'sml_[0-9a-f]+' | head -1)
  semlith key rotate > /dev/null 2>&1 || return 1
  after=$(semlith key show | grep -oE 'sml_[0-9a-f]+' | head -1)
  [ "$before" != "$after" ] || { echo "the key did not change: $before"; return 1; }
}

check cli/key/show             "key show prints an agent key"       c_key_show
check cli/key/rotate           "key rotate mints a new one"         c_key_rotate

# ------------------------------------------------------------------ MCP stdio

mcp_call() {
  printf '%s\n%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    "$1" | semlith mcp 2>/dev/null
}

c_mcp_initialize() { mcp_call '{"jsonrpc":"2.0","id":2,"method":"ping"}' | grep -q '"result"'; }
c_mcp_tools_list() {
  out=$(mcp_call '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}')
  printf '%s' "$out" | grep -q '"tools"' || { echo "$out" | head -3; return 1; }
  # Every tool must name itself and say what it takes, or a client cannot call it.
  printf '%s\n' "$out" \
    | jq -e -s 'map(select(.id == 2 and (.result.tools // null) != null)) | .[0].result.tools
                | length > 0 and all(has("name") and has("inputSchema"))' > /dev/null \
    || { echo "a tool is missing name or inputSchema:"; printf '%s\n' "$out" | head -2; return 1; }
}
c_mcp_tools_call() {
  out=$(mcp_call '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search","arguments":{"query":"store lock"}}}')
  printf '%s' "$out" | grep -q '"result"' || { echo "$out" | head -5; return 1; }
}
c_mcp_bad_method() {
  out=$(mcp_call '{"jsonrpc":"2.0","id":2,"method":"no/such/method"}')
  printf '%s' "$out" | grep -q '"error"' || { echo "an unknown method gave no JSON-RPC error:"; echo "$out" | head -3; return 1; }
  printf '%s' "$out" | grep -qi 'panic' && { echo "panicked"; return 1; }
  return 0
}

check cli/mcp/initialize       "MCP over stdio initializes"         c_mcp_initialize
check cli/mcp/tools-list       "tools/list describes every tool"    c_mcp_tools_list
check cli/mcp/tools-call       "tools/call answers a search"        c_mcp_tools_call
check cli/mcp/bad-method       "an unknown method is an error"      c_mcp_bad_method

# ------------------------------------------------------------- store selection

c_store_flag() {
  alt="$work/alt-store"
  semlith --store "$alt" index --quiet "$work/ünïcode" > /dev/null 2>&1 || return 1
  [ -d "$alt" ] || { echo "--store did not create $alt"; return 1; }
  semlith --store "$alt" stats > /dev/null 2>&1
}

c_store_env() {
  alt="$work/env-store"
  SEMLITH_STORE="$alt" semlith index --quiet "$work/ünïcode" > /dev/null 2>&1 || return 1
  [ -d "$alt" ] || { echo "SEMLITH_STORE did not create $alt"; return 1; }
}

c_trust_list() { semlith trust --list > /dev/null 2>&1; }

check cli/store/flag           "--store selects a directory"        c_store_flag
check cli/store/env            "SEMLITH_STORE selects a directory"  c_store_env
check cli/trust/list           "trust --list answers"               c_trust_list

# --------------------------------------------------------------- setup, upgrade

# `--no-service` because this check is about setup re-running cleanly, and from
# 0.21.0 setup installs a login service by default — which starts a daemon on
# the port the `cli/daemon/*` checks below need to bind, and every one of them
# then fails on a port this harness took from itself.
c_setup_idempotent() { semlith setup --yes --airgap --no-service > /dev/null 2>&1; }
c_upgrade_check() {
  semlith upgrade --check > /dev/null 2>&1
  rc=$?
  # 0 is current, 10 is an upgrade available. Anything else is a failure.
  [ "$rc" -eq 0 ] || [ "$rc" -eq 10 ] || { echo "upgrade --check exited $rc"; return 1; }
}
c_upgrade_airgap() {
  out=$(semlith upgrade --check --airgap 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  return 0
}

# 0.30.0: setup upgrades a Claude Code entry in place with alwaysLoad, and
# writes the hook matching Bash|Read|Grep|Glob with its PostToolUse entry —
# into the redirected HOME, never the real one.
c_setup_always_load() {
  mkdir -p "$HOME/.claude"
  printf '{"mcpServers":{"semlith":{"command":"semlith","args":["mcp"]}}}' > "$HOME/.claude.json"
  semlith setup --yes --airgap --no-service > /dev/null 2>&1 || { echo "setup failed"; return 1; }
  [ "$(jq -r '.mcpServers.semlith.alwaysLoad' "$HOME/.claude.json")" = "true" ] ||
    { echo "no alwaysLoad:"; cat "$HOME/.claude.json"; return 1; }
  jq -e '.hooks.PreToolUse[] | select(.matcher == "Bash|Read|Grep|Glob")' "$HOME/.claude/settings.json" > /dev/null ||
    { echo "no Bash-aware hook:"; cat "$HOME/.claude/settings.json"; return 1; }
  jq -e '.hooks.PostToolUse[] | select(.matcher == "mcp__.*semlith.*")' "$HOME/.claude/settings.json" > /dev/null ||
    { echo "no PostToolUse entry:"; cat "$HOME/.claude/settings.json"; return 1; }
}

check cli/setup/idempotent     "setup re-runs cleanly"              c_setup_idempotent
check cli/setup/always-load    "setup writes alwaysLoad and both hooks" c_setup_always_load
check cli/upgrade/check        "--check exits 0 or 10"              c_upgrade_check
check cli/upgrade/airgap       "--airgap refuses the network"       c_upgrade_airgap

# --------------------------------------------------------------------- watch

# Ctrl-C is how `semlith watch` ends, so it has to leave the store whole.
#
# There has been a Rust test saying exactly that since 0.6.0, and it is
# `#[ignore]`d, so CI has never run it. It failed for three releases without
# anyone hearing (#85). A correctness claim nobody checks is not a claim, so
# the guarantee is asserted here instead, against the installed binary, on
# every develop PR.
#
# Windows has no row here and that is deliberate rather than an omission:
# `watch::stop_on_signal` is `#[cfg(unix)]` and Ctrl-C on Windows is a
# documented no-op, so there is no behaviour to assert.
c_watch_sigint_whole() {
  wdir=$work/watchsig
  mkdir -p "$wdir/corpus"
  printf 'Sourdough needs flour and water.\n' > "$wdir/corpus/bread.md"
  semlith --store "$wdir/store" index "$wdir/corpus" > "$wdir/index.out" 2>&1 ||
    { sed 's/^/  /' "$wdir/index.out"; return 1; }

  semlith --store "$wdir/store" watch "$wdir/corpus" > "$wdir/watch.out" 2>&1 &
  wpid=$!

  # Wait for the watcher to say it is watching, never for a clock. The banner
  # is printed after the signal handler is installed, and the first exec of a
  # large binary can spend seconds in the kernel before `main` runs at all --
  # which is what made the Rust test's two-second sleep start failing.
  i=0
  while [ "$i" -lt 90 ]; do
    grep -q '^watching ' "$wdir/watch.out" && break
    kill -0 "$wpid" 2>/dev/null ||
      { echo "the watcher exited before it was ready"; sed 's/^/  /' "$wdir/watch.out"; return 1; }
    sleep 1
    i=$((i + 1))
  done
  grep -q '^watching ' "$wdir/watch.out" ||
    { echo "the watcher never said it was watching"; sed 's/^/  /' "$wdir/watch.out"; return 1; }

  kill -INT "$wpid" || { echo "could not signal the watcher"; return 1; }

  # Bounded, because the failure this guards against includes a watcher that
  # never stops. A check that hangs the job reports nothing; one that fails
  # names the bug.
  i=0
  while kill -0 "$wpid" 2>/dev/null && [ "$i" -lt 30 ]; do sleep 1; i=$((i + 1)); done
  if kill -0 "$wpid" 2>/dev/null; then
    kill -KILL "$wpid" 2>/dev/null
    wait "$wpid" 2>/dev/null
    echo "the watcher was still running 30s after SIGINT"
    sed 's/^/  /' "$wdir/watch.out"
    return 1
  fi
  wait "$wpid"
  rc=$?
  [ "$rc" -eq 0 ] ||
    { echo "the watcher exited $rc rather than 0 after SIGINT"; sed 's/^/  /' "$wdir/watch.out"; return 1; }

  # Nothing half-written left behind, in either store format.
  leftovers=$(find "$wdir/store" -name '*.tmp' 2>/dev/null)
  [ -z "$leftovers" ] || { echo "a half-written index was left behind: $leftovers"; return 1; }

  # And the two halves still agree: every chunk has a vector.
  semlith --store "$wdir/store" stats > "$wdir/stats.out" 2>&1 ||
    { sed 's/^/  /' "$wdir/stats.out"; return 1; }
  chunks=$(awk '$1 == "chunks" { print $2 }' "$wdir/stats.out")
  vectors=$(awk '$1 == "vectors" { print $2 }' "$wdir/stats.out")
  [ -n "$chunks" ] && [ "$chunks" = "$vectors" ] ||
    { echo "$chunks chunks against $vectors vectors after the interrupt"
      sed 's/^/  /' "$wdir/stats.out"; return 1; }
}

if [ "$family" = unix ]; then
  check cli/watch/sigint-whole   "Ctrl-C leaves the store whole"      c_watch_sigint_whole
fi

# -------------------------------------------------------------------- daemon

port=${SMOKE_PORT:-7365}
daemon_pid=""

start_daemon() {
  semlith start --port "$port" > "$work/daemon.out" 2>&1 &
  daemon_pid=$!
  i=0
  while [ $i -lt 90 ]; do
    if curl -fsS -o /dev/null "http://127.0.0.1:$port/" 2>/dev/null; then return 0; fi
    kill -0 "$daemon_pid" 2>/dev/null || return 1
    sleep 1
    i=$((i + 1))
  done
  return 1
}

c_daemon_up() { [ -n "$daemon_pid" ]; }
c_daemon_portal() { [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/")" = 200 ]; }
c_daemon_announces() {
  grep -qi 'listening on 127.0.0.1' "$work/daemon.out" || { sed 's/^/  /' "$work/daemon.out"; return 1; }
}
c_daemon_ledger_notice() {
  # The daemon promises to say on every start that it is recording.
  grep -qi 'ledger' "$work/daemon.out" || { sed 's/^/  /' "$work/daemon.out"; return 1; }
}
c_daemon_token_url() { grep -qE 'token=[0-9a-f]{16,}' "$work/daemon.out"; }
c_daemon_mcp_needs_key() {
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST "http://127.0.0.1:$port/mcp" \
    -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')
  [ "$code" = 401 ] || { echo "/mcp without a key answered $code, not 401"; return 1; }
}
c_daemon_mcp_with_key() {
  key=$(semlith key show | grep -oE 'sml_[0-9a-f]+' | head -1)
  body=$(curl -s -X POST "http://127.0.0.1:$port/mcp" \
    -H 'content-type: application/json' -H "Authorization: Bearer $key" \
    -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}')
  printf '%s' "$body" | grep -q '"tools"' || { echo "$body" | head -3; return 1; }
}
c_daemon_second_instance() {
  # A second `semlith start` against the store a live daemon holds is not a
  # failure from 0.27.0: nothing went wrong, the portal is already up. It
  # names the daemon that holds the store, prints that daemon's portal URL —
  # the one already bookmarked, never a new port — and exits 0. It must not
  # start a second listener, and it must not print an error chain.
  out=$(semlith start --port "$port" 2>&1)
  rc=$?
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  [ "$rc" -eq 0 ] || { echo "a second start against a held store exited $rc:"; echo "$out" | head -5; return 1; }
  echo "$out" | grep -qE '^(Error|Caused by):' && {
    echo "a second start printed an error chain:"; echo "$out" | head -5; return 1; }
  echo "$out" | grep -q 'already served by a running daemon' || {
    echo "a second start did not say which daemon holds the store:"; echo "$out" | head -5; return 1; }
  echo "$out" | grep -qE "^http://127\.0\.0\.1:$port/\?token=[0-9a-f]{64}\$" || {
    echo "a second start did not print the running daemon's URL on a line of its own:"
    echo "$out" | head -5; return 1; }
  echo "$out" | grep -qi 'listening on' && {
    echo "a second daemon started anyway:"; echo "$out" | head -3; return 1; }
  return 0
}
c_daemon_search_while_up() { semlith search "store lock" -k 1 > /dev/null 2>&1; }

if start_daemon; then
  check cli/daemon/up            "the daemon starts"                c_daemon_up
  check cli/daemon/portal        "the portal answers 200"           c_daemon_portal
  check cli/daemon/announces     "it says where it listens"         c_daemon_announces
  check cli/daemon/ledger-notice "it says the ledger records"       c_daemon_ledger_notice
  check cli/daemon/token-url     "it prints a tokenised URL"        c_daemon_token_url
  check cli/daemon/mcp-needs-key "/mcp without a key is 401"        c_daemon_mcp_needs_key
  check cli/daemon/mcp-with-key  "/mcp with the key lists tools"    c_daemon_mcp_with_key
  check cli/daemon/second-instance "a second start names the running one" c_daemon_second_instance
  check cli/daemon/search-while-up "search works with it running"   c_daemon_search_while_up
else
  echo "the daemon did not come up:"
  sed 's/^/  /' "$work/daemon.out" 2>/dev/null | head -20
  for id in up portal announces ledger-notice token-url mcp-needs-key mcp-with-key second-instance search-while-up; do
    skip "cli/daemon/$id" "the daemon did not start"
  done
fi

if [ -n "$daemon_pid" ]; then
  kill "$daemon_pid" 2>/dev/null
  wait "$daemon_pid" 2>/dev/null
fi

c_daemon_releases_lock() {
  # Whatever the daemon held must be free once it is gone, or the next index
  # fails for a reason the user cannot see.
  semlith index --quiet "$corpus/src" > /dev/null 2>&1
}
check cli/daemon/releases-lock "the lock is free after it stops"    c_daemon_releases_lock

# ------------------------------------------------------------------- 0.28.0

# Run control, the limits, the accelerator lanes and their fallback, driven
# through the daemon's own routes the way the portal drives them. Each group
# gets a daemon of its own with a pristine SEMLITH_HOME and a port of its own:
# a store left behind by one group would start a catch-up run in the next, and
# a card nobody asked for is exactly what these checks cannot tell apart from a
# defect. HOME is the harness's, so the model the CLI checks fetched is reused.
#
# `check` runs each function in a subshell, so a daemon is started and stopped
# out here and anything one check hands to the next goes through a file.

runs_root="$work/runs"
mkdir -p "$runs_root"
rc_port=""
rc_pid=""
rc_token=""
rc_home=""
rc_dir=""

# What the daemon is told a path is. Under Git Bash `$work` is an MSYS path
# that means nothing to a Windows process; the mixed form is a Windows path
# with forward slashes, which also needs no escaping inside JSON.
native_path() {
  if [ "$family" = windows ]; then cygpath -m "$1"; else printf '%s' "$1"; fi
}

# N `## Section` headings, and so N chunks: Markdown is cut at its headings,
# and each section is well under the 800-character budget. A run that has to
# be caught while it is live gets a thousand: four hundred finish in about
# fifteen seconds on an M1, which is less than a slow poll and a model load.
rc_markdown() {
  awk -v n="$2" 'BEGIN {
    for (i = 0; i < n; i++) {
      printf "## Section %d\n\n", i
      for (j = 0; j < 6; j++) printf "This paragraph talks about indexing, embedding and search in section %d. ", i
      printf "\n\n"
    }
  }' > "$1"
}

# `rc_start <tag> [VAR=value ...]`: a daemon on its own home, up and holding a
# token, or a non-zero status with its log beside it.
#
# The port is whichever of these the daemon manages to bind, and never 7365:
# that is the port a developer's own daemon holds. Asking the daemon rather
# than probing first, because a probe that finds a port free says nothing
# about the moment after it: a second harness took one in exactly that gap
# while this block was being written.
rc_start() {
  rc_dir="$runs_root/$1"
  shift
  rc_home="$rc_dir/home"
  mkdir -p "$rc_home"
  for rc_port in 7481 7483 7487 7489 7491 7493 7497 7499; do
    env "$@" SEMLITH_HOME="$rc_home" semlith start --port "$rc_port" > "$rc_dir/daemon.out" 2>&1 &
    rc_pid=$!
    rc_token=""
    i=0
    while [ $i -lt 450 ]; do
      rc_token=$(grep -oE 'token=[0-9a-f]+' "$rc_dir/daemon.out" 2>/dev/null | head -1 | cut -d= -f2)
      if [ -n "$rc_token" ] &&
         curl -fsS -o /dev/null -m 2 -H "Semlith-Token: $rc_token" "http://127.0.0.1:$rc_port/api/index/runs" 2>/dev/null; then
        return 0
      fi
      kill -0 "$rc_pid" 2>/dev/null || break
      sleep 0.2
      i=$((i + 1))
    done
    rc_stop
    grep -q 'already in use' "$rc_dir/daemon.out" || return 1
  done
  return 1
}

rc_stop() {
  [ -n "$rc_pid" ] || return 0
  kill "$rc_pid" 2>/dev/null
  wait "$rc_pid" 2>/dev/null
  rc_pid=""
}

rc_get() {
  curl -sS -m 30 -H "Semlith-Token: $rc_token" "http://127.0.0.1:$rc_port$1"
}

# Every write needs all three: the token, a JSON content type, and an origin
# that is the daemon's own. `http::answer` refuses one without them before it
# looks at the token.
rc_post() {
  curl -sS -m 30 -X POST -H "Semlith-Token: $rc_token" -H 'Content-Type: application/json' \
    -H "Origin: http://127.0.0.1:$rc_port" -d "$2" "http://127.0.0.1:$rc_port$1"
}

# Index one folder as a store of its own, the way the portal's button does, and
# print the store's name.
rc_index() {
  rc_post /api/index "$(jq -nc --arg p "$(native_path "$1")" '{path: [$p]}')" | jq -r '.runs[0].store // empty'
}

# The store's newest card.
rc_run() {
  rc_get /api/index/runs | jq -c --arg s "$1" '[.runs[] | select(.store == $s)] | last // empty'
}

rc_field() { rc_run "$1" | jq -r ".$2 // empty"; }

# "files chunks" as the Stores page reads them.
rc_counts() {
  rc_get /api/stores | jq -r --arg s "$1" '.stores[] | select(.name == $s) | "\(.files) \(.chunks)"'
}

# `rc_until <seconds> <command...>`: the command, again and again, until it
# succeeds or the time is up. Every wait in this block goes through it, so a
# check that waits on a condition never waits on a clock. Whole seconds, since
# `date` on macOS has no finer unit: a limit of N gives up somewhere after N-1.
rc_until() {
  limit=$1
  shift
  end=$(( $(date +%s) + limit ))
  while :; do
    "$@" && return 0
    [ "$(date +%s)" -ge "$end" ] && return 1
    sleep 0.2
  done
}

rc_is() { [ "$(rc_field "$1" status)" = "$2" ]; }
rc_finished() { case "$(rc_field "$1" status)" in done|stopped|failed) return 0 ;; esac; return 1; }
rc_embedding() {
  r=$(rc_run "$1")
  [ "$(printf '%s' "$r" | jq -r '.status')" = running ] &&
    [ "$(printf '%s' "$r" | jq -r '.chunks // 0')" -gt 0 ]
}
rc_alive() { kill -0 "$rc_pid" 2>/dev/null; }

# The worker process a lane runs in, found by its command line and, where the
# platform says, by its parent, so no other daemon's worker is ever the one
# killed. Git Bash's `$!` is an MSYS pid; /proc gives the Windows one.
rc_worker_pid() {
  if [ "$family" = windows ]; then
    parent=$(cat "/proc/$rc_pid/winpid" 2>/dev/null)
    filter="\$_.CommandLine -like '*__embed-worker worker*'"
    [ -n "$parent" ] && filter="$filter -and \$_.ParentProcessId -eq $parent"
    powershell -NoProfile -NonInteractive -Command \
      "Get-CimInstance Win32_Process | Where-Object { $filter } | Select-Object -First 1 -ExpandProperty ProcessId" \
      2>/dev/null | tr -d '\r'
  else
    pgrep -P "$rc_pid" -f '__embed-worker worker' 2>/dev/null | head -1
  fi
}
rc_worker_up() { [ -n "$(rc_worker_pid)" ]; }

rc_kill_worker() {
  if [ "$family" = windows ]; then
    taskkill //F //PID "$1" > /dev/null 2>&1
  else
    kill -KILL "$1" 2>/dev/null
  fi
}

rc_lane() { rc_get /api/accel | jq -c --arg l "$1" '.lanes[] | select(.lane == $l)'; }

# ----- one run, paused, resumed and stopped; the catch-up card that carries it

# The run is a catch-up: small.md is indexed with no daemon running, big.md is
# added, and the daemon finds it on start. That is the card this release adds,
# and it is also the only way to have a run already going the moment a daemon
# is up, before anything could have raced it.
#
# Pinned to the CPU lane, here and below except where a lane is the subject:
# an Apple silicon runner has a Metal GPU, and the WebGPU lane would fetch its
# components and change what a rate or a thread count is measuring.

control_store=""
c_catch_up_card() {
  rc_until 300 eval '[ -n "$(rc_run "$control_store")" ]' ||
    { echo "no card appeared for $control_store"; rc_get /api/index/runs; return 1; }
  kind=$(rc_field "$control_store" kind)
  [ "$kind" = catch-up ] || { echo "the card's kind is \"$kind\", not catch-up:"; rc_run "$control_store"; return 1; }
}

c_pause_holds() {
  s=$control_store
  rc_until 300 rc_embedding "$s" ||
    { echo "the run never started embedding:"; rc_run "$s"; return 1; }
  rc_run "$s" | jq -c '{files_before, chunks_before}' > "$rc_dir/before.json"
  answer=$(curl -sS -m 30 -o "$rc_dir/pause.json" -w '%{time_total}' -X POST \
    -H "Semlith-Token: $rc_token" -H 'Content-Type: application/json' \
    -H "Origin: http://127.0.0.1:$rc_port" \
    -d "$(jq -nc --arg s "$s" '{store: $s, action: "pause"}')" \
    "http://127.0.0.1:$rc_port/api/index/control")
  ms=$(awk -v t="$answer" 'BEGIN { printf "%d", t * 1000 }')
  awk -v t="$answer" 'BEGIN { printf "%.1f\n", t * 1000 }' > "$rc_dir/pause-ms"
  # 100 ms is the release's promise: the route answers before the engine has
  # reached its next batch. Windows gets 300 because Git Bash's curl on a
  # hosted Windows runner has been seen to spend more than that connecting to
  # loopback before the daemon sees a byte; it is not a number that has been
  # measured there for this route, so tighten it if the runner shows it can.
  limit=100
  [ "$family" = windows ] && limit=300
  [ "$(jq -r '.state' "$rc_dir/pause.json")" = pausing ] ||
    { echo "pause answered:"; cat "$rc_dir/pause.json"; return 1; }
  [ "$ms" -lt "$limit" ] || { echo "pause took $ms ms, over $limit"; return 1; }
  # 3, because `rc_until` counts whole seconds and 3 is the first limit that
  # always allows the full 2.
  rc_until 3 rc_is "$s" paused ||
    { echo "not paused within 2 s of the answer:"; rc_run "$s"; return 1; }
  c1=$(rc_field "$s" chunks)
  # The one fixed wait in this block: holding still is the property, and only
  # a stretch of time can show it.
  sleep 2
  c2=$(rc_field "$s" chunks)
  [ "$c1" = "$c2" ] || { echo "paused, and the count still rose: $c1 -> $c2"; return 1; }
  rc_is "$s" paused || { echo "no longer paused:"; rc_run "$s"; return 1; }
  [ "$(rc_post /api/index/control "$(jq -nc --arg s "$s" '{store: $s, action: "resume"}')" | jq -r .state)" = running ] ||
    { echo "resume did not answer running"; return 1; }
  rc_until 60 eval '[ "$(rc_field "$s" chunks)" -gt "$c2" ]' ||
    { echo "resumed, and the count did not move from $c2:"; rc_run "$s"; return 1; }
}

c_stop_restores() {
  s=$control_store
  [ -s "$rc_dir/before.json" ] || { echo "the pause check recorded no pre-run counts"; return 1; }
  rc_is "$s" running || rc_until 30 rc_is "$s" running ||
    { echo "no live run to stop:"; rc_run "$s"; return 1; }
  [ "$(rc_post /api/index/control "$(jq -nc --arg s "$s" '{store: $s, action: "stop"}')" | jq -r .state)" = stopping ] ||
    { echo "stop did not answer stopping"; return 1; }
  rc_until 300 rc_is "$s" stopped || { echo "never reached stopped:"; rc_run "$s"; return 1; }
  want=$(jq -r '"\(.files_before) \(.chunks_before)"' "$rc_dir/before.json")
  cli=$(cat "$rc_dir/cli-counts" 2>/dev/null)
  got=$(rc_counts "$s")
  [ "$want" = "$cli" ] ||
    { echo "the card's pre-run counts ($want) are not what the store held before the daemon started ($cli)"; return 1; }
  [ "$got" = "$want" ] || { echo "the store holds $got after the stop, not its pre-run $want"; return 1; }
}

control_ready=0
cdir="$runs_root/control"
mkdir -p "$cdir/corpus" "$cdir/home"
printf '# Small\n\nA small file already in the store.\n' > "$cdir/corpus/small.md"
if SEMLITH_HOME="$cdir/home" semlith index --quiet "$cdir/corpus" > "$cdir/index.out" 2>&1; then
  control_store=$(jq -r '.stores | keys[0] // empty' "$cdir/home/registry.json" 2>/dev/null)
  SEMLITH_HOME="$cdir/home" semlith --store "$cdir/home/stores/$control_store" stats > "$cdir/stats.out" 2>&1
  awk '$1 == "files" { f = $2 } $1 == "chunks" { c = $2 } END { print f, c }' "$cdir/stats.out" > "$cdir/cli-counts"
  rc_markdown "$cdir/corpus/big.md" 1000
  [ -n "$control_store" ] && rc_start control SEMLITH_ACCEL=cpu && control_ready=1
fi
if [ $control_ready -eq 1 ]; then
  check cli/runs/catch-up-card   "a file added offline is a catch-up card" c_catch_up_card
  check cli/runs/pause-holds     "pause answers at once and holds"     c_pause_holds
  [ -f "$rc_dir/pause-ms" ] && echo "         pause answered in $(cat "$rc_dir/pause-ms") ms"
  check cli/runs/stop-restores   "stop puts the store back as it was"  c_stop_restores
else
  echo "the run-control daemon did not come up:"
  sed 's/^/  /' "$runs_root/control/index.out" "$runs_root/control/daemon.out" 2>/dev/null | head -20
  for id in catch-up-card pause-holds stop-restores; do skip "cli/runs/$id" "the run-control daemon did not start"; done
fi
rc_stop

# ----------------------------------------------- delete, rate, a live limit

c_delete_on_stop() {
  mkdir -p "$rc_dir/delete"
  rc_markdown "$rc_dir/delete/big.md" 1000
  s=$(rc_index "$rc_dir/delete")
  [ -n "$s" ] || { echo "the index route started no run"; return 1; }
  rc_until 300 rc_embedding "$s" || { echo "the run never started embedding:"; rc_run "$s"; return 1; }
  [ "$(rc_field "$s" files_before)" = 0 ] ||
    { echo "a new folder's run did not start from an empty store:"; rc_run "$s"; return 1; }
  dir="$rc_home/stores/$s"
  [ -d "$dir" ] || { echo "no store directory at $dir to delete, so this proves nothing"; return 1; }
  [ "$(rc_post /api/index/control "$(jq -nc --arg s "$s" '{store: $s, action: "stop", delete: true}')" | jq -r .state)" = stopping ] ||
    { echo "stop did not answer stopping"; return 1; }
  # No exception for Windows. A file the daemon still holds open is what keeps
  # a directory there on Windows and nowhere else, and a leftover directory is
  # a store the user was told was deleted.
  rc_until 120 eval '[ ! -e "$dir" ]' ||
    { echo "the store directory is still there after the stop:"; find "$dir" 2>&1 | head -10; rc_run "$s"; return 1; }
  jq -e --arg s "$s" '.stores | has($s) | not' "$rc_home/registry.json" > /dev/null ||
    { echo "the registry still names $s:"; jq -c '.stores | keys' "$rc_home/registry.json"; return 1; }
  [ -z "$(rc_counts "$s")" ] || { echo "/api/stores still lists $s"; return 1; }
}

# Null only before the first batch is the promise. A rate that blinks out
# between batches is a card that says "—" for a run that is plainly going.
c_rate_every_poll() {
  mkdir -p "$rc_dir/rate"
  rc_markdown "$rc_dir/rate/big.md" 1000
  s=$(rc_index "$rc_dir/rate")
  [ -n "$s" ] || { echo "the index route started no run"; return 1; }
  seen=0
  blank=0
  end=$(( $(date +%s) + 600 ))
  while [ "$(date +%s)" -lt "$end" ]; do
    r=$(rc_run "$s")
    st=$(printf '%s' "$r" | jq -r '.status // empty')
    case "$st" in done|stopped|failed) break ;; esac
    if [ "$st" = running ]; then
      if [ "$(printf '%s' "$r" | jq -r '.rate')" != null ]; then
        seen=$((seen + 1))
      elif [ "$seen" -gt 0 ]; then
        blank=$((blank + 1))
        [ "$blank" -le 3 ] && echo "no rate after the first batch: $r"
      fi
    fi
    sleep 0.3
  done
  [ "$st" = "done" ] || { echo "the run ended \"$st\", not done:"; rc_run "$s"; return 1; }
  [ "$seen" -ge 3 ] || { echo "only $seen polls saw a live rate, too few to prove anything"; return 1; }
  [ "$blank" -eq 0 ] || { echo "$blank polls after the first batch had no rate"; return 1; }
}

c_limit_live() {
  # Two threads to start from, so lowering to one is a change. A two-core
  # runner derives one, and then the check proves nothing.
  rc_post /api/index/settings '{"embed_threads": 2}' > /dev/null
  mkdir -p "$rc_dir/threads"
  rc_markdown "$rc_dir/threads/big.md" 1000
  s=$(rc_index "$rc_dir/threads")
  [ -n "$s" ] || { echo "the index route started no run"; return 1; }
  rc_until 300 eval 'rc_is "$s" running && [ -n "$(rc_field "$s" threads)" ]' ||
    { echo "the card never showed a thread count:"; rc_run "$s"; return 1; }
  before=$(rc_field "$s" threads)
  [ "$before" != 1 ] || { echo "the run already had one thread, so lowering it to one proves nothing"; return 1; }
  answer=$(rc_post /api/index/settings '{"embed_threads": 1}')
  printf '%s' "$answer" | jq -e 'has("applied")' > /dev/null ||
    { echo "the settings route did not say what it applied: $answer"; return 1; }
  rc_until 21 eval '[ "$(rc_field "$s" threads)" = 1 ]' ||
    { echo "threads still $(rc_field "$s" threads) 20 s after saving 1 (was $before):"; rc_run "$s"; return 1; }
  # Nothing more to learn from this run, and a thousand chunks on one thread
  # is minutes of runner time.
  rc_post /api/index/control "$(jq -nc --arg s "$s" '{store: $s, action: "stop"}')" > /dev/null
  rc_until 120 rc_finished "$s"
  return 0
}

if rc_start limits SEMLITH_ACCEL=cpu; then
  check cli/runs/delete-on-stop  "stop with delete removes the store"  c_delete_on_stop
  check cli/runs/rate-every-poll "a live run always carries a rate"    c_rate_every_poll
  check cli/runs/limit-live      "embed_threads reaches a live run"    c_limit_live
else
  echo "the limits daemon did not come up:"
  sed 's/^/  /' "$runs_root/limits/daemon.out" 2>/dev/null | head -20
  for id in delete-on-stop rate-every-poll limit-live; do skip "cli/runs/$id" "the limits daemon did not start"; done
fi
rc_stop

# ----------------------------------------------------- lowering runs at once

hold_stores=""
# How many of the three cards read `$1`.
hold_count() {
  rc_get /api/index/runs |
    jq --argjson s "$hold_stores" --arg st "$1" '[.runs[] | select(.store as $n | $s | any(. == $n)) | select(.status == $st)] | length'
}
hold_all_embedding() {
  # Every poll's view, for the failure message: what the three cards read
  # when they were not all embedding.
  rc_get /api/index/runs | jq -c '{running, r: [.runs[] | [.store, .kind, .status, .chunks]]}' >> "$rc_dir/hold-trace"
  [ "$(rc_get /api/index/runs | jq '.running' | tr -d '\r')" = 3 ] || return 1
  for s in $(printf '%s' "$hold_stores" | jq -r '.[]' | tr -d '\r'); do rc_embedding "$s" || return 1; done
}

c_lower_limit_holds() {
  rc_post /api/index/settings '{"runs_at_once": 3}' | jq -e 'has("applied")' > /dev/null ||
    { echo "runs_at_once 3 was not applied"; return 1; }
  for n in a b c; do
    mkdir -p "$rc_dir/hold-$n"
    rc_markdown "$rc_dir/hold-$n/big.md" 1000
  done
  body=$(jq -nc --arg a "$(native_path "$rc_dir/hold-a")" --arg b "$(native_path "$rc_dir/hold-b")" \
    --arg c "$(native_path "$rc_dir/hold-c")" '{path: [$a, $b, $c]}')
  hold_stores=$(rc_post /api/index "$body" | jq -c '[.runs[].store]')
  [ "$(printf '%s' "$hold_stores" | jq 'length')" = 3 ] || { echo "three folders started $hold_stores"; return 1; }
  rc_until 300 hold_all_embedding ||
    { echo "three runs were never embedding at once:"; rc_get /api/index/runs | jq -c '{running, held, runs_at_once: .limits.runs_at_once.value}'
      rc_get /api/index/runs | jq -c '.runs[] | {store, kind, status, chunks, elapsed_ms, summary}'
      echo "each distinct poll, in order:"; uniq "$rc_dir/hold-trace" | head -40
      tail -20 "$runs_root/hold/daemon.out" 2>/dev/null; return 1; }
  rc_post /api/index/settings '{"runs_at_once": 1}' | jq -e 'has("applied")' > /dev/null ||
    { echo "runs_at_once 1 was not applied"; return 1; }
  rc_until 6 eval '[ "$(hold_count running)" = 1 ] && [ "$(hold_count held)" = 2 ]' ||
    { echo "not one running and two held 5 s after lowering the limit:"
      rc_get /api/index/runs | jq -c '.runs[] | {store, status, chunks}'; return 1; }
  rc_until 900 eval '[ "$(hold_count done)" = 3 ]' ||
    { echo "the three runs did not all finish:"; rc_get /api/index/runs | jq -c '.runs[] | {store, status, chunks}'; return 1; }
  counts=$(for s in $(printf '%s' "$hold_stores" | jq -r '.[]' | tr -d '\r'); do rc_counts "$s"; done | sort -u)
  [ "$(printf '%s\n' "$counts" | wc -l | tr -d ' ')" = 1 ] && [ "${counts#* }" != 0 ] ||
    { echo "three equal corpora, and the stores hold:"; printf '%s\n' "$counts"; return 1; }
}

if rc_start hold SEMLITH_ACCEL=cpu; then
  check cli/runs/lower-limit-holds "lowering runs at once holds the rest" c_lower_limit_holds
else
  sed 's/^/  /' "$runs_root/hold/daemon.out" 2>/dev/null | head -20
  skip cli/runs/lower-limit-holds "the hold daemon did not start"
fi
rc_stop

# ------------------------------------------------ the daemon against the CLI

# The service indexed at a fifth of the terminal's speed before 0.28.0, because
# launchd ran it on the efficiency cores. The runners cannot supervise a login
# service, so this is the same corpus through a `semlith start` daemon against
# `semlith index` in the shell, wall clock both ways. Each daemon run is paired
# with the CLI run just before it and the median of the three ratios is what
# is judged: a runner's speed drifts by a quarter between runs, which a
# comparison of two medians taken minutes apart reads as the daemon. On
# Windows the daemon is put below normal priority first, the way the logon task
# starts it, so what is measured is its own lift.
now_ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time * 1000'; }
median3() { printf '%s\n' "$@" | sort -n | sed -n 2p; }

c_daemon_rate() {
  for i in 1 2 3; do
    mkdir -p "$rc_dir/cli-$i" "$rc_dir/served-$i"
    rc_markdown "$rc_dir/cli-$i/corpus.md" 1200
    rc_markdown "$rc_dir/served-$i/corpus.md" 1200
  done
  if [ "$family" = windows ]; then
    winpid=$(cat "/proc/$rc_pid/winpid" 2>/dev/null)
    powershell -NoProfile -Command "(Get-Process -Id $winpid).PriorityClass = 'BelowNormal'" ||
      { echo "could not lower the daemon's priority"; return 1; }
  fi
  cli="" served="" ratios=""
  for i in 1 2 3; do
    t=$(now_ms)
    SEMLITH_ACCEL=cpu semlith --store "$rc_dir/cli-store-$i" index "$(native_path "$rc_dir/cli-$i")" --quiet ||
      { echo "the CLI index failed"; return 1; }
    ms=$(( $(now_ms) - t ))
    chunks=$(semlith --store "$rc_dir/cli-store-$i" stats | awk '$1 == "chunks" {print $2}' | tr -d '\r')
    c=$(( chunks * 1000000 / ms ))
    cli="$cli $c"

    t=$(now_ms)
    s=$(rc_index "$rc_dir/served-$i")
    [ -n "$s" ] || { echo "the index route started no run"; return 1; }
    rc_until 900 rc_finished "$s" || { echo "the daemon's run did not finish:"; rc_run "$s"; return 1; }
    ms=$(( $(now_ms) - t ))
    [ "$(rc_field "$s" status)" = done ] || { echo "the daemon's run did not end done:"; rc_run "$s"; return 1; }
    got=$(rc_field "$s" chunks)
    [ "$got" = "$chunks" ] || { echo "the daemon made $got chunks and the CLI $chunks"; return 1; }
    d=$(( got * 1000000 / ms ))
    served="$served $d"
    ratios="$ratios $(( d * 100 / c ))"
  done
  # Milli-chunks per second, so the integer arithmetic keeps three places.
  r=$(median3 $ratios)
  echo "CLI$cli, daemon$served (chunks/s x 1000); daemon as % of the CLI run beside it:$ratios"
  [ "$r" -ge 85 ] || { echo "the daemon ran at a median $r % of the CLI, under 85 %"; return 1; }
}

if rc_start rate SEMLITH_ACCEL=cpu; then
  check cli/runs/daemon-rate "a daemon run keeps 85 % of the CLI's rate" c_daemon_rate
else
  sed 's/^/  /' "$runs_root/rate/daemon.out" 2>/dev/null | head -20
  skip cli/runs/daemon-rate "the rate daemon did not start"
fi
rc_stop

# --------------------------------------------------------- the worker lane

# A lane that dies takes nothing with it: its batch is handed back, the run
# finishes on the CPU with every chunk, the lane says why it failed, and the
# daemon is still up. `SEMLITH_ACCEL=cpu,worker` adds a lane that runs the
# same int8 model in a process of its own, so the whole fault path is
# exercised on runners that have no GPU at all.
#
# The clean count is what the CLI makes of the same file, because the CLI
# never hands a batch to a lane.
fault_clean=""
mkdir -p "$runs_root/fault-corpus" "$runs_root/fault-clean"
rc_markdown "$runs_root/fault-corpus/big.md" 600
if SEMLITH_HOME="$runs_root/fault-clean/home" semlith --store "$runs_root/fault-clean/store" \
     index --quiet "$runs_root/fault-corpus" > "$runs_root/fault-clean/index.out" 2>&1; then
  fault_clean=$(SEMLITH_HOME="$runs_root/fault-clean/home" semlith --store "$runs_root/fault-clean/store" stats 2>/dev/null |
    awk '$1 == "chunks" { print $2 }')
fi

c_worker_fault() {
  mkdir -p "$rc_dir/corpus"
  cp "$runs_root/fault-corpus/big.md" "$rc_dir/corpus/big.md"
  s=$(rc_index "$rc_dir/corpus")
  [ -n "$s" ] || { echo "the index route started no run"; return 1; }
  if [ "$1" = kill ]; then
    rc_until 300 eval 'rc_embedding "$s" && rc_worker_up' ||
      { echo "no worker process appeared while the run was live:"; rc_run "$s"; rc_lane worker; return 1; }
    # Paused first, so the kill cannot land after the last batch and prove
    # nothing: the next batch after the resume is the one that meets it.
    rc_post /api/index/control "$(jq -nc --arg s "$s" '{store: $s, action: "pause"}')" > /dev/null
    rc_until 30 rc_is "$s" paused || { echo "the run did not pause:"; rc_run "$s"; return 1; }
    wpid=$(rc_worker_pid)
    [ -n "$wpid" ] || { echo "the worker was gone before it could be killed"; return 1; }
    rc_kill_worker "$wpid" || { echo "could not kill worker $wpid"; return 1; }
    rc_post /api/index/control "$(jq -nc --arg s "$s" '{store: $s, action: "resume"}')" > /dev/null
  fi
  rc_until 600 rc_finished "$s" || { echo "the run never finished:"; rc_run "$s"; return 1; }
  st=$(rc_field "$s" status)
  [ "$st" = "done" ] || { echo "the run ended \"$st\":"; rc_run "$s"; return 1; }
  got=$(rc_counts "$s")
  [ "${got#* }" = "$fault_clean" ] || { echo "$got files and chunks, and a clean run makes $fault_clean chunks"; return 1; }
  lane=$(rc_lane worker)
  [ "$(printf '%s' "$lane" | jq -r '.status.state')" = failed ] &&
    [ -n "$(printf '%s' "$lane" | jq -r '.status.reason // empty')" ] ||
    { echo "the worker lane does not read failed with a reason: $lane"; return 1; }
  rc_alive || { echo "the daemon died with its lane"; return 1; }
}
c_worker_kill()  { c_worker_fault kill; }
c_worker_batch() { c_worker_fault batch; }
c_worker_hang()  { c_worker_fault hang; }

for fault in kill batch hang; do
  case $fault in
    kill)  id=worker-kill;        desc="a killed worker fails its lane only";  fn=c_worker_kill;  inject="" ;;
    batch) id=worker-batch-fault; desc="a failed batch fails its lane only";   fn=c_worker_batch; inject=worker:batch:3 ;;
    hang)  id=worker-hang;        desc="a hung worker fails its lane only";    fn=c_worker_hang;  inject=worker:hang:60000 ;;
  esac
  if [ -z "$fault_clean" ]; then
    sed 's/^/  /' "$runs_root/fault-clean/index.out" 2>/dev/null | head -10
    skip "cli/accel/$id" "the clean reference run failed"
  elif rc_start "fault-$fault" SEMLITH_ACCEL=cpu,worker SEMLITH_ACCEL_DEADLINE_MS=4000 \
         ${inject:+SEMLITH_FAULT_ACCEL=$inject}; then
    check "cli/accel/$id" "$desc" "$fn"
  else
    sed 's/^/  /' "$runs_root/fault-$fault/daemon.out" 2>/dev/null | head -20
    skip "cli/accel/$id" "the fault daemon did not start"
  fi
  rc_stop
done

# ---------------------------------------- a panicking reader, and no GPU

# One daemon for both: the panic needs a run, and the GPU lane is only asked,
# and so only found wanting, when a run hands it a batch.
#
# Every Apple silicon Mac has a Metal GPU, the hosted arm64 runner included,
# so there the WebGPU lane is meant to run and the fallback has no
# precondition. It is pinned to the CPU there so nothing is fetched.
apple_gpu=0
[ "$platform" = macos ] && [ "$(uname -m)" = arm64 ] && apple_gpu=1
model_cache=${SEMLITH_MODEL_CACHE:-$HOME/.cache/semlith/models}
panic_store=""

c_panic_isolated() {
  [ -n "$panic_store" ] || { echo "the index route started no run"; return 1; }
  r=$(rc_run "$panic_store")
  [ "$(printf '%s' "$r" | jq -r .status)" = "done" ] || { echo "the run did not finish done:"; echo "$r"; return 1; }
  printf '%s' "$r" | jq -e '.summary.failed | any((.path | endswith("boom.md")) and (.why | test("panic")))' > /dev/null ||
    { echo "boom.md is not named as failed by a panic:"; printf '%s' "$r" | jq -c '.summary.failed'; return 1; }
  [ "$(rc_get /api/stores | jq -r --arg s "$panic_store" '.stores[] | select(.name == $s) | .watching')" = true ] ||
    { echo "the store stopped watching:"; rc_get /api/stores | jq -c --arg s "$panic_store" '.stores[] | select(.name == $s)'; return 1; }
  rc_alive || { echo "the daemon died"; return 1; }
}

c_gpu_fallback() {
  [ -n "$panic_store" ] || { echo "no run was made to hand the lane a batch"; return 1; }
  rc_until 30 eval '[ "$(rc_lane gpu | jq -r .status.state)" = unavailable ]' ||
    { echo "the gpu lane does not read unavailable after a run: $(rc_lane gpu)"; return 1; }
  if [ -d "$model_cache/accel" ] && [ -n "$(find "$model_cache/accel" -mindepth 1 2>/dev/null | head -1)" ]; then
    echo "a machine with no GPU fetched components:"
    find "$model_cache/accel" -mindepth 1 | head -5
    return 1
  fi
}

if [ $apple_gpu -eq 1 ]; then panic_env=SEMLITH_ACCEL=cpu; else panic_env=""; fi
if rc_start panic SEMLITH_FAULT_PANIC=boom.md ${panic_env:+"$panic_env"}; then
  mkdir -p "$rc_dir/corpus"
  printf '# Boom\n\nThis reader panics.\n' > "$rc_dir/corpus/boom.md"
  rc_markdown "$rc_dir/corpus/fine.md" 200
  panic_store=$(rc_index "$rc_dir/corpus")
  [ -n "$panic_store" ] && rc_until 300 rc_finished "$panic_store"
  check cli/runs/panic-isolated  "a panicking reader fails one file"   c_panic_isolated
  if [ $apple_gpu -eq 1 ]; then
    skip cli/accel/gpu-fallback "Apple silicon has a Metal GPU, so the WebGPU lane is meant to run"
  else
    check cli/accel/gpu-fallback "no GPU: lane unavailable, nothing fetched" c_gpu_fallback
  fi
else
  sed 's/^/  /' "$runs_root/panic/daemon.out" 2>/dev/null | head -20
  skip cli/runs/panic-isolated "the panic daemon did not start"
  skip cli/accel/gpu-fallback "the panic daemon did not start"
fi
rc_stop

# ------------------------------------------------------------------- 0.21.0

# The handshake must not be behind the embedding model. Proved by making the
# model impossible to load — airgapped, empty cache — so a server that still
# answers is a server that never waited for one. On 0.20.2 this printed nothing
# at all and exited.
c_mcp_handshake_without_model() {
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
    | SEMLITH_AIRGAP=1 SEMLITH_MODEL_CACHE="$work/no-model" \
      semlith mcp 2> "$work/mcp-airgap.err" > "$work/mcp-airgap.out"
  # Two complete responses, and stderr saying the model is being loaded behind
  # them — where a stdio client captures it.
  #
  # The line asserted is the one printed on the way past, not the one the warm
  # thread prints when it fails. The process ends when stdin closes, and on a
  # loaded machine that can happen before the thread has got as far as failing:
  # asserting the thread's line made this check fail once on a macOS runner and
  # pass everywhere else, which is a race in the check, not a defect it found.
  # What the release promises is that the handshake does not wait for the model
  # and that stderr says so, and that is what this asserts.
  ok=1
  [ "$(grep -c '"result"' "$work/mcp-airgap.out")" = "2" ] || {
    echo "expected 2 responses, got $(grep -c '"result"' "$work/mcp-airgap.out"):"
    sed 's/^/    /' "$work/mcp-airgap.out" | head -4
    ok=0
  }
  grep -q '"tools"' "$work/mcp-airgap.out" || { echo "tools/list carried no tools"; ok=0; }
  grep -q 'loading the embedding model in the background' "$work/mcp-airgap.err" || {
    echo "stderr did not say the model was loading behind the handshake:"
    sed 's/^/    /' "$work/mcp-airgap.err" | head -6
    ok=0
  }
  [ "$ok" = 1 ]
}
check cli/mcp/handshake-no-model "initialize answers with no model" c_mcp_handshake_without_model

# A fresh install wired into a client used to be a server that exited on
# startup: the client reports no server, and nothing says why.
c_mcp_without_a_store() {
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
    | SEMLITH_HOME="$work/empty-home" semlith mcp 2> /dev/null > "$work/mcp-bare.out"
  [ "$(grep -c '"result"' "$work/mcp-bare.out")" = "2" ] &&
    grep -q '"tools"' "$work/mcp-bare.out"
}
check cli/mcp/no-store          "a store-less machine still serves" c_mcp_without_a_store

# `doctor` has to be usable from a script: a machine that is fine exits zero.
c_doctor_exit_code() {
  semlith doctor > "$work/doctor.out" 2>&1
  rc=$?
  # Either answer is legitimate on a runner; what is not legitimate is a
  # non-zero exit with nothing naming a fault, which is a doctor nobody can act
  # on.
  if [ "$rc" -eq 0 ]; then
    grep -q 'Clients' "$work/doctor.out"
  else
    grep -qE 'FAIL|not registered|OFF HERE|run:' "$work/doctor.out"
  fi
}
check cli/doctor/exit-code      "doctor exits with a reason"       c_doctor_exit_code

# ------------------------------------------------------------------- service

# A login service is the whole of "semlith is already there". It is installed
# and removed here rather than assumed: the three mechanisms share no code and
# only one of them is ever exercised on a given runner.
service_installed=0
c_service_install() {
  semlith start --service > "$work/service.out" 2>&1 || return 1
  grep -q 'login service' "$work/service.out" || return 1
  case "$platform" in
    macos)   launchctl list 2>/dev/null | grep -q com.semlith.daemon ;;
    linux)   systemctl --user is-enabled semlith.service 2>/dev/null | grep -q enabled ;;
    windows) schtasks //Query //TN semlith > /dev/null 2>&1 ;;
    *)       return 1 ;;
  esac
}
if semlith start --service > "$work/service-probe.out" 2>&1 &&
   ! grep -q 'does not report it as installed' "$work/service-probe.out"; then
  service_installed=1
  check cli/service/install "the login service installs"   c_service_install
elif grep -q 'does not report it as installed' "$work/service-probe.out" 2>/dev/null; then
  # The definition was written and the service manager will not own it. A
  # headless macOS session has no `gui/<uid>` domain and a container has no
  # systemd user session; in both the daemon can be started but not supervised.
  # semlith says so rather than claiming a login service it has not got, and
  # this is that answer, not a defect to fail the product for.
  sed 's/^/           /' "$work/service-probe.out" | head -4
  skip cli/service/install "this session has no domain a login service can live in"
  semlith start --no-service > /dev/null 2>&1 || true
else
  # A runner with no user session has no systemd --user and no launchd GUI
  # domain. That is the runner's shape, not semlith's defect, and blaming the
  # product for it is what rule 3 at the top of this file forbids.
  skip cli/service/install "$(head -1 "$work/service-probe.out" 2>/dev/null)"
fi

# Whether this session has a facility that can restart a daemon nobody is
# watching, and what that facility is.
#
# Issue #104. A GitHub macOS runner has no Aqua login session, so `launchctl`
# loads a user agent, runs it once, and never keeps it alive: the plist is on
# disk, `launchctl print` finds the label, and nothing restarts it. The same
# shape on Linux is a container with no `systemd --user` manager, where
# `enable --now` reports success and starts nothing.
#
# That was carried in known-failures.txt as `cli/service/recovers macos 104`,
# which is worse than it sounds: a row in that file turns a check that cannot
# run into a check that ran and failed as expected, and the run prints `xfail`
# either way. Somebody reading the log cannot tell the runner's shape from a
# regression in the product. So the probe is here, its answer is printed, and
# the run itself says which facility was missing.
supervision_facility() {
  case "$platform" in
    macos)
      # Two probes for this were written for 0.22.0 and both were disproved on
      # the real runner. `launchctl print gui/<uid>` answers there, so the check
      # ran and failed. `launchctl managername` returns "Aqua" there too, so it
      # ran and failed again. The GitHub-hosted macOS runner presents every
      # property a real login session has and still will not bring a killed
      # agent back.
      #
      # So the host is named instead of probed. That is not a proxy for the
      # facility — it is the one host where the facility's own answers are
      # known to be wrong, and there is no third property to ask. #104 is closed
      # as won't-fix on exactly this: measured working in about two seconds on
      # real hardware, and not demonstrable on a GitHub-hosted runner. The check
      # still runs everywhere else, including a self-hosted macOS runner and a
      # developer's laptop, which is where a regression in it would matter.
      name=$(launchctl managername 2>/dev/null || echo unknown)
      if [ "${RUNNER_ENVIRONMENT:-}" = "github-hosted" ]; then
        echo "launchd: GitHub-hosted macOS runner — reports \"$name\" and still does not keep a user agent alive; not demonstrable here (#104, won't-fix)"
      elif [ "$name" = "Aqua" ]; then
        echo "launchd: Aqua session, KeepAlive can work"
      else
        echo "launchd: session manager is \"$name\", not Aqua — launchd runs a user agent once here and does not keep it alive"
      fi
      ;;
    linux)
      if systemctl --user show-environment > /dev/null 2>&1; then
        echo "systemd --user: present"
      else
        echo "systemd --user: absent — no user manager is running for this uid, so a unit enables and never starts"
      fi
      ;;
    windows)
      echo "Windows logon task: restarts a task that failed, and does not supervise one that exited cleanly"
      ;;
    *)
      echo "no supervision facility is known for $platform"
      ;;
  esac
}

# Zero when this session can actually keep a daemon alive.
supervision_present() {
  case "$platform" in
    macos)
      # A GitHub-hosted runner answers "Aqua" and supervises nothing, so it is
      # excluded by name. See `supervision_facility` and #104.
      [ "${RUNNER_ENVIRONMENT:-}" != "github-hosted" ] &&
        [ "$(launchctl managername 2>/dev/null)" = "Aqua" ]
      ;;
    linux) systemctl --user show-environment > /dev/null 2>&1 ;;
    *)     return 1 ;;
  esac
}

# The service exists so that a daemon which dies comes back with nobody
# watching. macOS KeepAlive and systemd Restart=always do that; a Windows logon
# task restarts a task that failed and does not supervise one that exited, which
# the product says out loud rather than claiming parity.
c_service_recovers() {
  pid=$(pgrep -f 'semlith start' | head -1)
  [ -n "$pid" ] || { echo "nothing to kill: no 'semlith start' is running"; return 1; }
  kill -9 "$pid" 2>/dev/null
  # Sixty seconds, not forty: launchd throttles a job that exited within ten
  # seconds of starting, and this kills one that has just started.
  i=0
  while [ "$i" -lt 30 ]; do
    sleep 2
    back=$(pgrep -f 'semlith start' | head -1)
    if [ -n "$back" ] && [ "$back" != "$pid" ]; then
      echo "restarted: $pid -> $back after $((i * 2 + 2))s"
      return 0
    fi
    i=$((i + 1))
  done
  # Captured, not summarised. "It did not come back" is not something anybody
  # can act on, and this check runs where nobody can attach a debugger.
  echo "killed $pid; nothing came back in 60s"
  case "$platform" in
    macos)
      launchctl print "gui/$(id -u)/com.semlith.daemon" 2>&1 |
        grep -iE 'state|pid|last exit|runs|throttle|program|path' | sed 's/^/    /' | head -12
      ;;
    linux)
      systemctl --user status semlith.service 2>&1 | sed 's/^/    /' | head -12
      ;;
  esac
  echo "    port 7365: $(curl -s -o /dev/null -w '%{http_code}' -m 3 http://127.0.0.1:7365/ || echo unreachable)"
  log="${SEMLITH_HOME:-$HOME/.semlith}/logs/daemon.log"
  [ -f "$log" ] && { echo "    --- $log ---"; tail -15 "$log" | sed 's/^/    /'; }
  return 1
}
# Registered is not running. `systemctl --user enable --now` reports success and
# leaves the unit enabled-but-not-running wherever the user session cannot run
# one — a container, a runner, a machine with no lingering — and there is
# nothing for a restart check to restart. That is the session's shape, not
# semlith's defect, so it is a skip that says why rather than a failure that
# does not.
service_running=0
if [ "$service_installed" = "1" ]; then
  i=0
  while [ $i -lt 15 ]; do
    if pgrep -f 'semlith start' > /dev/null 2>&1; then service_running=1; break; fi
    sleep 2
    i=$((i + 1))
  done
fi

echo "           supervision: $(supervision_facility)"
if [ "$service_running" = "1" ] && [ "$platform" != "windows" ] && supervision_present; then
  check cli/service/recovers "a killed daemon is restarted"  c_service_recovers
elif [ "$platform" = "windows" ]; then
  skip cli/service/recovers "a logon task does not supervise a clean exit"
elif ! supervision_present; then
  # Not a defect and not an expected failure: the facility the check needs is
  # not present, and the line above names it.
  skip cli/service/recovers "$(supervision_facility)"
elif [ "$service_installed" = "1" ]; then
  # Captured, not summarised: the next person reading this log needs the
  # service manager's own words about why nothing started.
  case "$platform" in
    linux) systemctl --user status semlith.service 2>&1 | sed 's/^/           /' | head -12 ;;
    macos) launchctl print "gui/$(id -u)/com.semlith.daemon" 2>&1 | sed 's/^/           /' | head -12 ;;
  esac
  skip cli/service/recovers "the service is registered but no daemon started in this session"
else
  skip cli/service/recovers "the service did not install"
fi

# Removing is the rollback, so it has to work and has to be safe twice.
c_service_remove() {
  semlith start --no-service > "$work/unservice.out" 2>&1 || return 1
  grep -q 'removed' "$work/unservice.out" || return 1
  # Second time: still exits zero, and says there was nothing there.
  semlith start --no-service > "$work/unservice2.out" 2>&1 || return 1
  grep -q 'no login service' "$work/unservice2.out"
}
if [ "$service_installed" = "1" ]; then
  check cli/service/remove "the service removes, twice"      c_service_remove
else
  skip cli/service/remove "the service did not install"
fi

# Nothing of this release may be left running on the runner — and removing the
# service is not enough. A service that is removed leaves the daemon it started
# running, deliberately: `--no-service` is documented as not touching one. So
# the daemon is stopped here too, or the next run of this harness meets a port
# it cannot bind and blames the product for it.
semlith start --no-service > /dev/null 2>&1 || true
pkill -f 'semlith start' 2>/dev/null || true

# ------------------------------------------------- the real home, afterwards

# Issue #106, proven rather than asserted. Every check above has run, including
# `semlith setup`, and not one byte of the developer's own client configuration
# may have moved. `real_home` is empty only where the environment had no HOME to
# begin with, and there is then nothing to protect.
c_home_untouched() {
  if [ -z "$real_home" ]; then
    echo "no HOME was set when this run started; there is nothing to compare"
    return 0
  fi
  after=$(client_config_fingerprint)
  if [ "$after" = "$home_before" ]; then
    return 0
  fi
  echo "the run changed a client configuration under the real home ($real_home):"
  printf '%s\n' "$home_before" > "$work/home-before.txt"
  printf '%s\n' "$after" > "$work/home-after.txt"
  diff "$work/home-before.txt" "$work/home-after.txt" | sed 's/^/    /' | head -20
  return 1
}

check cli/harness/leaves-the-real-home-alone \
  "the run writes nothing under the developer's own home" c_home_untouched

# ------------------------------------------------------------------- summary

echo
echo "--------------------------------------------------------------"
printf 'pass %d   fail %d   xfail %d   XPASS %d   skip %d\n' \
  "$passes" "$fails" "$xfails" "$xpasses" "$skips"
if [ -n "$failed_ids" ]; then
  echo "needs attention:$failed_ids"
fi
rm -rf "$work" 2>/dev/null
exit $((fails + xpasses))
