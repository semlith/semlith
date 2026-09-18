#!/usr/bin/env bash
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
echo "work     : $work"

command -v semlith >/dev/null || { echo "semlith is not on PATH"; exit 1; }
command -v jq >/dev/null || { echo "jq is not on PATH"; exit 1; }
echo "jq       : $(jq --version)"
echo

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
    [ $missing -ge 3 ] && break
  done < "$work/files.all"
  [ $missing -eq 0 ]
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

c_read_span() { semlith read "src/main.rs:28-40" | grep -q '.'; }
c_read_line() { semlith read "src/main.rs:30" | grep -q '.'; }
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

c_read_json() { semlith read "src/main.rs:28-40" --json | jq -e '.' > /dev/null; }

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
  [ $rc -eq 0 ] || [ $rc -eq 10 ] || { echo "upgrade --check exited $rc"; return 1; }
}
c_upgrade_airgap() {
  out=$(semlith upgrade --check --airgap 2>&1)
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  return 0
}

check cli/setup/idempotent     "setup re-runs cleanly"              c_setup_idempotent
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
  while [ $i -lt 90 ]; do
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
  while kill -0 "$wpid" 2>/dev/null && [ $i -lt 30 ]; do sleep 1; i=$((i + 1)); done
  if kill -0 "$wpid" 2>/dev/null; then
    kill -KILL "$wpid" 2>/dev/null
    wait "$wpid" 2>/dev/null
    echo "the watcher was still running 30s after SIGINT"
    sed 's/^/  /' "$wdir/watch.out"
    return 1
  fi
  wait "$wpid"
  rc=$?
  [ $rc -eq 0 ] ||
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
  # A second daemon must refuse, say why, and never quietly take another port:
  # the README calls the URL a bookmark. Whether it stops at the store lock or
  # at the bind, the requirement is the same.
  out=$(semlith start --port "$port" 2>&1)
  rc=$?
  echo "$out" | grep -qi 'panic' && { echo "panicked:"; echo "$out"; return 1; }
  [ $rc -ne 0 ] || { echo "a second daemon exited 0:"; echo "$out" | head -5; return 1; }
  [ -n "$out" ] || { echo "a second daemon refused with no message at all"; return 1; }
  # A refusal must not also announce a listener. POSIX grep has no lookahead,
  # so this asserts the simpler and stronger thing: it did not start at all.
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
  check cli/daemon/second-instance "a second daemon refuses clearly" c_daemon_second_instance
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
  # Two complete responses, and the reason searches will fail on stderr where a
  # stdio client captures it.
  [ "$(grep -c '"result"' "$work/mcp-airgap.out")" = "2" ] &&
    grep -q '"tools"' "$work/mcp-airgap.out" &&
    grep -q 'could not load the embedding model' "$work/mcp-airgap.err"
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
  if [ $rc -eq 0 ]; then
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
if semlith start --service > "$work/service-probe.out" 2>&1; then
  service_installed=1
  check cli/service/install "the login service installs"   c_service_install
else
  # A runner with no user session has no systemd --user and no launchd GUI
  # domain. That is the runner's shape, not semlith's defect, and blaming the
  # product for it is what rule 3 at the top of this file forbids.
  skip cli/service/install "$(head -1 "$work/service-probe.out" 2>/dev/null)"
fi

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
  while [ $i -lt 30 ]; do
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

if [ "$service_running" = "1" ] && [ "$platform" != "windows" ]; then
  check cli/service/recovers "a killed daemon is restarted"  c_service_recovers
elif [ "$platform" = "windows" ]; then
  skip cli/service/recovers "a logon task does not supervise a clean exit"
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
