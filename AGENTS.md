# vibe-check

A GitHub Action that classifies and adjudicates pull requests.

## Packages

- **tracing** and **tracing-subscriber** -- structured diagnostics emitted by the action.
- **eyre** and **color-eyre** -- application error propagation and readable failure reports.
- **tokio** -- asynchronous execution for GitHub API and workflow operations.

## Quality

Validate changes through the project task interface:

```bash
mise run test          # correctness
mise run format-check  # formatting
mise run lint          # lint
```

Keep entry-point code thin and move behavior into testable functions. All public items need doc comments, and errors must carry actionable context.

## Git hooks

`hk` invokes the `mise` tasks defined in `mise.toml`. Pre-commit hooks fix formatting and Clippy findings; pre-push hooks check formatting, lint, tests, and outgoing commit messages.

## Commits

Commits MUST follow [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `chore:`, etc.). `convco` enforces this on commit, pre-push, and pull-request CI; merge commits are exempt.

## Releases

`release-plz` maintains the version-bump pull request. Merging that pull request creates the tag and GitHub release; never bump the version or tag manually.
