#!/usr/bin/env bash
# The executable half of four claims that are otherwise only prose.
#
# actionlint lints workflow files only: it has no mode for composite action
# metadata and rejects action.yml outright as a malformed workflow. So the
# action's own guarantees have to be asserted here.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
status=0
fail() { echo "lint-surface: $*" >&2; status=1; }

# 1. The frozen input surface. A third input is a major version, not a tweak,
#    and the reasoning is in action.yml's own header.
inputs="$(awk '/^inputs:/{f=1;next} /^[a-z]/{f=0} f && /^  [a-z][a-z-]*:/{gsub(/[ :]/,"");print}' action.yml)"
expected="config-file
config-inline"
if [ "$inputs" != "$expected" ]; then
  fail "action.yml's inputs are not the frozen pair."
  echo "  expected: $(echo "$expected" | tr '\n' ' ')" >&2
  echo "  found:    $(echo "$inputs" | tr '\n' ' ')" >&2
fi

# 2. release-binaries.yml triggers on release-plz.yml's `name:`. Renaming that
#    workflow otherwise silently disables the entire binary chain, with no error
#    anywhere — the same class of silent failure the workflow_run design exists
#    to avoid.
producer="$(awk '/^name:/{sub(/^name:[ ]*/,"");print;exit}' .github/workflows/release-plz.yml)"
if ! grep -q "workflows: \[\"${producer}\"\]" .github/workflows/release-binaries.yml; then
  fail "release-binaries.yml does not trigger on release-plz.yml's name (\"${producer}\")."
fi

# 3. dist/RELEASE and dist/SHA256SUMS agree with each other.
#
#    Both are absent until the first promotion, and `mise run check` has to keep
#    passing in between — so absence is fine and disagreement is not.
if [ -f dist/RELEASE ] || [ -f dist/SHA256SUMS ]; then
  if [ ! -f dist/RELEASE ] || [ ! -f dist/SHA256SUMS ]; then
    fail "dist/RELEASE and dist/SHA256SUMS must exist together or not at all."
  else
    version="$(awk '$1=="version"{print $2}' dist/RELEASE)"
    [ -n "$version" ] || fail "dist/RELEASE names no version."
    count=0
    while read -r _ name; do
      [ -n "$name" ] || continue
      count=$((count + 1))
      case "$name" in
        "vibe-check-${version}-"*.tar.gz) ;;
        *) fail "dist/SHA256SUMS lists ${name}, which is not version ${version}." ;;
      esac
    done < dist/SHA256SUMS
    if [ "$count" -ne 5 ]; then
      fail "dist/SHA256SUMS lists ${count} archives; the release matrix builds 5."
    fi
  fi
fi

# 4. Every `[[package]]` in release-plz.toml names a package that exists.
#
#    A name here that matches no package is a hard config error inside
#    release-plz, raised on master after the merge — and nothing else in
#    `mise run check` reads this file, so a rename that misses it is discovered
#    only once the release chain is already broken. That is the failure class
#    #96 and #99 exist to remove.
#
#    Read from the manifests rather than from `cargo metadata`, because this
#    script is otherwise pure text linting and needs neither a toolchain nor a
#    JSON parser to run. The two sets are the same set: `Cargo.toml` declares
#    `members = ["crates/*"]`, so every workspace package is exactly one
#    `crates/*/Cargo.toml`.
manifest_names="$(for manifest in crates/*/Cargo.toml; do
  awk -F'"' '
    /^\[package\]/ { p = 1; next }
    /^\[/          { p = 0 }
    p && $0 ~ /^name[[:space:]]*=/ { print $2; exit }
  ' "$manifest"
done)"
if [ -z "$manifest_names" ]; then
  fail "no package name could be read from any crates/*/Cargo.toml."
else
  while read -r declared; do
    [ -n "$declared" ] || continue
    printf '%s\n' "$manifest_names" | grep -qxF "$declared" ||
      fail "release-plz.toml names package ${declared}, which no crates/*/Cargo.toml declares."
  done <<EOF
$(awk -F'"' '
  /^\[\[package\]\]/ { p = 1; next }
  /^\[/                { p = 0 }
  p && $0 ~ /^name[[:space:]]*=/ { print $2 }
' release-plz.toml)
EOF
fi

exit "$status"
