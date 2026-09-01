# vibe-check

Decides which evidence a pull request needs, sources it from the CI you already
run, and adjudicates a verdict.

> **Status: early, and nothing adjudicates yet.**
> The command surface, the exit-code contract, the local scheduler, and the
> registration seam are in place. Classification, policy resolution, evidence
> parsing, and adjudication are not: every subcommand exits `1` with a message
> naming what it is waiting on. There is no GitHub Action, no published binary,
> and no code in this workspace that talks to a forge over the network.
>
> Everything from here to [What exists today](#what-exists-today) describes the
> design being built. That section says which of it runs.

## The problem

A green tick is not a measurement.

A check that was skipped rather than run concludes `skipped`, which renders as a
non-blocking green tick in most branch-protection configurations. A check named
`tests` may have run a subset, excluded a feature, or skipped a target — its
name and its colour say nothing about the code. No human reviewing the pull
request can tell the difference by looking.

**vibe-check exists to make your green mean something.** It decides what a
particular diff needs evidence of, goes and finds that evidence, and says
`unverified` — loudly, and never as a pass — when it is not there.

**The mechanism is adoption, not replacement.** It does not want to re-run the
job you already run; it wants the artifact that job already uploaded. A question
is answered by adopting an existing artifact wherever one exists, and by running
a tool directly only where none does.

## Why this is not another check runner

- **Routing is per-diff.** What a change needs evidence of is a function of what
  it touched, not of a fixed job list. A diff that adds `unsafe` raises
  questions a documentation typo does not.
- **`unverified` is a distinct outcome, and it can never be a pass.** "The job
  reported success but uploaded nothing machine-readable" is a specific,
  named, escalating state — not a green tick. So is "the artifact came from a
  different commit", and so is "the evidence did not answer the question".
- **A pull request cannot weaken its own gates.** A change that edits the
  workflow producing its own evidence can make that evidence say anything, so
  evidence from a modified gate is attacker-controlled by construction and is
  refused. Policy is read from the merge base, and a diff that touches policy,
  workflows, the toolchain file, or build scripts is adjudicated under both the
  merge base's rules and its own.

Nothing a pull request contains can lower its own scrutiny, because no operation
that lowers it exists. Tiers combine by taking the greater of the two, and
escalation is the only mutation.

## The four capability states

Every requirement resolves into exactly one of four states. This is the one
concept worth holding.

| state | what happened | what it costs |
| --- | --- | --- |
| **adopt** | An artifact your CI already produced answered the question. | Nothing, if the answer was yes. |
| **run** | vibe-check ran a tool to answer the question. | Nothing, if the answer was yes. |
| **skip** | The question does not apply. | Nothing when the engine derived that — the change simply does not raise the question. `T1` when a human waived it in policy, because a change riding on a human's waiver is precisely the one that should not merge unattended. |
| **unverified** | The question could not be answered. | Always escalates to the top tier. |

Two rules sit alongside them and have no exceptions. A judgement of
*inconclusive* escalates exactly like an unverified result, because a benchmark
that did not converge is not a pass. And evidence that was merely *declared*
rather than measured cannot satisfy anything, whatever it claims — an assertion
in a configuration file is the cheapest possible way to fake a pass.

## Verdicts and exit codes

Scrutiny is a tier; the verdict is a function of it; the exit code is a function
of the verdict.

| code | verdict | meaning |
| --- | --- | --- |
| `0` | `auto` | The change is sufficiently evidenced. |
| `10` | `interface-review` | A reviewer should look at the interface change. |
| `20` | `human` | A human must review. |
| `1` | — | vibe-check itself could not produce a verdict. |
| `2` | — | The command line was rejected before anything ran. |

**`1` is the one to get right in your pipeline.** A tool crash, an unreadable
policy, an unreachable merge base — none of these are `auto`. Reusing `0` for
"we could not tell" would make every outage look like a clean bill of health,
which is the exact failure mode vibe-check exists to remove from other people's
pipelines. This is also why the unimplemented commands exit `1` today rather
than `0`.

`2` is not chosen by vibe-check; it is what the argument parser exits with when
a flag this build does not have reaches it. It is listed because a script
branching on the table meets it eventually, and an undocumented code is one a
script cannot branch on. One further number is worth knowing: a crash before the
guard is installed exits `101`, which means vibe-check came apart before it
started and told you nothing. Read it the way you would read a signal.

**An exit code does not block a merge.** Nothing vibe-check reports can, on its
own — what blocks a merge is your branch protection listing the vibe-check check
as required. Without that, a red result is a red result that merges anyway, and
the tool looks broken when it is not.

## What exists today

All eight subcommands parse, and all fail:

```console
$ vibe-check classify
   0: `vibe-check classify` is not implemented in this build.
      The command surface, exit-code contract, and registration seam are in
      place; the classification, policy, and adjudication stages land in later
      milestones.
      Exiting 1 rather than 0, because "not implemented" must never be mistaken
      for "this change is fine".

Location:
   crates/vibe-check/src/lib.rs:81

$ echo $?
1
```

| built | not built |
| --- | --- |
| The command surface: `classify`, `plan`, `run`, `adjudicate`, `replay`, `init`, `escape`, `schema`, and the global `--base`, `--config`, `--format`, `--scheduler` flags. | Every one of their implementations. |
| The exit-code contract above, and a panic guard that turns a crash inside a command into exit `1` and a bundle that says `human`, rather than into a `101` the table above does not own. | Any command that can reach a non-`1` exit code. |
| The vocabulary the verdict is made of: tiers, verdicts, capability resolutions, evidence, provenance, reason codes, and the bundle types. | Anything that fills them in — no diff classifier, no policy reader, no artifact parser. |
| Reading a pull-request diff from a local git repository, against a computed merge base. | Any caller that uses it. |
| The local scheduler, and the seam where builtins get registered. | Any registered builtin. The registry is empty. |
| The `ForgeRead` / `ForgeWrite` port traits, and a forge that refuses every read so local runs degrade to a more cautious verdict rather than an error. | A forge that talks to GitHub. There is no HTTP client anywhere in this workspace. |

Consequently there is also no installation path yet. There is no `action.yml`,
so this is not usable as a GitHub Action; every crate is `publish = false`, so
nothing is on crates.io; and the release workflow cuts tags and GitHub releases
but attaches no binaries. Building from source is the only way to run it, and
what you get for your trouble is a program that exits `1`.

## Building it

The toolchain is pinned in `rust-toolchain.toml`; `rustup` will fetch it.

```console
$ git clone https://github.com/getkono/vibe-check
$ cd vibe-check
$ cargo build --release
```

That produces two binaries in `target/release/`: `vibe-check`, and
`cargo-vibe-check`, which exists so that `cargo vibe-check <command>` works.
They share one library, so the local and CI paths cannot drift.

The quality gate is `mise run check` — formatting, clippy, `actionlint`, and the
test suite, in that order. CI runs the same task rather than respelling the
cargo invocations.

## Documentation

`README.md` and `AGENTS.md` deliberately do not overlap. This file owns the
user-facing half: what vibe-check is for, how to adopt it, how to install it,
and what the exit codes mean to a caller. `AGENTS.md` owns the contract for
changing the workspace — the crate DAG, the invariants, and the file that
enforces each one — and is deliberately absent from here. Reference
documentation for policy and bundles is its own deliverable and does not exist
yet.

The reasoning behind every rule above lives in the module documentation next to
the code that implements it, which is the authority whenever prose disagrees
with it.

## Licence

MIT. See [`LICENSE`](LICENSE).
