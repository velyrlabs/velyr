#!/usr/bin/env bash
# Fetches the tc39/test262 commit pinned in compat/test262.lock into compat/vendor/test262/.
# Idempotent. The first run records the tarball sha256 in the lock; later runs verify it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
lock="$root/compat/test262.lock"
dest="$root/compat/vendor/test262"

lock_value() { sed -n "s/^$1=//p" "$lock"; }
sha="$(lock_value sha)"
expected_sha256="$(lock_value sha256)"
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo "invalid sha in $lock" >&2; exit 1; }

if [[ -f "$dest/.velyr-sha" && "$(cat "$dest/.velyr-sha")" == "$sha" ]]; then
  echo "test262 $sha already present"
  exit 0
fi

# Extract into a fresh dir next to dest, so the final step is a same-filesystem rename.
mkdir -p "$root/compat/vendor"
tmp="$(mktemp -d "$root/compat/vendor/.fetch.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "https://codeload.github.com/tc39/test262/tar.gz/$sha" -o "$tmp/test262.tar.gz"

actual_sha256="$(shasum -a 256 "$tmp/test262.tar.gz" | cut -d' ' -f1)"
if [[ -z "$expected_sha256" ]]; then
  sed -i.bak "s/^sha256=.*/sha256=$actual_sha256/" "$lock" && rm -f "$lock.bak"
  echo "recorded sha256 $actual_sha256 in $lock"
elif [[ "$expected_sha256" != "$actual_sha256" ]]; then
  echo "sha256 mismatch: expected $expected_sha256, got $actual_sha256" >&2
  exit 1
fi

mkdir "$tmp/extract"
tar -xzf "$tmp/test262.tar.gz" -C "$tmp/extract" --strip-components=1
echo "$sha" > "$tmp/extract/.velyr-sha"
rm -rf "$dest"
mv "$tmp/extract" "$dest"
echo "test262 $sha fetched to $dest"
