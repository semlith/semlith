TBD
===
Unreleased changes. Release notes have not yet been written.


15.2.0 (2026-07-15)
===================
This release fixes a number of bugs related to gitignore matching as well as
some performance improvements in directory tree traversal.

Platform support:

* `aarch64-unknown-linux-musl` is now included in the release binaries for
  ripgrep.

Performance improvements:

* [PERF #3293](https://github.com/BurntSushi/ripgrep/issues/3293):
  Improves directory traversal time on very large corpora.

Feature enhancements:

* [FEATURE #3275](https://github.com/BurntSushi/ripgrep/issues/3275):
  ripgrep now respects `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM`.

Bug fixes:

* [BUG #3212](https://github.com/BurntSushi/ripgrep/pull/3212):
  Don't check for the existence of `.jj` when `--no-ignore` is used.
* [BUG #3320](https://github.com/BurntSushi/ripgrep/pull/3419),
  [BUG #3376](https://github.com/BurntSushi/ripgrep/pull/3376),
  [BUG #3419](https://github.com/BurntSushi/ripgrep/pull/3419):
  Fix gitignore matching bug when searching across multiple directories.


15.1.0 (2025-10-22)
===================
This is a small release that fixes a bug with how ripgrep handles line
buffering. This might manifest as ripgrep printing output later than you
expect or not working correctly with `tail -f` (even if you're using the
`--line-buffered` flag).

Bug fixes:

* [BUG #3194](https://github.com/BurntSushi/ripgrep/issues/3194):
  Fix a regression with `--line-buffered` introduced in ripgrep 15.0.0.

Feature enhancements:

* [FEATURE #3192](https://github.com/BurntSushi/ripgrep/pull/3192):
  Add hyperlink alias for Cursor.


15.0.0 (2025-10-15)
===================
ripgrep 15 is a new major version release of ripgrep that mostly has bug fixes,
  Fix bug where `--passthru` and `-A/-B/-C` did not override each other.
* [BUG #1869](https://github.com/BurntSushi/ripgrep/pull/1869):
  Clarify docs for `--files-with-matches` and `--files-without-match`.
* [BUG #1878](https://github.com/BurntSushi/ripgrep/issues/1878):
  Fix bug where `\A` could produce unanchored matches in multiline search.
* [BUG 94e4b8e3](https://github.com/BurntSushi/ripgrep/commit/94e4b8e3):
  Fix column numbers with `--vimgrep` is used with `-U/--multiline`.


12.1.1 (2020-05-29)
===================
ripgrep 12.1.1 is a patch release that fixes a couple small bugs. In
particular, the ripgrep 12.1.0 release did not tag new releases for all of its
in-tree dependencies. As a result, ripgrep built dependencies from crates.io
would produce a different build than compiling ripgrep from source on the
`12.1.0` tag. Namely, some crates like `grep-cli` had unreleased changes.

Bug fixes:

* [BUG #1581](https://github.com/BurntSushi/ripgrep/issues/1581):
  Corrects some egregious markup output in `--help`.
