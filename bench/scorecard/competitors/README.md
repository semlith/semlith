# Competitor arms

One adapter per tool for the retrieval scorecard (SWE-bench Lite, retrieval
only). Each module exposes `NAME`, `VERSION`, `available()`, `version()`,
`index(root, work)` and `search(query, root, work, k=50)`. `search` returns
`(path relative to root, start_line, end_line, text)` in the order the tool
ranks them. `ALL` in `__init__.py` maps the key to its module. Python 3.11,
stdlib only. Every adapter calls the tool's own CLI or its documented
MCP/HTTP interface with a timeout: 2 h for an index, 120 s for a search.

    cd bench/scorecard && python3.11 -m competitors.smoke [key ...]

`smoke.py` indexes `tools/testrepo` (psf/requests) and runs one query:
"Session.request ignores timeout when proxies set".

## Containment

- Everything is installed under `~/semlith-bench/scorecard/tools/` (`TOOLS`,
  which `$SCORECARD_TOOLS` overrides). Each subprocess gets `HOME`, `HF_HOME`
  and `XDG_*` pointed there by `_common.env()`. Sourcebot is the exception:
  it uses the real `HOME`, because the docker CLI needs `~/.docker`.
- Nothing is written into the repository under test. ck, grepai and graphify
  keep their index inside the project, so they index an `rsync -a --delete
  --exclude=.git` mirror at `work/mirror`. rsync keeps mtimes, so after a
  `git checkout` only the changed files look changed. Serena's `.serena`
  folder moves out through `tools/serena-home/serena_config.yml`. Sourcebot
  mounts the bench directory read-only.
- `work` is `tools/work/<key>/<repo>`. Serena is the one exception: its
  project folder is `tools/work/serena/<basename of root>/.serena`.

## Query construction (`_common.py`)

- **identifiers**: snake_case, dotted.names, camelCase/CapsWords, backticked
  code, and `name(` call sites, kept in order of appearance.
- **words**: 4+-letter words, lowercased, minus a stopword list (English plus
  issue boilerplate).
- **grep terms** = the identifiers, then the words. rg, ugrep and Sourcebot
  OR these terms together.

## Tools

Index times are wall seconds on testrepo (psf/requests, 130 tracked files) on
the M1. Cold means an empty work dir with the models already downloaded.
"After checkout" means after `git checkout HEAD~50`, a step that changes 34
files.
Load average was 80-127 during these runs (rustc builds from other sessions),
so treat the times as upper bounds.

| key | tool | version | official source | install | cold index s | after checkout s | incremental |
|---|---|---|---|---|---|---|---|
| rg | ripgrep | 15.2.0 | github.com/BurntSushi/ripgrep | preinstalled (Homebrew) | 0 | 0 | n/a (index-free) |
| ugrep | ugrep | 7.8.5 | github.com/Genivia/ugrep | source build | 0 | 0 | n/a (index-free) |
| colgrep | ColGrep | 1.7.0 | github.com/lightonai/next-plaid | release binary | 240.5 | 191.5 | yes (no-change re-run: 0 s) |
| semble | Semble | 0.6.1 | github.com/MinishLab/semble | PyPI in venv | 22.3 | 4.0 | yes |
| ck | ck | 0.7.11 | github.com/BeaconBay/ck | release binary | 953.5 | 141.4 | yes |
| grepai | grepai | 0.37.0 | github.com/yoanbernabeu/grepai | release binary + Ollama 0.35.1 | 152.7 | 109.0 | yes (no-change re-run: 0.8 s) |
| serena | Serena | 1.7.0 | github.com/oraios/serena | git tag in venv | 47.5 | 9.6 | yes (LSP symbol cache) |
| graphify | Graphify | 0.9.74 | github.com/safishamsi/graphify | PyPI `graphifyy` in venv | 15.3 | 5.7 | yes (per-file cache) |
| sourcebot | Sourcebot | 5.1.15 | github.com/sourcebot-dev/sourcebot | Docker Compose | 31.1 | 19.0 | no: Zoekt rebuilds the repo's shard; the time is mostly two 15 s re-index cycles |

Search seconds on the smoke query, warm: rg 0.14, ugrep 0.18, ColGrep 3.9,
Semble 1.5, ck 1.0, grepai 0.3, Serena 7.9 (the first call starts the MCP
server and the LSP), Graphify 0.9, Sourcebot 0.5.

Top 5 files on the smoke query. The fix for this issue would edit
`src/requests/sessions.py`.

| key | top 5 |
|---|---|
| rg | test_requests.py, docs/user/advanced.rst, sessions.py, HISTORY.md, adapters.py |
| ugrep | identical to rg |
| colgrep | sessions.py, test_requests.py, api.py, utils.py, adapters.py |
| semble | sessions.py, adapters.py, utils.py, api.py, _types.py |
| ck | test_requests.py, api.py, docs/user/quickstart.rst, docs/user/advanced.rst, HISTORY.md |
| grepai | api.py, sessions.py, adapters.py, utils.py, tests/testserver/server.py |
| serena | sessions.py (symbol `Session/request`), HISTORY.md (only 2 results) |
| graphify | exceptions.py, sessions.py, models.py, utils.py, cookies.py |
| sourcebot | models.py, sessions.py, exceptions.py, api.py, test_requests.py |

### rg — ripgrep
- Install: already present (`/opt/homebrew/bin/rg`, Homebrew).
- Query: grep terms as `-i -F -e <term>` patterns. Files are ranked by
  `--count-matches`, and the top k each give one result: matched lines with
  3 lines of context, merged into blocks (text capped at 4000 chars).
- No index.

### ugrep
- Source: https://github.com/Genivia/ugrep, tag v7.8.5 source tarball.
- Install: `./configure --prefix=tools/ugrep && make -j8 && make install`.
  There is no macOS binary release; brew would install globally.
- Query: the same shape as rg (`-r -I --ignore-files --exclude-dir=.git -i -F
  -e ...`), ranked by `-c -u -m1,` match counts. `--bool`/`-%` is not used: in
  that mode a space means AND, so a long issue would match nothing. OR-ed
  `-e` patterns are what `--bool 'a|b|c'` means.
- No index.

### colgrep — ColGrep (LightOn)
- Source: https://github.com/lightonai/next-plaid, release v1.7.0, asset
  `colgrep-aarch64-apple-darwin.tar.xz`. This is the asset the official
  `colgrep-installer.sh` downloads.
- Install: extract into `tools/colgrep/`. On first init it downloads the
  model `lightonai/LateOn-Code-edge` (about 100 MB) into `tools/cache/hf`.
- Query: the raw issue text: `colgrep --json -k K "<issue>" <root>`. Its
  default mode fuses ColBERT with an FTS5 trigram pass using RRF.
- Index: `colgrep init -y <root>`, with `COLGREP_DATA_DIR=work` so the index
  stays out of `~/Library/Application Support`. Incremental: it re-encodes
  only added and changed files.
- Limitation: the JSON `line`/`end_line` are often whole-file spans (1..EOF).
  They are passed through as returned. `text` is the unit's code.

### semble — Semble (MinishLab)
- Source: https://github.com/MinishLab/semble, PyPI `semble` 0.6.1.
- Install: `uv venv -p 3.11 tools/venv-semble && uv pip install -p tools/venv-semble semble`.
  The docs say `uv tool install semble`, which installs globally.
- Query: the raw issue text: `semble search -k K --format json "<issue>" <root>`,
  with the default content type (code). Model: potion-code-16M plus BM25.
- Index: there is no index command. The first search builds and caches the
  index (`SEMBLE_CACHE_LOCATION=work`), and `index()` runs a throwaway query to
  build it. Later searches re-index changed files.

### ck
- Source: https://github.com/BeaconBay/ck, release 0.7.11, asset
  `ck-0.7.11-aarch64-apple-darwin.tar.gz`. The documented route is
  `cargo install ck-search`; this is the same version, prebuilt.
- Query: the raw issue text: `ck --sem --jsonl --topk K --threshold 0 "<issue>" .`
  The docs recommend `--jsonl --sem` for natural-language queries. `--hybrid`
  adds a regex pass, and a whole issue used as a regex matches nothing.
  `--threshold 0` stops ck's 0.6 default score floor from cutting the top-k.
- Index: `ck --index .` on the mirror (it writes `.ck/` next to the code).
  The embedding model is BGE-Small (fastembed), cached in `tools/cache`.
  Incremental: a chunk-level embedding cache, so only changed chunks are
  re-embedded.

### grepai
- Source: https://github.com/yoanbernabeu/grepai, release v0.37.0, asset
  `grepai_0.37.0_darwin_arm64.tar.gz`. This is what `install.sh` fetches.
- Embeddings: grepai's default provider, Ollama, with `nomic-embed-text`. The
  only other local option is the LM Studio GUI; no API account is used.
  Ollama 0.35.1 comes from https://github.com/ollama/ollama
  (`ollama-darwin.tgz`), unpacked into `tools/ollama`, models in
  `tools/cache/ollama-models`, serving on 127.0.0.1:11434 (grepai's default).
  `index()` and `search()` start it and pull the model if needed.
- Query: the raw issue text: `grepai search --json -n K "<issue>"` in the mirror.
  Its default test-path penalties stay on.
- Index: `grepai init --provider ollama --backend gob --yes` once, then
  `grepai watch --no-ui` until it prints "Watching for changes", then SIGINT.
  grepai has no one-shot index command. Incremental: the scan skips unchanged
  files and drops deleted ones.
- Limitation: it can return fewer than K results (35 of 50 on testrepo).

### serena — Serena (oraios)
- Source: https://github.com/oraios/serena, tag v1.7.0. The MCP server reports
  itself as 1.28.1.
- Install: `uv venv -p 3.11 tools/venv-serena && uv pip install -p tools/venv-serena "serena-agent @ git+https://github.com/oraios/serena@v1.7.0"`.
  `SERENA_HOME=tools/serena-home`. Its `serena_config.yml` is the packaged
  template with `project_serena_folder_location` moved under tools/work and
  the dashboard and GUI log turned off.
- Interface: MCP over stdio (`serena start-mcp-server --project <root>
  --transport stdio --context agent`), one long-lived server per root, which
  `index()` restarts.
- Query: Serena has no NL search. For each identifier-like term (at most 15),
  `find_symbol` runs with its name path (`a.b` becomes `a/b`), and those
  symbol hits come first. Then `search_for_pattern` runs with the identifiers
  OR-ed as escaped regex, and its files are appended, ranked by matching-line
  count. An issue with no identifiers falls back to its words for the pattern
  search.
- Index: `serena project index <root>` (the LSP symbol cache). It
  auto-creates the project with the detected language server (Python: the
  bundled one).
- Limitation: an issue with one identifier yields few results (2 on the
  smoke query).

### graphify — Graphify
- Source: https://github.com/safishamsi/graphify, PyPI `graphifyy` 0.9.74.
- Install: `uv venv -p 3.11 tools/venv-graphify && uv pip install -p tools/venv-graphify graphifyy`.
- Index: `graphify update . --no-cluster` on the mirror. This is the
  tree-sitter code pass: no LLM, and the communities a query does not need are
  skipped. `GRAPHIFY_FORCE=1` lets an older checkout overwrite a larger graph.
  It re-extracts code files, with a per-file cache in `graphify-out/cache`.
- Query: `graphify query "<issue>" --budget max(2000, 40*K)`, a BFS from the
  nodes the question names. `NODE` lines come back in printed order as
  `(src, line, line, label)`.
- Hazard: every graphify run rewrites `~/.claude/skills/graphify` unless
  `GRAPHIFY_NO_AUTO_REFRESH=1` is set. `_common.env()` sets it.

### sourcebot — Sourcebot
- Source: https://github.com/sourcebot-dev/sourcebot. The image is
  `ghcr.io/sourcebot-dev/sourcebot:v5.1.15`, plus `postgres:16` and `redis:8`.
- Install: `tools/sourcebot/docker-compose.yml` is the official compose file
  with these changes: version pinned, bind mounts under `tools/sourcebot`,
  only 127.0.0.1:3070 exposed, the bench dir mounted read-only at its host
  path, `FORCE_ENABLE_ANONYMOUS_ACCESS=true` (so the API needs no account),
  and telemetry off. `AUTH_SECRET` and `SOURCEBOT_ENCRYPTION_KEY` were
  generated locally into `tools/sourcebot/.env`. Run it with
  `docker compose -p scorecard-sourcebot up -d`, which `index()` does on
  demand. Config: `tools/sourcebot/data/config.json` (reindexIntervalMs 15 s).
- Index: Sourcebot indexes a local repo's default branch, never a detached
  HEAD ("Failed to get local default branch"), and never fetches. So
  `index()` keeps a `git clone --no-checkout --no-hardlinks` of the root in
  `work/git`, fetches the root's HEAD into it, and moves branch `scorecard`
  there. It then waits for two completed re-index runs after the move. A
  `--shared` clone indexes nothing, because zoekt's go-git ignores alternates.
  Incremental: Zoekt rebuilds the repo's shard.
- Query: grep terms, each quoted as a literal keyword, OR-ed and scoped with
  `repo:^<name>$`, sent to `POST /api/search` (`matches` 5000,
  `contextLines` 3). Files come back in Zoekt rank order.
- Limitations: each repo costs a full copy of its `.git` under work. The
  container has no outbound network, so its licensing ping fails; that is
  harmless.
