#!/usr/bin/env bash
# Tests for scripts/ci/check-deps.sh and scripts/ci/check-unsafe.sh using temp fixtures.
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ci="$(dirname "$here")"
repo="$(cd "$ci/../.." && pwd)"
tmp="$(mktemp -d)"
# Removes only the temp dir created above.
trap 'rm -r -f -- "$tmp"' EXIT
fail=0

expect() { # expect <0|nonzero> <label> <cmd...>
  local want="$1" label="$2"; shift 2
  "$@" >/dev/null 2>&1; local rc=$?
  if { [ "$want" = 0 ] && [ $rc -eq 0 ]; } || { [ "$want" != 0 ] && [ $rc -ne 0 ]; }; then
    echo "ok   $label"
  else
    echo "FAIL $label (rc=$rc)"; fail=1
  fi
}

mk_crate() { # mk_crate <root> <dir> <pkg> <extra-toml>
  mkdir -p "$1/crates/$2/src"
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2024"\n%s\n' "$3" "$4" >"$1/crates/$2/Cargo.toml"
  : >"$1/crates/$2/src/lib.rs"
}

mk_ws() { # mk_ws <root>
  mkdir -p "$1"
  printf '[workspace]\nmembers = ["crates/*"]\nresolver = "3"\n' >"$1/Cargo.toml"
  printf 'engine-v8\n' >"$1/unsafe-allowlist.txt"
}

# --- real repo ---
expect 0 "check-deps passes on repo" bash "$ci/check-deps.sh" "$repo"
expect 0 "check-unsafe passes on repo" bash "$ci/check-unsafe.sh" "$repo"

# --- check-unsafe fixtures ---
ok="$tmp/unsafe-ok"; mk_ws "$ok"
mk_crate "$ok" a velyr-a $'[lints]\nworkspace = true'
mk_crate "$ok" engine-v8 velyr-engine-v8 $'[lints.rust]\nunsafe_code = "warn"'
expect 0 "check-unsafe: allowlisted crate may override" bash "$ci/check-unsafe.sh" "$ok"
expect 0 "check-unsafe: root via env" env VELYR_ROOT="$ok" bash "$ci/check-unsafe.sh"

bad="$tmp/unsafe-bad"; mk_ws "$bad"
mk_crate "$bad" a velyr-a $'[lints]\nworkspace = true'
mk_crate "$bad" b velyr-b ''
expect 1 "check-unsafe: missing [lints] fails" bash "$ci/check-unsafe.sh" "$bad"

bad2="$tmp/unsafe-bad2"; mk_ws "$bad2"
mk_crate "$bad2" c velyr-c $'[lints.rust]\nunsafe_code = "allow"'
expect 1 "check-unsafe: own lints without workspace=true fails" bash "$ci/check-unsafe.sh" "$bad2"

# --- check-deps fixtures ---
good="$tmp/deps-ok"; mk_ws "$good"
mk_crate "$good" runtime-core velyr-runtime-core $'[dependencies]\nvelyr-engine-api = { path = "../engine-api" }'
mk_crate "$good" engine-api velyr-engine-api ''
expect 0 "check-deps: clean graph passes" bash "$ci/check-deps.sh" "$good"

for forbidden in web compat tooling io; do
  d="$tmp/deps-$forbidden"; mk_ws "$d"
  mk_crate "$d" runtime-core velyr-runtime-core "$(printf '[dependencies]\nvelyr-%s-x = { path = "../%s-x" }' "$forbidden" "$forbidden")"
  mk_crate "$d" "$forbidden-x" "velyr-$forbidden-x" ''
  expect 1 "check-deps: velyr-$forbidden- dependency fails" bash "$ci/check-deps.sh" "$d"
done

t="$tmp/deps-transitive"; mk_ws "$t"
mk_crate "$t" runtime-core velyr-runtime-core $'[dependencies]\nvelyr-engine-api = { path = "../engine-api" }'
mk_crate "$t" engine-api velyr-engine-api $'[dependencies]\nvelyr-web-x = { path = "../web-x" }'
mk_crate "$t" web-x velyr-web-x ''
expect 1 "check-deps: transitive velyr-web- fails" bash "$ci/check-deps.sh" "$t"

# dev-dependencies must not count (-e normal)
dv="$tmp/deps-dev"; mk_ws "$dv"
mk_crate "$dv" runtime-core velyr-runtime-core $'[dev-dependencies]\nvelyr-web-x = { path = "../web-x" }'
mk_crate "$dv" web-x velyr-web-x ''
expect 0 "check-deps: dev-dependency ignored" bash "$ci/check-deps.sh" "$dv"

exit $fail
