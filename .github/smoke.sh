#!/usr/bin/env bash
# Drive every user-facing command against an installed semlith, on whatever
# OS this runs on. Bash on Windows is Git Bash, which the hosted runner has.
#
# Every step is a line in the log. A failure is recorded and the script keeps
# going, so one run shows every broken command rather than the first one.
# Exit status is the number of failures.

set -u

fails=0
step() {
  local name=$1
  shift
  echo
  echo "### $name"
  if "$@"; then
    echo "--- ok: $name"
  else
    echo "--- FAIL($?): $name"
    fails=$((fails + 1))
  fi
}

# The corpus: this repository's sources plus the format fixtures, so parsing,
# the code graph and the document extractors all get exercised.
corpus=$PWD

step "version" semlith --version
step "languages" semlith languages
step "models" semlith models

step "index" semlith index --quiet "$corpus/src" "$corpus/tests/fixtures"
step "index again is a no-op" semlith index --quiet "$corpus/src" "$corpus/tests/fixtures"
step "stats" semlith stats
step "files" sh -c 'semlith files | head -20'

step "search" semlith search "how does the store lock work" -k 3
step "search --json" sh -c 'semlith search "embedding model download" -k 2 --json | head -c 2000; echo'
step "search --lang" semlith search "parse a document" -k 2 --lang rust
step "read a span" semlith read src/main.rs:28-40
step "symbol" semlith symbol main
step "neighbors" semlith neighbors main
step "pattern" semlith pattern '(function_item name: (identifier) @name)' --lang rust

step "forget" semlith forget "$corpus/src/lock.rs"
step "re-index after forget" semlith index --quiet "$corpus/src"

# MCP over stdio: initialize, then list tools. The answer must name tools.
mcp_out=$(mktemp)
mcp_stdio() {
  printf '%s\n%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}' \
    '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
    | semlith mcp | tee "$mcp_out" | head -c 1500
  echo
  grep -q '"tools"' "$mcp_out"
}
step "mcp stdio" mcp_stdio

# The daemon: portal and HTTP MCP endpoint answer, key shows, ledger prints.
port=${SMOKE_PORT:-7365}
daemon() {
  semlith start --port "$port" &
  local pid=$!
  local up=0
  for _ in $(seq 1 60); do
    if curl -fsS -o /dev/null "http://127.0.0.1:$port/"; then up=1; break; fi
    sleep 1
  done
  local rc=0
  if [ $up = 1 ]; then
    curl -fsS -o /dev/null -w 'portal %{http_code}\n' "http://127.0.0.1:$port/" || rc=1
    semlith key show || rc=1
    curl -sS -o /dev/null -w 'mcp without key %{http_code}\n' -X POST "http://127.0.0.1:$port/mcp" \
      -H 'content-type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' || rc=1
    semlith search "store lock" -k 1 || rc=1
    semlith ledger --last 5 || rc=1
  else
    echo "daemon never answered on 7365"
    rc=1
  fi
  kill "$pid" 2>/dev/null
  wait "$pid" 2>/dev/null
  return $rc
}
step "daemon" daemon

step "setup is idempotent" semlith setup --yes --airgap
# Exit 10 means a newer release exists; both 0 and 10 are healthy here.
upgrade_check() { semlith upgrade --check; local rc=$?; [ $rc = 0 ] || [ $rc = 10 ]; }
step "upgrade --check" upgrade_check

echo
echo "### $fails failure(s)"
exit $fails
