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

### #93: keeping two green pull requests from merging into a red master

Two pull requests can each pass CI against a stale base and still conflict
with each other once both land — #91 was exactly this twice over: a guard's
argument pool shared across a macro's rules, and `RequirementId::new` removed
in one pull request while new callers of it were added in another. Neither
GitHub check ever saw the combination that broke, because nothing required
either branch to be current against the other before merging.

**Decision: a merge queue, not "require branches up to date before
merging."** Both close the hole completely — a queue re-validates a pull
request against the current master before merging, exactly what "require
up to date" forces via a manual re-run. The difference is cost: "require up
to date" serializes every merge behind a fresh CI run, paid by whoever is
waiting; a queue pays the same re-validation without blocking a human, which
matters here because this repository's own tooling delivers backlog work as
multiple parallel pull requests, and #11's `merge_group` amendment was
already written in anticipation of this choice.

**Cost taken on:** the `merge_group` trigger in `ci.yml` has to exist before
the queue is turned on, or the queue accepts entries and merges none of them
(#11): a workflow with no `merge_group` trigger never runs for the queue's
synthetic ref, so a check required there is never posted, and an entry
waiting on a status that is not coming is failed out of the queue when the
queue's configurable status-check timeout expires. Nothing merges, and the
recorded reason is a check that was never asked to run rather than one that
ran and failed. That is the hazard this trigger closes, and it is the only
one of the two that blocks a merge at all.
A job that is triggered and then skipped by its own `if:` is the opposite —
GitHub still posts the check run and reports it as a success, and a skipped
job does not block a pull request even when it is required. So the
`Conventional commits` job's `if: github.event_name == 'pull_request'` is
deliberate, because `github.event.pull_request.base.ref` does not exist
under `merge_group`, but marking that job required would not hang a queue
entry; it would pass every one of them green while enforcing nothing. Keep
`Quality` as the only required check — not because the alternative stalls
the queue, but because it would be enforcement in name only. Wanting commit
messages enforced in the queue lane means building a check that is genuinely
`merge_group`-aware, resolving the commit range some other way when there is
no base ref to read; that resolution is #11's own unbuilt ladder work, not a
toggle.

**Second cost, and the one that bites first:** enabling the queue breaks the
release chain's direct push, on its own, before #62 is reached. "Require merge
queue" cannot be enabled without "Require a pull request before merging", and
that setting alone rejects `git push origin HEAD:master` — which is exactly
what the promote job does at `.github/workflows/release-binaries.yml`. That
file's own comment ("master carries no branch protection and no ruleset, so the
built-in token can push") becomes false at the moment the queue is switched on,
not later when a check is added. The fallback is above, under "If master
becomes protected"; it has to be in place *before* #115 is executed, or the
next release fails at the push step with the cause recorded against a different
decision. #62 and #102 compound this — #102's push already carries zero check
runs — but neither is required for it to happen.

**Third cost: CI spend roughly doubles per pull request.** Every change now
runs `Quality` twice, once on `pull_request` and once on `merge_group`, each a
full `cargo test --workspace --all-targets --all-features` over a
`fetch-depth: 0` checkout. Actions caches written on a `gh-readonly-queue/*`
ref are scoped to that ephemeral branch and discarded, so the queue lane can
read master's `rust-cache` entry but never improves it, and the second run is
the one most likely to be cold. That is the price of not serialising humans
behind a re-run, and it is worth paying at this repository's cadence — but it
is a recurring cost rather than a one-off, and #93 asks for the decision to be
recorded with its cost.

**What remains, and cannot be done from this repository's code:** branch
protection on `master` requiring the `Quality` check, "Require merge queue"
enabled, and "Require branches to be up to date before merging" left off
(the queue already guarantees that). The settings flip itself is #115, which
carries the checklist and the verified before-state; #93 is the analysis and
stays open until the queue is live and one pull request has drained through it.
Do not execute #115 before the promote job's fallback above is in place.

## The `dist/` commit carries no CI check

The promotion job pushes `chore(dist): pin the action to <tag>` with the
default `GITHUB_TOKEN` (job permission `contents: write`, no PAT). GitHub's
loop-prevention rule for `GITHUB_TOKEN` — the same rule that stops this push
from re-entering Release-plz — also means the push fires no `push` event, so
ci.yml's `quality` job never runs against it. That commit therefore reaches
`master` with zero check runs, permanently: nothing will ever retroactively
check it, because nothing fires for it to check.

**Decision: accepted, with same-job assertions over independent inputs, not
independent CI.** Before `git add`, the "Commit the digests" step makes three
assertions against the files it has just written. What each can and cannot
detect matters more than that there are three of them, so:

- `dist/lint-surface.sh` — the identical script `mise run lint-action-surface`
  runs locally and in `ci.yml`'s `quality` job on every other commit. **In
  this job it cannot fail.** Its first two assertions read `action.yml` and
  `release-plz.yml`'s `name:` out of the master checkout: files this step
  never writes, on a commit `quality` already linted. Its third compares
  `dist/RELEASE`'s `version` against the filenames in `dist/SHA256SUMS` — and
  both of those descend from the same `${VERSION}` expansion in the same
  shell, so they are two copies of one variable and cannot disagree. It never
  parses `tag`, and it discards the hash on every line of `SHA256SUMS`. Do not
  read this call as verification of the promotion; it is a structural check
  whose failure modes are all unreachable here, kept because it costs nothing
  and because it is what would catch a future edit that stopped writing a
  well-formed pair at all.
- **`dist/RELEASE`'s `tag` field, read back through `awk`.** This is the field
  `dist/install.sh` builds its download URL from, on a consumer's runner,
  months later — and until now nothing in the repository read it back at all.
  The assertion is a round trip, not a judgement: it does not know whether the
  resolved tag is the right one (the `resolve` job decided that), it proves
  that the bytes on disk parse back to it under the same one-line `awk`
  program `install.sh` will use. A tag that does not survive that round trip —
  reformatted, whitespace in it, the file's comment block reordered so a
  different line matches first — sends every consumer to a 404 and, before
  this, failed nothing anywhere.
- **`dist/SHA256SUMS` against the bytes GitHub actually served.** The step
  above, "Verify what GitHub actually serves," downloaded all five archives
  anonymously from the public release into `verify/` and checked
  `incoming/SHA256SUMS` against them. This assertion re-runs `sha256sum -c`
  from inside `verify/` against `dist/SHA256SUMS` — the copy about to be
  committed to `master` and pointed at by an immutable tag. The two files are
  a `cp` apart, and that `cp` was the one step between "verified" and
  "published" that nothing watched. A digest mistranscribed, truncated,
  reordered, or written for a different release is now fatal here, rather than
  surfacing as a failed integrity check on a consumer's runner against a tag
  that can no longer be moved. A missing `verify/` is a hard failure, never a
  skip: a check that evaporates when its inputs do is the same hole in a new
  costume.

Note what the third assertion buys and what it does not. It is still the
promote job checking its own work — but it now checks the written artifact
against an input the writing shell did not produce (bytes fetched over the
public internet from GitHub's CDN), which is a different thing from comparing
a variable to itself. That is accepted rather than deferred because the commit
touches only `dist/RELEASE` and `dist/SHA256SUMS`, never Rust source, so
`cargo fmt`/`clippy`/`test` have nothing to say about it that they do not
already say about every other commit that changed no `.rs` file.

**Honest status of #102.** Its first Done-when box — the `dist/` commit
"verified by something that is not the job that wrote it" — is **not met, and
is not met by this change.** The same job still runs all three assertions.
What changed is that two of them are now *reachable* and read *independent
inputs*, where the state before this was a call that could not fail. Meeting
that box literally requires a checker that is not the promote job: a
`workflow_dispatch`-triggered or scheduled job that re-derives `dist/` from
the release it names, or a PAT-pushed commit that fires `push` and is caught
by `quality` like any other. Both were rejected here — the first is unbuilt,
the second reintroduces the loop the `GITHUB_TOKEN` rule prevents and puts a
long-lived credential in a workflow that runs after a release. Whoever wants
the box ticked owns building the first.

**Interaction with required checks (#62, #93):** if `master` ever requires a
status check to pass — via branch protection or a ruleset — this commit is
the one commit that can never acquire one, because nothing fires for it.
Whether that wedges the push (as opposed to the existing "protected branch"
fallback above, which is written for the push itself being rejected) depends
on how #62/#93 implement the requirement, which is not decided yet. That
interaction is left unresolved here on purpose: solving it now would mean
guessing at a mechanism neither issue has settled. Whoever lands required
checks on `master` must re-read this section and the fallback above together
before doing so.

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
