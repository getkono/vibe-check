# Releasing

## Two tag namespaces

| Namespace | Created by | Points at | Carries |
|---|---|---|---|
| `binaries-v0.1.0` | release-plz, on merge to master | the released commit | the GitHub release, the changelog, five platform archives, `SHA256SUMS` |
| `v0.1.0` | the promotion job | a later master commit | `dist/RELEASE` and `dist/SHA256SUMS` naming `binaries-v0.1.0` |
| `v0` (`v1`, …) | the promotion job, force-moved | the same commit as the newest `v0.x.y` | the same |

### Why two

Digest verification is only worth performing if the digest is committed **into
the action's own tree at the ref being used**. Fetching the binary and its
digest from the same release verifies transport integrity and nothing else: an
asset overwritten after publication takes its digest with it, and GitHub permits
exactly that.

But the digests only exist after the binaries are built, and the binaries are
built from the tag. The tree at `binaries-v0.1.0` can never contain the digests
of the binaries built from `binaries-v0.1.0`. That circularity is not solvable
in place; it is solvable by decoupling. The action's ref names the release it
consumes, and the two are joined by a commit made only after a workflow
re-downloaded every asset anonymously and checked it.

Consequence, stated plainly: **`uses: getkono/vibe-check@master` does not work,
and neither does `@binaries-v0.1.0`.** Only action tags carry `dist/`, and the
action says so by name when it cannot find one.

## The `@vN` promotion policy

`vN` is a moving tag that lives alongside the immutable `vN.M.P` tags.

It moves **only**:

- from `.github/workflows/release-binaries.yml`, triggered by `Release-plz`
  completing a run that actually released — never by hand, never from any other
  workflow;
- for a release whose version has major `N`;
- **after every platform binary and its digest is attached** to the source
  release, and after those bytes have been re-downloaded from the public release
  URL and verified against the digests computed on the build runners. A `vN`
  pointing at a release with no binaries — or with four of five — is the silent
  failure this whole chain exists to prevent;
- **never to a prerelease.** Checked twice, independently: the version string
  must carry no `-` suffix, and GitHub must report `isPrerelease: false`;
- **never backwards.** The alias's current commit records its own version in
  `dist/RELEASE`; the job refuses to move the alias to a version that does not
  sort strictly above it under `sort -V`.

`vN.M.P` is immutable. The job refuses to create one that already exists.

### Before 1.0

The alias today is `v0`, because the workspace version is `0.x`. **`v0` carries
no compatibility promise.** It is moved by the same machinery `v1` will use, so
that the machinery is exercised and verified long before anything depends on it
— which is the entire reason for shipping the distribution chain before there is
a verdict to distribute. Pin `@v0.1.0` if you want stability now.

### What requires a major bump

- a change to `action.yml`'s input surface — frozen at `config-file` and
  `config-inline`, so in practice this never happens;
- a change to the exit-code mapping in `crates/vibe-check/src/exit.rs`, which is
  documented as a public interface. **This is the realistic `v2` trigger;**
- a breaking `BundleCore` change.

`vN` never moves across a major boundary. `v1` and `v2` are separate tags that
coexist.

## Why `workflow_run`, and not `on: release`

A release created by `secrets.GITHUB_TOKEN` fires no `release` event and no
`push: tags` event — GitHub's loop prevention. The binary-build workflow would
never run: no error, no failed job, just a tag with no binaries. That is the
failure #5 describes, and it is silent.

`workflow_run` fires on the *run*, and that run was started by a human's push to
master, so loop prevention does not apply. The chain therefore works today,
without the GitHub App from #5. Configuring that App remains worthwhile for
release-plz's own pull-request authorship; it is not a prerequisite for shipping
binaries.

Two things about `workflow_run` that bite:

- **It only runs from the copy on the default branch.** This workflow cannot be
  tested from a feature branch. It will simply never fire, and the absence looks
  exactly like a broken trigger.
- **It runs with `github.ref` = the default branch.** A bare checkout gives you
  master, not the tag. Every job that needs released source names the tag.

And one that is subtler: `workflow_run` hands over a run id, a head sha and a
conclusion — no tag, no release — and it fires on *every* `Release-plz`
completion, which is every push to master, almost none of which release
anything. A conclusion of `success` says the run worked, not that it released.
The tag therefore travels as an artifact written by the run that knows it, and
**the artifact's absence is the signal that nothing was released**.

## The name `vibe-check` is taken on crates.io, and it disabled this chain

Worth knowing before anything here is debugged, because the symptom is silence.

`publish = false` skips `cargo publish` and nothing else. release-plz still asks
the **cargo registry** what the latest released version of a package is — and
`vibe-check` on crates.io is an unrelated project, published at 0.3.2 since
March 2026. release-plz compared this workspace's 0.1.0 against that 0.3.2,
concluded there was nothing to release, and exited **successfully** on every
push to master for the life of this repository. No tag, no release, no
version-bump pull request, and no error anywhere.

`git_only = true` in `release-plz.toml` is the fix: versions come from git tags
matching `git_tag_name`, and with no such tag present the package is an initial
release.

Two consequences that outlive the fix:

- **`cargo install vibe-check` will never be a supported path**, and publishing
  under that name is not available. Whatever #6 decides about crates.io, it
  decides it about a *different* name. The distribution paths that remain are
  the ones this document describes — the action, and a released binary via
  `cargo binstall` or `mise use ubi:getkono/vibe-check`.
- **A green release job is not evidence that anything was released.** The one
  observable that means it is `gh release list` being non-empty, which is why
  the verification plan checks that rather than the job's conclusion.

## If master becomes protected

The promotion job pushes `dist/` to master with `secrets.GITHUB_TOKEN`, which
works because master today carries no branch protection and no ruleset. If that
changes — #62 is the likely cause, and #93 may be another — the
`git push origin HEAD:master` step fails with "protected branch".

**The fallback is a pull request, not a personal access token.** Replace the
push with `gh pr create` (`GITHUB_TOKEN` can open one), have a human merge it,
and add a `push: branches: [master], paths: ['dist/**']` workflow that performs
the tagging. One manual step per release; no new credential.

Do not reach for a PAT. A long-lived token with write access to master, held to
avoid one click per release, is a worse trade than the click.

## What the digest does and does not protect against

Worth stating, because a verification step that is believed to do more than it
does is worse than none.

**It catches:** an asset overwritten after publication — the one property that
makes the exercise worthwhile, since `gh release upload --clobber` is how every
release workflow in this org uploads; a wrong-tag or wrong-target download, that
is, a bug in the action's own resolution; a truncated or corrupted transfer; and
a promotion that ran against a half-uploaded release, because the promote job
re-downloads before it commits anything.

**It does not catch:** anyone who can merge to `master` here — the action's tree
and the binary have the same trust root, and Sigstore attestation (M7) is the
only thing that changes that; a compromised runner in this repository's own
release workflow, which builds the bytes and computes the digest; the
`VIBE_CHECK_BINARY` path, where the digest is deliberately not consulted because
the operator supplied the bytes and the step summary says so in plain words.
