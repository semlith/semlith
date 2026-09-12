#!/bin/sh
# Install semlith from a GitHub release: download this platform's binary, verify
# it against SHA256SUMS, then hand off to `semlith setup`.
# SEMLITH_VERSION=v0.10.0  pin a release tag (default: the latest release)
# SEMLITH_HOME=<dir>       install into <dir>/bin (default: ~/.semlith/bin)
# SEMLITH_YES=1            answer yes to every `semlith setup` prompt
# SEMLITH_RELEASES_ORIGIN  where releases are fetched from; for the tests
set -eu
repo=semlith/semlith
origin=${SEMLITH_RELEASES_ORIGIN:-https://github.com}
say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

arch=$(uname -m)
case "$arch" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) die "unsupported architecture: $arch" ;;
esac

os=$(uname -s)
case "$os" in
  Linux) target="$arch-unknown-linux-gnu" ;;
  Darwin)
    if [ "$arch" != aarch64 ]; then
      printf '%s\n' \
        "semlith has no prebuilt binary for Intel (x86_64) macOS." \
        "ONNX Runtime stopped publishing osx-x86_64 builds, so semlith's" \
        "embedding runtime cannot be linked for that target and the release" \
        "workflow does not produce one." \
        "" \
        "Build it from source instead:" \
        "    cargo install semlith" >&2
      exit 1
    fi
    target=aarch64-apple-darwin
    ;;
  *) die "unsupported operating system: $os" ;;
esac

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fSL --progress-bar -o "$2" "$1"; }
  final_url() { curl -fsSLI -o /dev/null -w '%{url_effective}' "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q --show-progress -O "$2" "$1"; }
  final_url() {
    wget -q -S --spider --max-redirect=10 "$1" 2>&1 |
      sed -n 's/^[[:space:]]*Location:[[:space:]]*//p' | tail -n 1
  }
else
  die "need curl or wget to download semlith"
fi

if command -v sha256sum >/dev/null 2>&1; then
  check_sums() { sha256sum -c "$1"; }
elif command -v shasum >/dev/null 2>&1; then
  check_sums() { shasum -a 256 -c "$1"; }
else
  die "need sha256sum or shasum to verify the download"
fi

if [ -n "${SEMLITH_VERSION:-}" ]; then
  tag=$SEMLITH_VERSION
else
  say "Resolving the latest semlith release..."
  tag=$(final_url "$origin/$repo/releases/latest")
  tag=${tag##*/}
fi
case "$tag" in
  v*) ;;
  *) die "could not resolve a release tag (got '$tag')" ;;
esac

name="semlith-$tag-$target"
archive="$name.tar.gz"
base="$origin/$repo/releases/download/$tag"
say "Installing semlith $tag for $target"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading $archive"
fetch "$base/$archive" "$tmp/$archive"
say "Downloading SHA256SUMS"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"

say "Verifying checksum"
grep "  $archive\$" "$tmp/SHA256SUMS" >"$tmp/expected" ||
  die "SHA256SUMS has no entry for $archive"
(cd "$tmp" && check_sums expected) ||
  die "checksum mismatch: $archive does not match its SHA256SUMS entry; nothing was installed"

say "Unpacking"
tar xzf "$tmp/$archive" -C "$tmp"

bin_dir="${SEMLITH_HOME:-$HOME/.semlith}/bin"
mkdir -p "$bin_dir"
mv "$tmp/$name/semlith" "$bin_dir/semlith"
chmod +x "$bin_dir/semlith"
say "Installed $bin_dir/semlith"

if "$bin_dir/semlith" setup --help >/dev/null 2>&1; then
  if [ "${SEMLITH_YES:-}" = 1 ]; then
    "$bin_dir/semlith" setup --yes
  elif [ -t 0 ]; then
    "$bin_dir/semlith" setup
  elif [ -r /dev/tty ]; then
    # `curl | sh` leaves stdin on the pipe, so the guided flow reads the
    # terminal rather than every pasted install silently taking the defaults.
    "$bin_dir/semlith" setup </dev/tty
  else
    "$bin_dir/semlith" setup --yes
  fi
else
  say ""
  say "This release has no 'setup' command. Add semlith to your PATH by putting"
  say "this line in your shell rc file (~/.zshrc, ~/.bashrc or ~/.profile):"
  say "    export PATH=\"$bin_dir:\$PATH\""
fi
