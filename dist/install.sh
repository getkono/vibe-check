#!/usr/bin/env bash
# Resolve, download, and verify the pinned binary.
set -euo pipefail

# GITHUB_ACTION_PATH is a Windows path on windows-latest. Git Bash tolerates it
# as an argument but not in every construction, so normalize it once.
ACTION_PATH="$(cygpath -u "$GITHUB_ACTION_PATH" 2>/dev/null || printf '%s' "$GITHUB_ACTION_PATH")"

summary() { printf '%s\n' "$*" >> "$GITHUB_STEP_SUMMARY"; }

# --- the air-gapped path ----------------------------------------------------
# Environment, not an input, so it does not touch the frozen surface. The digest
# is deliberately NOT consulted: the operator supplied the bytes and owns their
# provenance. The summary says so, rather than implying a check that did not run.
if [ -n "${VIBE_CHECK_BINARY:-}" ]; then
  if [ ! -x "$VIBE_CHECK_BINARY" ]; then
    echo "::error::VIBE_CHECK_BINARY=${VIBE_CHECK_BINARY} is not an executable file"
    exit 1
  fi
  echo "path=${VIBE_CHECK_BINARY}" >> "$GITHUB_OUTPUT"
  summary "### vibe-check"
  summary ""
  summary "Binary supplied by \`VIBE_CHECK_BINARY\` (\`${VIBE_CHECK_BINARY}\`)."
  summary "The committed digest was **not** consulted."
  exit 0
fi

# --- what this ref of the action is pinned to -------------------------------
# Read from the ACTION's own tree at the ref the caller used, which is the whole
# point: a digest fetched from the same release as the binary moves with the
# asset and verifies transport and nothing else.
lock="${ACTION_PATH}/dist/RELEASE"
sums="${ACTION_PATH}/dist/SHA256SUMS"
if [ ! -f "$lock" ] || [ ! -f "$sums" ]; then
  echo "::error::the ref '${ACTION_REF:-?}' of ${ACTION_REPO:-getkono/vibe-check} carries no pinned release."
  cat >&2 <<'EOF'
dist/RELEASE and dist/SHA256SUMS are written by the release workflow onto the
commit that the action tags point at. A branch ref (@master), or a source
release tag (@binaries-v0.1.0), will not have them.

Use an action tag:  uses: getkono/vibe-check@v0.1.0
EOF
  exit 1
fi
tag="$(awk '$1=="tag"{print $2}' "$lock")"
version="$(awk '$1=="version"{print $2}' "$lock")"
if [ -z "$tag" ] || [ -z "$version" ]; then
  echo "::error::dist/RELEASE is malformed"
  exit 1
fi

# --- which archive this runner needs ----------------------------------------
case "${RUNNER_OS}/${RUNNER_ARCH}" in
  Linux/X64) target=x86_64-unknown-linux-musl ;;
  Linux/ARM64) target=aarch64-unknown-linux-musl ;;
  macOS/X64) target=x86_64-apple-darwin ;;
  macOS/ARM64) target=aarch64-apple-darwin ;;
  Windows/X64) target=x86_64-pc-windows-msvc ;;
  *)
    echo "::error::no vibe-check binary for ${RUNNER_OS}/${RUNNER_ARCH}."
    echo "Set VIBE_CHECK_BINARY to a locally built binary on this runner." >&2
    exit 1
    ;;
esac
asset="vibe-check-${version}-${target}.tar.gz"

expected="$(awk -v a="$asset" '$2==a{print $1}' "$sums")"
if [ -z "$expected" ]; then
  echo "::error::${asset} has no committed digest in dist/SHA256SUMS at this ref"
  exit 1
fi

# --- download, anonymously --------------------------------------------------
# No Authorization header. The release is public, so a token buys nothing and
# spends the caller's rate limit in a repository whose other jobs need it.
#
# Retry with backoff, because this is an unpinned network dependency inside
# someone else's CI and a slow CDN must not fail their pull request. `--fail`
# throughout, so an HTML error page is never mistaken for a tarball.
url="https://github.com/getkono/vibe-check/releases/download/${tag}/${asset}"
dest="${RUNNER_TEMP}/${asset}"
ok=false
for attempt in 1 2 3 4 5; do
  if curl -fsSL --connect-timeout 10 --max-time 300 -o "$dest" "$url"; then
    ok=true
    break
  fi
  delay=$(( attempt * attempt * 3 ))
  echo "download attempt ${attempt} failed; retrying in ${delay}s" >&2
  sleep "$delay"
done
if [ "$ok" != true ]; then
  # Exit 1, never 0. Reusing 0 for "we could not tell" makes every outage look
  # like a clean bill of health, which is the failure this tool exists to
  # remove from other people's pipelines.
  echo "::error::could not download ${url} after 5 attempts"
  exit 1
fi

# --- verify against the COMMITTED digest ------------------------------------
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$dest" | cut -d' ' -f1)"
else
  actual="$(shasum -a 256 "$dest" | cut -d' ' -f1)"
fi
if [ "$actual" != "$expected" ]; then
  echo "::error::digest mismatch for ${asset}"
  {
    echo "  expected (committed at ${ACTION_REF:-this ref}): ${expected}"
    echo "  actual   (downloaded from ${tag}):               ${actual}"
    echo "The release asset does not match what this ref of the action was"
    echo "published against. Do not re-run; report it."
  } >&2
  exit 1
fi

# --- unpack -----------------------------------------------------------------
ext=""
if [ "$RUNNER_OS" = "Windows" ]; then ext=".exe"; fi
stage="${RUNNER_TEMP}/vibe-check-${version}"
mkdir -p "$stage"
tar -xzf "$dest" -C "$stage" --strip-components=1
bin="${stage}/vibe-check${ext}"
chmod +x "$bin" 2>/dev/null || true
"$bin" --version

echo "path=${bin}" >> "$GITHUB_OUTPUT"

# The provenance block, written by the ACTION rather than the binary — so it is
# present and useful even when the binary itself fails, which is what makes it
# the observable for the distribution chain's own verification.
summary "### vibe-check"
summary ""
summary "| | |"
summary "|---|---|"
summary "| version | \`${version}\` |"
summary "| release | \`${tag}\` |"
summary "| target | \`${target}\` |"
summary "| sha256 | \`${actual}\` — matches the digest committed at \`${ACTION_REF:-?}\` |"
summary ""
