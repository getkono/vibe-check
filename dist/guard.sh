#!/usr/bin/env bash
# Refusals that must happen before anything is downloaded or executed.
set -euo pipefail

# --- pull_request_target: refused, with no escape hatch ---------------------
#
# That trigger checks out the BASE branch but runs with a WRITE token and access
# to repository secrets, while evaluating a fork's code. vibe-check reads the
# head tree. There is no configuration that makes the combination safe, so there
# is no input for it and no environment variable for it: an escape hatch on this
# is the vulnerability, not a convenience.
#
# Use `pull_request`. If the reason you reached for `pull_request_target` was a
# token for a fork's pull request, that is a publishing problem — a job that
# never sees pull-request code — and not this one's.
if [ "${GITHUB_EVENT_NAME:-}" = "pull_request_target" ]; then
  echo "::error::vibe-check refuses to run on pull_request_target."
  {
    echo "That trigger grants a write token and repository secrets to a workflow"
    echo "evaluating fork-authored code. Use \`on: pull_request\` instead."
  } >&2
  exit 1
fi

# --- history: enough of it, or an actionable failure ------------------------
#
# vibe-check always compares against `git merge-base`, never the base branch tip
# recorded in the event payload, which drifts as the base branch moves. That
# needs real history.
#
# Deliberately NOT auto-fetching. An action that quietly unshallows hides a
# misconfiguration costing minutes on every run in the consumer's repository,
# and it would need credentials this action does not want to hold. One loud
# failure carrying the one-line fix is worth more than a silent tax.
# Asked of git rather than by looking for a `.git` directory: in a linked
# worktree and in a submodule, `.git` is a *file* pointing elsewhere, and a
# directory test reports "no repository" for a perfectly good checkout.
if ! git -C "$GITHUB_WORKSPACE" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  echo "::error::no git repository in the workspace — add actions/checkout before this step."
  exit 1
fi
if [ "$(git -C "$GITHUB_WORKSPACE" rev-parse --is-shallow-repository)" = "true" ]; then
  echo "::error::shallow clone: vibe-check computes a merge base and cannot on truncated history."
  cat >&2 <<'EOF'
Set fetch-depth: 0 on the checkout in this job:

    - uses: actions/checkout@v4
      with:
        fetch-depth: 0
        persist-credentials: false
EOF
  exit 1
fi
