# Contributing to semlith

How to build semlith, which checks a change must pass, and how to send it. The
bar for a change is that it makes the tool better at its job: finding the right
excerpt, fast, locally. [AGENTS.md](AGENTS.md) holds the full build, test and
architecture notes; this page is the short version.

By participating you agree to abide by the [Code of Conduct](CODE_OF_CONDUCT.md).

## Build

You need a 64-bit machine and Rust 1.90 or newer (turbovec refuses 32-bit
targets). Nothing else has to be installed first: there is no BLAS or OpenSSL
dependency.

```sh
git clone https://github.com/semlith/semlith
cd semlith
cargo build
cargo test
```

The first build compiles SQLite and downloads ONNX Runtime, so it takes a few
minutes.

## Checks

CI runs these on Linux, macOS and Windows. Run them before opening a pull
request:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI also runs `msrv`, which builds on the Rust version `Cargo.toml` declares, and
`scripts`, which shellchecks `install.sh` and lints `install.ps1`.

Most integration tests are `#[ignore]`d because they download the ~52 MB
embedding model. Run them when your change touches what they cover:

```sh
cargo test -- --ignored                                        # everything, incl. the round trip
cargo test --test mcp -- --ignored                             # one file
cargo test --release --test measure -- --ignored --nocapture   # performance claims
cargo test --release --test shards -- --ignored --nocapture
cargo test --release --test retrieval -- --ignored --nocapture # retrieval quality
```

**Run `cargo test -- --ignored` if you touch `lib.rs`, `store.rs` or
`chunk.rs`.** Its round trip (index, search, edit, prune, reopen, forget) is the
test that catches chunk ids in the vector index drifting out of sync with their
rows in SQLite. `measure` and `shards` assert real thresholds and need
`--release`; a debug build fails them.

## Code layout

| File | Responsibility |
|---|---|
| `src/lib.rs` | The `Semlith` type: opening a store, indexing, searching, persistence |
| `src/store.rs` | Every SQL statement; nothing else touches the database |
| `src/chunk.rs` | Reading a file into text and splitting it into chunks |
| `src/mcp.rs` | The MCP server |
| `src/main.rs` | CLI argument parsing and output formatting |

[docs/architecture.md](docs/architecture.md) explains how the pieces fit
together.

## House style

- **`cargo fmt` decides formatting.**
- **Comments explain why, not what.** If a line needs a comment to say what it
  does, rename something. Worth writing: why a batch size is 32, why a hash is
  written after the index and not before.
- **Prefer deleting to adding.** The smaller diff that solves the problem wins.
- **Errors a user can act on.** `bail!("store was built with X, not Y")`, not
  `bail!("model mismatch")`.
- **Deliberate shortcuts get a `ponytail:` comment** naming the ceiling and the
  upgrade path.

## Commits and pull requests

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add hybrid keyword and vector search
fix: evict old chunk ids before re-adding a changed file
docs: record measured indexing throughput
perf: cap embed batch size to keep attention memory bounded
```

The subject says what changed; the body, when there is one, says why.

Open pull requests against `develop`; `main` receives releases.

- One logical change per PR.
- Include the test that proves it: a bug fix without a test that would have
  failed before tends to come back.
- If you changed indexing or search behaviour, put what you measured in the PR
  body. Numbers, not adjectives.
- Update `CHANGELOG.md` under `## [Unreleased]`.
- A change to a public surface (command, flag, environment variable, MCP tool,
  store format) also lands in [docs/compatibility.md](docs/compatibility.md).

## Performance work

Measure first and put the numbers in the PR:

```sh
# Indexing throughput and peak memory
/usr/bin/time -l ./target/release/semlith --store /tmp/bench index <corpus> --quiet

# Warm query latency — start the MCP server, feed it N searches, divide
./target/release/semlith --store /tmp/bench mcp < requests.jsonl > /dev/null
```

Peak memory matters as much as speed: semlith runs on a laptop beside other
work, and an embedding batch that is too large pushes the machine into swap and
looks like a hang. Published figures and their commands are in
[docs/performance.md](docs/performance.md).

### The 100k-chunk benchmark store

Building a hundred-thousand-chunk store takes over an hour of embedding, so
build it once and keep it at `~/.cache/semlith/bench/100k-store`. It is made
from crates already in the local cargo registry:

```sh
R=~/.cargo/registry/src/index.crates.io-*/
semlith -s ~/.cache/semlith/bench/100k-store index \
  $R/tracing-0.1.44 $R/rav1e-0.8.1 $R/rav1e-0.7.1 $R/ring-0.17.14 \
  $R/spdx-0.13.4 $R/web-sys-0.3.103 $R/umya-spreadsheet-3.0.1 \
  $R/libsqlite3-sys-0.38.1 $R/libsqlite3-sys-0.35.0 $R/libsqlite3-sys-0.30.1 \
  $R/linux-raw-sys-0.4.15 $R/chrono-tz-0.10.4 $R/winapi-0.3.9 --quiet
```

Rebuild it only when the corpus or the default model changes; re-running
`index` on an unchanged corpus takes seconds. Use it when a change is about
indexing or scan performance, not for every release.

## Discuss first

Open an issue before building any of these, because each changes what the tool
is:

- another bind address than `127.0.0.1`, or any new outbound connection;
- sending text to a hosted embedding API;
- a user-editable configuration file. Files such as `~/.semlith/registry.json`
  and `settings.json` are tool-written state, not configuration.

## Bugs and feature requests

Use the issue templates. For a bug, include the exact command and the output of
`semlith stats`. Security issues go through [SECURITY.md](SECURITY.md), not the
issue tracker.

## License

Contributions are licensed under the [Apache License 2.0](LICENSE), the same
license as the project.
