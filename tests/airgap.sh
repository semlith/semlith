#!/usr/bin/env bash
# Air-gap gate: a local check on the M1 with Docker, never run in CI.
#
#   tests/airgap.sh <path or URL to a semlith Linux release archive>
#
# Seeds a model cache in a connected container with `semlith setup
# --all-models`, then indexes and searches a small corpus (code, Markdown, one
# PNG) in --network none containers with that cache mounted read-only: with
# SEMLITH_AIRGAP=1, without it, and with the CLIP text encoder removed. Prints
# one PASS or FAIL line per step and exits non-zero if any step failed. Pick
# the archive whose architecture matches the Docker host (aarch64 on the M1).
#
# AIRGAP_IMAGE overrides the base image (default ubuntu:24.04). AIRGAP_KEEP=1
# keeps the work directory, with every step's stdout and stderr, and prints
# where it is.
set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <path or URL to a semlith Linux release archive>" >&2
    exit 2
fi
archive=$1
image=${AIRGAP_IMAGE:-ubuntu:24.04}

work=$(mktemp -d "${TMPDIR:-/tmp}/semlith-airgap.XXXXXX")
# shellcheck disable=SC2329 # invoked by the trap below
cleanup() {
    if [[ ${AIRGAP_KEEP:-} == 1 ]]; then
        echo "kept $work" >&2
    else
        rm -r "$work"
    fi
}
trap cleanup EXIT

failed=0
pass() { echo "PASS $1"; }
fail() {
    echo "FAIL $1: $2"
    failed=1
}

# --- the release ------------------------------------------------------------
mkdir -p "$work/rel"
if [[ $archive =~ ^https?:// ]]; then
    curl -fsSL -o "$work/archive.tar.gz" "$archive"
    archive=$work/archive.tar.gz
fi
tar xzf "$archive" -C "$work/rel"
bin=$(find "$work/rel" -type f -name semlith -print -quit)
if [[ -z $bin ]]; then
    echo "no semlith binary in $1" >&2
    exit 2
fi
bindir=$(dirname "$bin")

# --- the corpus -------------------------------------------------------------
corpus=$work/corpus
mkdir -p "$corpus/src" "$corpus/assets"
cat >"$corpus/src/config.rs" <<'EOF'
use std::collections::HashMap;

/// Parse a config file of `key = value` lines into a map, skipping comments.
pub fn parse_config(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}
EOF
cat >"$corpus/README.md" <<'EOF'
# Lantern

Lantern reads its settings from a config file, parsed by `parse_config` in
src/config.rs. The project logo, a red square with a white circle, is in
assets/logo.png.
EOF
# 64x64 red square with a white circle.
base64 --decode >"$corpus/assets/logo.png" <<'EOF'
iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAe0lEQVR42u3YUQ3AMAgFwEqpfzVzxBQsWdslA3IvTwD3B4xrztIdAAAAAAAAAAAAAAAAAAAA3zWekx0Q75IUECtJB4j1JALEblIA4iwAv05/bgAAAAAAALBKFAeU30Y73AMdLrION3GHr4THFgAAAAAAAAAAAAAAAECZ3kFLSr4NrXfeAAAAAElFTkSuQmCC
EOF

cache=$work/cache
store=$work/store
out=$work/out
mkdir -p "$cache" "$store" "$out"

# run <name> <cache dir> <docker args...> -- <semlith args...>
# Runs semlith once in a fresh container and leaves its exit code, stdout,
# stderr and wall time (measured inside the container, so container start-up
# is not counted) in $out/<name>.{rc,stdout,stderr,secs}. Stdin is passed on.
run() {
    local name=$1 models=$2
    shift 2
    local docker_args=()
    while [[ $1 != -- ]]; do
        docker_args+=("$1")
        shift
    done
    shift
    local rc=0
    docker run --rm -i \
        --user "$(id -u):$(id -g)" -e HOME=/tmp \
        -e SEMLITH_MODEL_CACHE=/models -e SEMLITH_NO_SERVICE=1 \
        -v "$bindir:/opt/semlith:ro" -v "$corpus:/corpus:ro" -v "$store:/store" \
        -v "$out:/out" -e STEP="$name" \
        ${docker_args[@]+"${docker_args[@]}"} -v "$models:/models${ro:-}" \
        "$image" bash -c '
            start=$(date +%s.%N)
            /opt/semlith/semlith "$@"
            rc=$?
            echo "$start $(date +%s.%N)" | awk "{ printf \"%.2f\", \$2 - \$1 }" >"/out/$STEP.secs"
            exit $rc' semlith "$@" >"$out/$name.stdout" 2>"$out/$name.stderr" || rc=$?
    echo "$rc" >"$out/$name.rc"
    return "$rc"
}

# A step's first error line (the head of its chain), else its last stderr
# line, for a FAIL reason.
why() { { grep -m1 -i '^error' "$out/$1.stderr" || tail -n 1 "$out/$1.stderr"; } | cut -c1-300; }

airgap=(--network none -e SEMLITH_AIRGAP=1)
offline=(--network none)
text_query="how is the config file parsed"
pictured_query="the red logo"

# --- seed (connected) ---------------------------------------------------------
setup=(setup --yes --no-service --no-hooks --no-agents)
if run seed "$cache" -- "${setup[@]}" --all-models </dev/null; then
    pass seed
else
    fail seed "exit $(cat "$out/seed.rc"): $(why seed)"
    # The 0.48.0 user flow, so the remaining steps still show what it does:
    # setup fetches the embedding model, indexing an image fetches CLIP.
    if run seed-fallback "$cache" -- "${setup[@]}" </dev/null &&
        run seed-fallback-index "$cache" -- --store /tmp/s index /corpus -q --no-review </dev/null; then
        pass "seed-fallback (setup --yes, then index once, connected)"
    else
        fail seed-fallback "$(why seed-fallback-index 2>/dev/null || why seed-fallback)"
        exit 1
    fi
fi

ro=:ro

# --- air-gapped, SEMLITH_AIRGAP=1 -------------------------------------------
if run airgap-index "$cache" "${airgap[@]}" -- --store /store/s index /corpus -q --no-review </dev/null &&
    run airgap-files "$cache" "${airgap[@]}" -- --store /store/s files </dev/null &&
    grep -q 'logo.png' "$out/airgap-files.stdout"; then
    pass airgap-index
else
    fail airgap-index "$(why airgap-index)"
fi

# check_query <step> <expect in stdout> <docker args...> -- <query>
check_query() {
    local step=$1 expect=$2 models=$3
    shift 3
    if run "$step" "$models" "$@" </dev/null && grep -q "$expect" "$out/$step.stdout"; then
        return 0
    fi
    if [[ $(cat "$out/$step.rc") != 0 ]]; then
        fail "$step" "exit $(cat "$out/$step.rc") after $(cat "$out/$step.secs") s: $(why "$step")"
    else
        fail "$step" "no $expect in the results ($(cat "$out/$step.secs") s)"
    fi
    return 1
}

# A pictured query searched its images only if it printed no skip note:
# logo.png alone also matches the README text hit that names the file.
# check_pictured <step> <docker args...> -- <query>
check_pictured() {
    local step=$1
    check_query "$@" || return 1
    if grep -q 'image results skipped' "$out/$step.stderr"; then
        fail "$step" "image results skipped: $(grep -m1 'image results skipped' "$out/$step.stderr" | cut -c1-200)"
        return 1
    fi
}

check_query airgap-text-query config.rs "$cache" "${airgap[@]}" -- \
    --store /store/s search "$text_query" --local && pass airgap-text-query
check_pictured airgap-pictured-query logo.png "$cache" "${airgap[@]}" -- \
    --store /store/s search "$pictured_query" --local && pass airgap-pictured-query

# A 2026-07-28 client: no initialize, the version and client on the request.
mcp_call='{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"semlith_search","arguments":{"query":"'$text_query'","k":5},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"airgap-sh","version":"0"}}}}'
if run airgap-mcp "$cache" "${airgap[@]}" -- --store /store/s mcp <<<"$mcp_call" &&
    grep -q 'config.rs' "$out/airgap-mcp.stdout" &&
    ! grep -q '"isError":true' "$out/airgap-mcp.stdout"; then
    pass airgap-mcp
else
    fail airgap-mcp "exit $(cat "$out/airgap-mcp.rc"): $(head -c 300 "$out/airgap-mcp.stdout") $(why airgap-mcp)"
fi

# --- offline, SEMLITH_AIRGAP unset ------------------------------------------
check_query offline-text-query config.rs "$cache" "${offline[@]}" -- \
    --store /store/s search "$text_query" --local && pass offline-text-query
# The contract's risk: every offline pictured query retries the fetch, and
# over 2 s brings remembering the failure into scope. Every query also pays
# for loading the embedding model, so the cost of the fetch attempt is the
# time over the offline text query.
if check_pictured offline-pictured-query logo.png "$cache" "${offline[@]}" -- \
    --store /store/s search "$pictured_query" --local; then
    secs=$(cat "$out/offline-pictured-query.secs")
    extra=$(awk -v p="$secs" -v t="$(cat "$out/offline-text-query.secs")" 'BEGIN { printf "%.2f", p - t }')
    pass "offline-pictured-query ($secs s, $extra s over the text query)"
fi

# --- CLIP text encoder removed, SEMLITH_AIRGAP=1 ----------------------------
nocache=$work/cache-no-text
# A clone on APFS costs no space; elsewhere a plain copy. -R keeps the hub
# cache's relative symlinks as symlinks.
if ! cp -Rc "$cache" "$nocache" 2>/dev/null; then
    [[ -e $nocache ]] && rm -r "$nocache"
    cp -R "$cache" "$nocache"
fi
find "$nocache" -maxdepth 3 -type d -name 'models--Qdrant--clip-ViT-B-32-text' -prune -exec rm -r {} +
step=no-text-encoder-pictured-query
if run "$step" "$nocache" "${airgap[@]}" -- --store /store/s search "$pictured_query" --local </dev/null; then
    if ! grep -qE 'config.rs|README.md' "$out/$step.stdout"; then
        fail "$step" "no text hits"
    elif ! grep -q 'image results skipped' "$out/$step.stderr"; then
        fail "$step" "no \"image results skipped\" note on stderr"
    else
        pass "$step"
    fi
else
    fail "$step" "exit $(cat "$out/$step.rc"): $(why "$step")"
fi

exit "$failed"
