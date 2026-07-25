# What this changes

<!-- What is different after this PR, and why. If it fixes an issue, "Fixes #123". -->

# Why

<!-- What was wrong before. What tradeoff you took. What you deliberately left out. -->

# How it was verified

<!--
Which check proves this works? A bug fix should come with a test that would
have failed before. If you changed indexing or search behaviour, include what
you measured — numbers beat adjectives.
-->

# Checklist

- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] `cargo test -- --ignored` passes — **required if this touches `lib.rs`, `store.rs`, or `chunk.rs`**
- [ ] `CHANGELOG.md` updated under `## [Unreleased]`
- [ ] Documentation updated if behaviour changed
