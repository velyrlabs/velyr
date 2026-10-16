#!/usr/bin/env bash
# Fetches the web-platform-tests/wpt commit pinned in compat/wpt.lock into compat/vendor/wpt/,
# as a sparse checkout of the directories velyr-wpt can run (ADR-015). Idempotent.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
lock="$root/compat/wpt.lock"
dest="$root/compat/vendor/wpt"

sha="$(sed -n 's/^sha=//p' "$lock")"
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || { echo "invalid sha in $lock" >&2; exit 1; }

# resources/ and common/ hold testharness.js and the helpers `META: script=` pulls in.
dirs=(
  resources common
  url encoding streams fetch/api FileAPI
  dom/events dom/abort WebCryptoAPI
  html/webappapis/timers html/webappapis/structured-clone
  websockets
)

if [[ -d "$dest/.git" && "$(git -C "$dest" rev-parse HEAD 2>/dev/null)" == "$sha" ]]; then
  echo "wpt $sha already present"
  exit 0
fi

mkdir -p "$dest"
[[ -d "$dest/.git" ]] || git -C "$dest" init -q
git -C "$dest" remote get-url origin >/dev/null 2>&1 ||
  git -C "$dest" remote add origin https://github.com/web-platform-tests/wpt.git
git -C "$dest" sparse-checkout set --cone "${dirs[@]}"
git -C "$dest" fetch -q --depth 1 --filter=blob:none origin "$sha"
git -C "$dest" checkout -q --detach FETCH_HEAD
echo "wpt $sha fetched to $dest"
