#!/bin/sh
# Install semlith from a GitHub release: download this platform's binary, verify
# it against SHA256SUMS, then hand off to `semlith setup`.
# SEMLITH_VERSION=v0.10.0  pin a release tag (default: the latest release)
# SEMLITH_HOME=<dir>       install into <dir>/bin (default: ~/.semlith/bin)
# SEMLITH_YES=1            answer yes to every `semlith setup` prompt
set -eu
repo=semlith/semlith
# One origin, over HTTPS, with no way to be told another. A script that took
# its origin from the environment would install whatever a hostile shell
# profile pointed it at, on a machine where the user typed `curl … | sh`.
# `tests/install.rs` rewrites this line in a copy of the script rather than
# setting a variable, so what it exercises is what ships.
origin=https://github.com
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

# A progress bar drawn from a size we already know.
#
# curl's own `--progress-bar` is not usable here. It draws a bouncing `#=O=-`
# indicator for as long as it does not know the transfer size, and over HTTPS
# that is the whole connect, TLS and header phase: measured against the release
# asset, 72 of 100 frames were that indicator and only the last 28 were a bar,
# with no redirects involved at all. It reads as line noise rather than as
# progress, which is worse than showing nothing.
bar() {
  [ "$2" -gt 0 ] 2>/dev/null || return 0
  pct=$(( $1 * 100 / $2 ))
  [ "$pct" -gt 100 ] && pct=100
  fill=$(( pct * 36 / 100 ))
  printf '\r  %-36s %3d%%' "$(printf "%${fill}s" '' | tr ' ' '#')" "$pct"
}

if command -v curl >/dev/null 2>&1; then
  fetch() {
    # The last Content-Length in the header chain, so a redirect is followed
    # first and the size is the one the body will actually have.
    size=$(curl -fsSLI "$1" 2>/dev/null | tr -d '\r' \
      | awk 'tolower($1)=="content-length:"{n=$2} END{print n+0}')
    curl -fsSL -o "$2" "$1" &
    dl=$!
    while kill -0 "$dl" 2>/dev/null; do
      # Guarded because the file does not exist until curl creates it, and the
      # shell reports a failed redirection itself — `wc`'s own stderr is not
      # what would leak here.
      have=0
      [ -f "$2" ] && have=$(wc -c < "$2" 2>/dev/null || echo 0)
      bar "$have" "${size:-0}"
      sleep 0.2
    done
    # The download's exit status is the function's: a failed fetch has to fail
    # the install, and a bar that reached 100% must not hide a truncated body.
    if wait "$dl"; then
      bar "${size:-0}" "${size:-0}"
      [ "${size:-0}" -gt 0 ] && printf '\n'
      return 0
    fi
    printf '\n'
    return 1
  }
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
  elif (: </dev/tty) 2>/dev/null; then
    # `curl | sh` leaves stdin on the pipe, so read the terminal instead of
    # silently taking defaults. Opening it is the test: in a container
    # /dev/tty exists and passes -r but cannot be opened.
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
