# vibe-check

Decides which evidence a pull request needs, sources it from the CI you already
run, and adjudicates a verdict.

## What this file is

This is the contract for whoever changes this workspace, human or agent. Every
section below states one invariant and names the file that enforces it. Break
one and the tool stops being trustworthy in a way that reads, from the outside,
as a pass.

Read it as a map, not as a substitute for the source: each section is a pointer
plus the single sentence that tells you what you must not do. The reasoning
lives in the module documentation, and if this file and a module doc ever
disagree, the module doc wins and this file is the bug.

User-facing framing — what vibe-check is for, how to adopt it, how to install
it, what the exit codes mean to a caller — is `README.md`'s job and is
deliberately absent here. That split is a contract about ownership, not a
description of what is written today: `README.md` is currently three lines and
carries none of it. #14 is the issue that fills it in.

Where an invariant is enforced by a **test** rather than by a type, this file
says so. That distinction is load-bearing: it tells you whether the compiler
will stop you or whether only `mise run test` will. Where nothing enforces an
invariant yet, this file says "not yet enforced" and names where enforcement
will land. Three such gaps exist today — §5 and §6 — and are listed as gaps,
not as promises. The unwritten `README.md` above is a fourth, of a different
kind: a documentation gap rather than an unenforced invariant.

## 1. The workspace

**The DAG points one way.** `vibe-check-model` depends on nothing internal;
`vibe-check-host` depends on the model; everything else depends on those two.
`vibe-check` is link-time only — it holds the binaries and the registration
seam, and nothing depends on it.

**`vibe-check-engine` must never gain a dependency on a concrete parser,
analyzer, capability, probe, forge, or renderer.** That crate does not exist
yet, which is exactly when the prohibition is cheapest to honour: it constrains
a crate nobody has written into a corner yet.

Enforcing file: `Cargo.toml`. The workspace header comment states the DAG, and
the prohibition is enforced by the absence of the dependency rather than by
review — which only works if you notice you are about to add it.

The five crates that exist today:

- **`vibe-check-model`** — the frozen vocabulary. No I/O, no async, no git, no
  HTTP. Changing it is the refactor the architecture exists to avoid.
- **`vibe-check-host`** — the side-effect ports: forge, VCS, process execution,
  clock, scheduler, escape store. The only crate whose traits are `async`.
- **`vibe-check-diff`** — reads a pull-request diff into the shared
  representation, over `karet-vcs`. Note `gix::Repository` is not `Sync`, so
  every git read happens on one thread; see the crate docs.
- **`vibe-check-testkit`** — test doubles and fixture builders. A
  dev-dependency only, and the one crate where `unwrap`/`expect`/`panic` are
  allowed in library code.
- **`vibe-check`** — the `vibe-check` and `cargo-vibe-check` binaries, the CLI,
  the exit-code mapping, the local scheduler, and the registration seam in
  `crates/vibe-check/src/assembly.rs`.

## 2. Scrutiny only rises

**`Tier` is a join-semilattice.** Tiers combine with `max`, never with
assignment. Nothing a pull request contains can lower its own tier, because no
operation that lowers a tier exists. `Verdict` is a total function of `Tier`,
never an independently stored field.

**`Adjudicator::escalate` is the only `&mut self` method**, and `finish` takes
`self` by value so a finished verdict cannot be amended. `escalate` requires a
`ReasonCode` and an `EvidenceRef`, so an escalation that cannot explain itself
is not expressible.

Enforcing files: `crates/vibe-check-model/src/tier.rs` for the lattice, with
proptests for the join being an upper bound, commutative, associative, and
idempotent; `crates/vibe-check-model/src/adjudicate/accumulator.rs` for the API;
`crates/vibe-check-model/src/reason.rs` for the required-reason argument.

**`adjudicate::accumulator` and `known` must have no child modules.** Rust field
privacy is module-scoped, not type-scoped: a child module can write the private
`tier` field directly and bypass `escalate` entirely, and nothing in the type
system would object. Put new code in a sibling under `adjudicate/`, where it has
to go through the public API.

Enforced by a **test that reads the source text**, because "no submodules" and
"one mutator" are not expressible as types:
`crates/vibe-check-model/tests/accumulator_invariants.rs`. It also asserts the
absence of `set_tier`, `tier_mut`, `DerefMut`, `impl AsMut`, and
`impl Default for Adjudicator`. Adding a submodule under either file is a test
failure, not a review comment: `cargo build` succeeds, and `mise run test` is
what stops you.

## 3. The four capability states

**Adopt, run, skip, unverified — closed deliberately**, and everything
downstream branches on them exhaustively. `CapabilityResolution` says *how* the
question was answered; the `Judgement` inside two of those states says *what*
the answer was. Collapsing the two axes has nowhere to put "adopted, but the
test binary would not compile", which is the case the negation probe is built
on.

The rules, all applied in one place:

- **Unverified is never a pass.** Every `UnverifiedReason` escalates to
  `Tier::TOP`.
- **An `Inconclusive` judgement is not a pass.** It escalates like an unverified
  result, not like a satisfied one.
- **Evidence whose provenance is `Declared` cannot satisfy anything.** A
  declaration masquerading as a measurement is the cheapest possible way to fake
  a pass, so `account` checks `Provenance::is_measured` before it looks at the
  judgement at all.
- **A derived skip is free; a policy-declared waiver costs `Tier::T1`.** Nobody
  made a judgement call in the first case. In the second a human did, and a
  change riding on a human's waiver is precisely the change that should not
  merge unattended.

Enforcing file: `crates/vibe-check-model/src/resolution.rs`.
`CapabilityResolution::account` is the **single consumer** of a resolution, so
there is one place to audit. Adding a second consumer — anything else that
inspects a `CapabilityResolution` and decides what it means — is the change that
breaks this, and it will not look like a security change while you are writing
it.

## 4. Fail-closed means unknown must stay representable

**Domain identifiers are newtypes over interned strings, never closed enums.**
`CapabilityId`, `RiskFlagId`, `ParserId` and the rest are `SmolStr` wrappers. A
policy read from the merge base may be arbitrarily old and the binary reading it
arbitrarily new, or the reverse; if an unknown name fails to deserialize, the
document does not load, and you cannot escalate over a flag you refused to
parse. Unknown has to be representable in order to be dangerous.

**`Known::get` is the only way to reach an unresolved value, and it escalates to
`Tier::TOP` as a side effect.** There is no `unwrap`, no `Deref`, no
`into_option`, and no public variant to match on — `Inner` is private to the
module, which is why `known` may have no children (see §2). `Known<T>` is
`#[must_use]`, because dropping one on the floor is an unknown identifier
passing through without escalating. Silently dropping the unknown entry instead
would convert "this build cannot check what policy demanded" into "policy
demanded nothing", which is a way to disable a gate by typo.

Enforcing files: `crates/vibe-check-model/src/ids.rs`,
`crates/vibe-check-model/src/known.rs`.

**Two enums are closed on purpose**, and the asymmetry with identifiers is
intentional. `Tier` (`crates/vibe-check-model/src/tier.rs`) and `ReasonCode`
(`crates/vibe-check-model/src/reason.rs`) are ours, not input: the set is small,
it is the core of the safety argument, and adding a variant *should* break every
match arm until each has been reconsidered. Identifiers come from documents we
did not write; reason codes and tiers do not.

## 5. The strictness asymmetry

**Unknown keys in policy are a hard error. Unknown keys in bundles are
preserved.** Policy is adversarial input: a silently ignored misspelling of a
security-relevant key weakens a gate while the diff still looks right. Bundles
are archive output: silently dropping a field an older reader does not
understand corrupts the record. Getting these backwards is a security bug, not a
style inconsistency.

Enforcing files: `crates/vibe-check-model/src/schema.rs` states the doctrine and
why. The bundle half is enforced in `crates/vibe-check-model/src/bundle.rs` by
`#[serde(flatten)] extensions` on `EvidenceBundle`, with a round-trip test
asserting that a section this build predates is not dropped on rewrite.

**The policy half is not yet enforced anywhere.** There is no policy reader in
this workspace and `#[serde(deny_unknown_fields)]` appears nowhere in it. The
implementer who writes the policy types owns putting `deny_unknown_fields` on
every one of them, in the same change that introduces them. Until then this is
doctrine with no enforcer.

## 6. Determinism

Three parts:

1. **Same diff plus same policy yields the same verdict.**
2. **Time-dependent *decisions* read the head commit's committer date, never the
   wall clock.** Waiver expiry and artifact freshness compare against it, so
   re-evaluating last month's pull request gives the verdict it had.
3. **Iteration order never reaches a digest or a bundle.**

Enforced prophylactically by `clippy.toml`, which bans `HashMap` and `HashSet`
(unspecified iteration order), `Path` and `PathBuf` (a lossily converted
non-UTF-8 path silently changes crate attribution), and the methods
`SystemTime::now`, `Instant::now`, and `fs::read_dir`.

Enforcing files: `crates/vibe-check-host/src/vcs.rs` — `Vcs::committer_date` is
the decision clock; `crates/vibe-check-host/src/clock.rs` — the sanctioned
wall-clock exception, whose every field is on the digest's exclusion list and is
display-only; `crates/vibe-check/src/assembly.rs` and
`crates/vibe-check-model/src/bundle.rs` — `BTreeSet` and `BTreeMap` in
everything that reaches a digest; `crates/vibe-check/src/scheduler.rs` — sorts
completion order away, because how long each tool happened to take is not
something anything downstream may depend on.

**Escaping one of these lints is `#[allow(clippy::disallowed_types)]` with a
comment** naming the reason. That is deliberately visible in review — and
`allow-added` is one of the risk flags vibe-check itself classifies, so we are
held to our own standard.

Two gaps, stated plainly:

- **The replay-corpus test does not exist.** `clippy.toml` describes it as "the
  real guarantee", and `BundleCore::verdict_digest` is declared in
  `crates/vibe-check-model/src/bundle.rs` and never computed anywhere in the
  workspace. `vibe-check replay` is declared in `crates/vibe-check/src/cli.rs`
  and unimplemented. Enforcement lands with the milestone that writes the first
  bundle; until then determinism rests on the lints and the proptests alone.
- **The `fs` wrapper does not exist.** `clippy.toml` tells you to use it instead
  of `std::fs::read_dir`, and there is no such module. Whoever needs the first
  directory walk owns writing it — in `vibe-check-host`, alongside the other
  side-effect ports — rather than allowing the lint.

## 7. Authority is a type, not a flag

**`ForgeRead` and `ForgeWrite` are separate traits**, and an anti-gaming probe
is handed only a `&dyn ForgeRead`. It cannot post a comment, update a check run,
or move a label — not because a policy forbids it, but because the value it
holds has no such method.

**Capabilities describe `ProcessPlan` values and never hold an `Exec`.** They
plan work; the engine runs it. Because the plan is data it can be digested, and
that digest goes into the evidence provenance, so a bundle proves a tool ran
with retries disabled and a fixed thread count rather than merely that it ran.

Both are the same move: withhold the capability rather than check a permission,
because a check can be wrong and an absent method cannot be called.

Enforcing files: `crates/vibe-check-host/src/forge.rs`,
`crates/vibe-check-host/src/exec.rs`, summarised in
`crates/vibe-check-host/src/lib.rs`.

The adjacent absolute: **there is no route from a check-run conclusion to
evidence.** A check named `tests` may have run a subset, excluded a feature, or
skipped a target; its name and its colour are not evidence about the code.
`Artifact` cannot be constructed without bytes, and `Evidence` cannot be
constructed except from a parse that succeeded. Because that refusal is
implemented as an *absence*, nothing fails when someone adds the missing
conversion — the type checker is happier afterwards and the diff looks like a
small ergonomic improvement. So the absence is asserted by a test that scans
every crate's source for a `From` impl into `Evidence` or `Artifact`, or out of
`CheckRun`, `CheckConclusion`, or `CheckRequest`:
`crates/vibe-check-model/tests/no_evidence_from_status.rs`.

## 8. Adding a builtin

**A new capability, parser, analyzer, probe, or adoption source touches exactly
two places**: a new file in the crate that implements it, and one line in
`builtin()`. If your change needs a third edit site, the seam is in the wrong
place, and that is worth fixing before the change lands.

Enforcing file: `crates/vibe-check/src/assembly.rs`.

The registry is **passed explicitly rather than reached through a global**, and
attribute-based registration was rejected for reasons specific to this system:
gate-integrity evaluates a pull request under the merge base's rules *and* its
own, so two registries must be alive at once; the registry digest goes into
every bundle, and link order is unspecified; and capabilities declared in
configuration cannot be registered at link time anyway.

`crates/vibe-check/src/assembly.rs` currently carries a
`nothing_is_registered_yet` test. The first registration deletes it — that is
what it is for.

## 9. Quality gates

`mise run check` is the gate. It is exactly four tasks, in order:

```bash
mise run format-check  # cargo fmt --all --check
mise run lint          # cargo clippy --workspace --all-targets --all-features -- -D warnings
mise run lint-actions  # actionlint
mise run test          # cargo test --workspace --all-targets --all-features
```

CI runs `mise run check` verbatim (`.github/workflows/ci.yml:41`) rather than
spelling the cargo invocations out again, because duplicating them is how CI and
the hooks drift apart.

`hk.pkl` routes through the **same** `mise` task interface, but it does not call
`check`. It calls the leaf tasks:

- **commit-msg** — `commit-msg` (`convco` on the message being written).
- **pre-commit** — `format` then `lint`, both in fix mode, with `stash = "git"`,
  scoped to `**/*.rs`.
- **pre-push** — `format-check`, `lint`, `test`, then `commits`
  (`convco check origin/master..HEAD`).

**`lint-actions` is CI-only.** A change to a workflow YAML file passes pre-push
and can still fail CI. Run `mise run check` yourself before pushing one.

Enforcing files: `mise.toml`, `.github/workflows/ci.yml`, `hk.pkl`.

Standing rules:

- Keep entry-point code thin and move behaviour into testable functions. The
  binaries are wrappers over `run` in `crates/vibe-check/src/lib.rs`.
- All public items need doc comments — `missing_docs` is `warn` workspace-wide.
- Errors must carry actionable context. "Not implemented" without a next step
  leaves the reader wondering what they misconfigured.
- `unwrap_used`, `expect_used`, and `panic` are warned workspace-wide
  (`Cargo.toml`, `[workspace.lints.clippy]`), because a panic in the adjudicator
  is a non-verdict and a non-verdict is indistinguishable from a pass to
  anything reading an exit code. Library crates lift the ban under
  `#[cfg(test)]` and only there; `vibe-check-testkit` lifts it for its library
  code too, and says why in its own docs.
- The exit-code contract lives in `crates/vibe-check/src/exit.rs`, next to the
  tier it derives from. Do not restate the table anywhere else — `README.md`
  owns the user-facing version, and does not carry it yet (#14).

Tests run hermetically and without network access, because they run on the
pre-push hook. Anything slow or networked belongs behind an `e2e` feature.

## 10. Dependencies

The full reasoning for each choice lives in `Cargo.toml`, next to the pin. What
matters here:

- **`tokio`** — the async runtime. `#[tokio::main]` on both binaries and
  `JoinSet` in `LocalScheduler`; features `rt-multi-thread`, `macros`,
  `process`, `time`, `sync` (`crates/vibe-check/Cargo.toml`), of which
  `process`, `time`, and `sync` are all declared and not yet exercised. **There
  is no HTTP client in this workspace** — no `reqwest`, no `octocrab`, no
  `hyper`, so the forge traits have no network-backed implementation. The two
  that exist are `NullForge` (`crates/vibe-check-host/src/forge.rs`), which
  refuses every read, and `FakeForge`
  (`crates/vibe-check-testkit/src/forge.rs`), the workspace's only `ForgeWrite`
  implementation. Test against `FakeForge` rather than writing a second fake.
- **`tracing`** and **`tracing-subscriber`** — structured diagnostics.
- **`eyre`** and **`color-eyre`** — application error propagation and readable
  failure reports. Library crates use `thiserror` for typed errors instead.
- **`karet-vcs`** with `default-features = false` — its git and diff layer is
  headless and already load-bearing, and `range_changes` forces rename detection
  on regardless of the user's `diff.renames`. The default `signature` feature
  pulls `ssh-key`, and a dependency that parses key material has no business in
  a process that runs untrusted pull-request code. **Never enable the `view`
  feature on any karet crate** — it pulls `ratatui`.
  `cargo tree -p vibe-check-diff` must show neither `ssh-key` nor `ratatui`.
- **`smol_str`** — interned, cheap-to-clone strings, so every open identifier is
  a newtype over one rather than an enum (see §4).
- **`camino`** — UTF-8-only paths. Non-UTF-8 paths are rejected explicitly, not
  lossily converted (see §6).
- **`jiff`** — calendar dates and timestamps, for display and for waiver
  expiry. Decisions still read the committer date, not the wall clock.

`vibe-check-model`'s dependency list is deliberately tiny and should stay that
way.

## 11. Commits

Commits MUST follow [Conventional Commits](https://www.conventionalcommits.org/)
(`feat:`, `fix:`, `chore:`, etc.). `convco` enforces this on commit, pre-push,
and pull-request CI; merge commits are exempt.

## 12. Releases

`release-plz` maintains the version-bump pull request. Merging that pull request
creates the tag and GitHub release; never bump the version or tag manually.
