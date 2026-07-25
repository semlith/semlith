# Contributing to semlith

Thanks for taking the time. semlith is a small, focused tool, and the bar for
a change is simply that it makes the tool better at its job: finding the right
excerpt, fast, locally.

By participating you agree to abide by our [Code of Conduct](CODE_OF_CONDUCT.md).

## Getting set up

You need a 64-bit machine and Rust 1.85 or newer. turbovec refuses to compile
on 32-bit targets by design.

**On Linux, install OpenBLAS first** — turbovec's build script emits
`-lopenblas` there. macOS uses Accelerate, which is part of the OS.

```sh
sudo apt-get install libopenblas-dev     # Debian/Ubuntu
sudo dnf install openblas-devel          # Fedora/RHEL
sudo pacman -S openblas                  # Arch
```

```sh
git clone https://github.com/semlith/semlith
cd semlith
cargo build
cargo test
```

The first build compiles SQLite and downloads ONNX Runtime binaries, so give it
a few minutes. Later builds are fast.

## The checks that must pass

CI runs exactly these on Linux and macOS. Run them before opening a pull
request and there will be no surprises:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

There is one more test that CI does not run, because it downloads a ~130 MB
embedding model:

```sh
cargo test -- --ignored
```

This is the end-to-end round trip — index, search, edit, prune, reopen, forget.
**Run it if you touch anything in `lib.rs`, `store.rs`, or `chunk.rs`.** It is
the test that catches chunk ids in the vector index drifting out of sync with
their rows in SQLite, which is the failure mode that matters most here.

## How the code is laid out

| File | Responsibility |
|---|---|
| `src/lib.rs` | The `Semlith` type: opening a store, indexing, searching, persistence |
| `src/store.rs` | Every SQL statement. Nothing else touches the database |
| `src/chunk.rs` | Reading a file into text and splitting it into chunks |
| `src/mcp.rs` | The stdio MCP server |
| `src/main.rs` | CLI argument parsing and output formatting |

See [docs/architecture.md](docs/architecture.md) for how the pieces fit
together and why the store is split across two files.

## House style

The codebase follows a few conventions. They are not arbitrary, and matching
them makes review quick:

- **`cargo fmt` decides formatting.** Do not fight it.
- **Comments explain why, not what.** If a line needs a comment to say what it
  does, rename something instead. The comments worth writing are the ones that
  save the next person an hour: why a batch size is 32, why a hash is written
  after the index and not before.
- **Prefer deleting to adding.** A smaller diff that solves the problem beats a
  larger one that also solves problems nobody has.
- **Errors that a user can act on.** `bail!("store was built with X, not Y")`
  beats `bail!("model mismatch")`.
- **Deliberate shortcuts get a `ponytail:` comment** naming the ceiling and the
  upgrade path, so the next person knows it was a choice and not an oversight.

## Commits and pull requests

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add hybrid keyword and vector search
fix: evict old chunk ids before re-adding a changed file
docs: record measured indexing throughput
perf: cap embed batch size to keep attention memory bounded
```

The subject line says what changed. The body, when there is one, says why —
what was wrong before, what tradeoff you took, what you deliberately left out.

For pull requests:

- One logical change per PR. Two unrelated fixes are two PRs.
- Include the check that proves it works. A bug fix without a test that would
  have failed before is a bug fix that comes back.
- If you changed indexing or search behaviour, say so in the PR body along with
  whatever you measured. Numbers beat adjectives.
- Update `CHANGELOG.md` under `## [Unreleased]`.

## Performance work

If you are optimizing, measure first and put the numbers in the PR. The two
that matter:

```sh
# Indexing throughput and peak memory
/usr/bin/time -l ./target/release/semlith --store /tmp/bench index <corpus> --quiet

# Warm query latency — start the MCP server, feed it N searches, divide
./target/release/semlith --store /tmp/bench mcp < requests.jsonl > /dev/null
```

Peak memory is a first-class concern. This tool is meant to run on a laptop
while other things are open, and an embedding batch that is too large will
quietly push a machine into swap and look like a hang.

## Things we are deliberately not doing

Please open an issue to discuss before building any of these — not because they
are bad ideas, but because they change what the tool is:

- A server mode, or anything that listens on a network port. Local means local.
- Sending text to a hosted embedding API. The whole point is that nothing
  leaves the machine.
- Configuration files. Flags and environment variables have been enough.

## Reporting bugs and asking for features

Use the issue templates. For a bug, the single most useful thing you can
include is the exact command and the output of `semlith stats`.

Security issues do **not** go in the issue tracker — see
[SECURITY.md](SECURITY.md).

## License

By contributing, you agree that your contributions will be licensed under the
[Apache License 2.0](LICENSE), the same license that covers the project.
