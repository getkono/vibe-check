#!/usr/bin/env bash
# NOT -e: the exit code is the product here.
set -uo pipefail

if [ -n "$INPUT_CONFIG_FILE" ] && [ -n "$INPUT_CONFIG_INLINE" ]; then
  echo "::error::config-file and config-inline are mutually exclusive."
  exit 1
fi

args=()
if [ -n "$INPUT_CONFIG_FILE" ]; then
  args+=(--config "$INPUT_CONFIG_FILE")
elif [ -n "$INPUT_CONFIG_INLINE" ]; then
  inline="${RUNNER_TEMP}/vibe-check-policy.toml"
  printf '%s\n' "$INPUT_CONFIG_INLINE" > "$inline"
  args+=(--config "$inline")
fi
# Neither set: the binary's own default, .vibe-check/policy.toml.

out="${RUNNER_TEMP}/vibe-check.json"
echo "verdict-file=${out}" >> "$GITHUB_OUTPUT"

# One invocation, not two. `--format json` goes to stdout and is redirected
# here; the step summary is written by the binary itself, which reads
# GITHUB_STEP_SUMMARY from the environment. Two invocations could disagree,
# which is exactly what "one function, three renderings" exists to prevent.
# Diagnostics are on stderr, so stdout stays machine-readable at any RUST_LOG.
# `|| exit` matters here specifically: this script deliberately runs without
# `set -e`, because the binary's exit code is its product. Without the guard a
# failed `cd` would classify whatever directory the runner happened to be in.
cd "$GITHUB_WORKSPACE" || exit 1
"$VIBE_CHECK_BIN" classify --format json "${args[@]}" > "$out"
code=$?

# The exit table is crates/vibe-check/src/exit.rs, and it is a public interface.
# At this milestone `classify` decides nothing, so any non-zero is a failure of
# the tool rather than a verdict, and the step fails. When the observe/advisory/
# enforcing modes land, `observe` maps 10 and 20 to a passing step and only
# 1 / 2 / 101 stay fatal — that change belongs there, not here.
case "$code" in
  0) echo "vibe-check classified the change." ;;
  10) echo "::warning::interface-review (exit 10)" ;;
  20) echo "::warning::human review required (exit 20)" ;;
  1) echo "::error::vibe-check could not produce a verdict (exit 1)" ;;
  2) echo "::error::vibe-check rejected its command line (exit 2) — this is a bug in the action" ;;
  101) echo "::error::vibe-check came apart before it started (exit 101)" ;;
  *) echo "::error::vibe-check exited ${code}, which its documented table does not describe" ;;
esac
exit "$code"
