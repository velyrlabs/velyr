#!/bin/sh
# Installs the velyr binary from GitHub Releases and verifies its SHA256 checksum.
# Env: VELYR_INSTALL (default ~/.velyr/bin), VELYR_VERSION (tag, default latest stable release),
#      VELYR_DOWNLOAD_URL (directory URL holding the artifacts; overrides the GitHub URL),
#      VELYR_TARGET (Rust target triple; overrides detection).
set -eu

# TODO: replace with the real GitHub repository (OWNER/REPO) before the first release.
VELYR_REPO="OWNER/REPO"

die() { echo "install.sh: $*" >&2; exit 1; }

detect_target() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported architecture: $arch" ;;
  esac
  case "$os" in
    Darwin) echo "$arch-apple-darwin" ;;
    Linux)
      libc=gnu
      if ls /lib/ld-musl-* >/dev/null 2>&1; then libc=musl; fi
      echo "$arch-unknown-linux-$libc"
      ;;
    *) die "unsupported OS: $os (Windows: download the .zip from the release page)" ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    die "need sha256sum or shasum"
  fi
}

command -v curl >/dev/null 2>&1 || die "curl is required"

target=${VELYR_TARGET:-$(detect_target)}
install_dir=${VELYR_INSTALL:-$HOME/.velyr/bin}
file="velyr-$target.tar.gz"

if [ -n "${VELYR_DOWNLOAD_URL:-}" ]; then
  base=$VELYR_DOWNLOAD_URL
elif [ -n "${VELYR_VERSION:-}" ]; then
  base="https://github.com/$VELYR_REPO/releases/download/$VELYR_VERSION"
else
  base="https://github.com/$VELYR_REPO/releases/latest/download"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "Downloading $file"
curl -fsSL "$base/$file" -o "$tmp/$file" || die "download failed: $base/$file"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || die "download failed: $base/SHA256SUMS"

expected=$(awk -v f="$file" '{n=$2; sub(/^\*/, "", n)} n==f {print $1}' "$tmp/SHA256SUMS")
[ -n "$expected" ] || die "no checksum for $file in SHA256SUMS"
actual=$(sha256_of "$tmp/$file")
[ "$expected" = "$actual" ] || die "checksum mismatch for $file (expected $expected, got $actual)"

tar -xzf "$tmp/$file" -C "$tmp" velyr || die "archive does not contain velyr"
mkdir -p "$install_dir"
cp "$tmp/velyr" "$install_dir/velyr"
chmod 755 "$install_dir/velyr"

echo "Installed velyr to $install_dir/velyr"
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) echo "$install_dir is not in your shell PATH; add it to run velyr directly." ;;
esac
