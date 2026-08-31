//! Turning a panic into a verdict.
//!
//! A panic that escapes `main` exits `101`, which is a number the exit-code
//! contract in [`crate::exit`] has never heard of. A pipeline branching on that
//! contract sees an unrecognized code and, in practice, treats it as "something
//! odd happened" rather than as "this tool could not tell you anything" — which
//! is the one outcome vibe-check exists to make impossible to miss.
//!
//! So a panic is caught here and turned into the two things a caller can act
//! on:
//!
//! 1. **Exit [`exit::FAILURE`]**, because we did not produce a verdict.
//! 2. **A minimal bundle on stdout**, whose single escalation is
//!    [`ReasonCode::InternalPanic`] at [`Tier::TOP`], so the comment and check
//!    run that gate a merge say `human` rather than saying nothing.
//!
//! Those two numbers describe different things and do not contradict each
//! other. The exit code answers *did vibe-check work?*; the bundle answers
//! *what should happen to this pull request?* A crash is `1` on the first
//! question and `human` on the second, and the full argument for that lives in
//! [`crate::exit`], which owns the table.
//!
//! # Why the bundle's identity fields are a sentinel
//!
//! A panic can happen before the repository, the head commit, or the merge base
//! have been resolved, and the whole point of this path is that it cannot fail.
//! Each of those four fields therefore carries [`UNKNOWN`] rather than a
//! best-effort guess: a bundle that says `head_sha: "unknown"` is honest, while
//! one that says `head_sha: "HEAD"` is a claim nobody checked.
//!
//! `bundle_id` and `verdict_digest` are **not** identity fields and do not get
//! that sentinel — see [`NO_DIGEST`].

use std::collections::BTreeMap;
use std::future::{Future, poll_fn};
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::sync::OnceLock;
use std::task::Poll;

use vibe_check_model::{
    Adjudicators, BundleCore, Confidence, EvidenceBundle, EvidenceRef, Generator, ReasonCode,
    SchemaVersion, Tier,
};

use crate::{Cli, assembly, exit};

/// The value every identity field of a panic bundle carries.
///
/// One constant rather than six string literals, so that a reader grepping a
/// bundle for this value finds the reason it is there attached to it.
pub const UNKNOWN: &str = "unknown";

/// The value a panic bundle carries where a digest would go.
///
/// [`BundleCore::bundle_id`] is documented as "derived from content digests …
/// so that regenerating the same evaluation yields the same identifier", and
/// [`BundleCore::verdict_digest`] as "what the replay test compares". Neither
/// is an identity field, so [`UNKNOWN`] is the wrong answer for them: a
/// consumer keying a store on `bundle_id` would collapse every crash the tool
/// ever emits into one row, and a replay recomputing `verdict_digest` would
/// read a mismatch as a *tampered* bundle rather than as one that carries no
/// digest.
///
/// # Why a sentinel rather than a real digest
///
/// Computing one would mean defining the canonical form the digest is taken
/// over — the algorithm every future bundle is compared by — from inside a
/// crash path. That belongs to the milestone that writes the first real
/// bundle, and a value sitting in the `blake3:` namespace but produced by a
/// different function is worse than an obviously absent one, because it looks
/// authoritative to a consumer that has no way to tell.
///
/// So this deliberately sits **outside** that namespace. Anything parsing
/// `blake3:<hex>` rejects it on sight, which is the outcome we want: not a
/// mismatch, not a collision, but "this bundle carries no digest, and here is
/// why".
pub const NO_DIGEST: &str = "none:internal-panic";

/// The environment variable that asks `run` to panic on purpose.
///
/// A test seam, not a feature: absent from `--help`, absent from `action.yml`,
/// and compiled out of anything shipped. It exists because the property this
/// module is for — *a panic exits `1` and emits a `human` bundle* — involves
/// `catch_unwind`, `set_hook`, and the exit code the operating system reports,
/// all of which are process-global. Nothing short of panicking in a real
/// process proves it.
///
/// # Why this gate and not the other two
///
/// **A cargo feature** is wrong here for a reason specific to this repository:
/// `mise run test` runs `cargo test --workspace --all-targets --all-features`,
/// so `#[cfg(feature = "…")]` would be *on* in every build the gate makes. The
/// hatch would be compiled into the very binary the gate runs, which is the
/// outcome the gate is supposed to rule out.
///
/// **No gate at all** works and is what this shipped as first, but it leaves a
/// release binary one environment variable away from a deliberate crash. It
/// fails closed — a forced panic yields `t2`/`human`, the strictest verdict
/// available, so it is a nuisance rather than a bypass — but a nuisance with a
/// smaller blast radius available for free is not worth keeping.
///
/// **`#[cfg(debug_assertions)]`** is that gate. It is a property of the
/// *profile*, not of the feature set, so `--all-features` does not reach it:
///
/// - `cargo test` builds under the `test` profile, which inherits `dev`, where
///   `debug_assertions` is on. `CARGO_BIN_EXE_vibe-check` — how
///   `tests/panic_is_a_verdict.rs` finds the binary — is built under that same
///   profile, so the hatch is present exactly where the proof needs it.
/// - `cargo build --release` has `debug_assertions` off, so the hatch and its
///   `panic!` are not compiled at all.
///
/// This holds because no `[profile]` section exists anywhere in the workspace
/// and there is no `.cargo/config.toml`. A profile that set
/// `debug-assertions = true` for `release`, or `false` for `test`, would move
/// the hatch with it — the first would ship it, and the second would delete the
/// only end-to-end proof that a panic is a verdict. Both directions are worth
/// noticing before the profile is written.
pub const PANIC_HATCH: &str = "VIBE_CHECK_PANIC";

/// The first panic observed in this process, as `<payload> at <location>`.
///
/// A `OnceLock` rather than a slot that can be overwritten: the first panic is
/// the one that explains the rest. A leaf that panicked and was already handled
/// by the scheduler therefore keeps its message here even if the process later
/// panics again, which is a deliberate trade — the earliest cause is the one
/// worth reporting.
static FIRST_PANIC: OnceLock<String> = OnceLock::new();

/// A future panicked. The payload is deliberately not carried.
///
/// Nothing downstream can do anything useful with a `Box<dyn Any>`, and the
/// message a human wants is already in [`FIRST_PANIC`], recorded by the hook
/// [`install`] sets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Panicked;

/// Install the panic hook that records where a panic happened.
///
/// Must run **after** `color_eyre::install()`, whose own hook prints the
/// backtrace a crash report is worth having.
///
/// This one *wraps* that hook rather than replacing it. Recording alone would
/// buy a bundle at the cost of the report that says which line of which crate
/// came apart, and a verdict nobody can act on is only half the job. The
/// previous hook keeps printing, to stderr, so stdout stays exactly one JSON
/// document; this hook only adds a note of the first panic for the bundle to
/// carry.
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info.location().map_or_else(
            || "an unknown location".to_owned(),
            std::string::ToString::to_string,
        );
        let payload = info
            .payload_as_str()
            .unwrap_or("a panic with a non-string payload");
        let _ = FIRST_PANIC.set(format!("{payload} at {location}"));
        previous(info);
    }));
}

/// Run a parsed command, returning the process exit code, and never panicking.
///
/// The one function both binaries call. Every outcome that gets this far — a
/// verdict, an error, a panic, and a panic *while reporting* a panic — leaves
/// through here as a `u8` the caller passes to `std::process::exit`, so
/// neither shim has to know that any of this happened.
///
/// # What this cannot cover
///
/// **Anything before the call.** Argument parsing happens in the binaries, and
/// clap exits `2` itself on a bad command line. That is why both shims read
/// `args_os` rather than `args`: the `String` iterator *panics* on a non-UTF-8
/// argument, and a panic raised before this function is entered escapes as
/// `101` with no bundle.
///
/// **A process killed by a signal.** If stderr cannot be written — a full disk,
/// a closed log — the panic hook's own report fails, and a panic while
/// panicking aborts. `SIGABRT` is not an exit code and is outside every exit
/// table; no `u8` returned from here can describe it. The writes *this* module
/// performs are all fallible-and-ignored for that reason, so they contribute
/// nothing to the risk, but the hook that prints the crash report is
/// `color_eyre`'s and is not ours to make infallible.
pub async fn run_guarded(cli: Cli) -> u8 {
    match caught(crate::run(cli)).await {
        Ok(Ok(code)) => code,
        Ok(Err(report)) => {
            // Report the failure, then exit with the reserved failure code.
            // Never 0, and never a verdict code: "we could not tell" is not a
            // verdict, and a pipeline must be able to tell the difference.
            //
            // `writeln!` and not `eprintln!`: the macro panics if the write
            // fails, and a full disk turning an ordinary error into an abort is
            // a worse outcome than a diagnostic nobody reads.
            let _ = writeln!(std::io::stderr(), "{report:?}");
            exit::FAILURE
        }
        // A panic while handling the panic is still a failure, and still not
        // `101`. `report` is a plain `fn`, so it is `UnwindSafe` on its own.
        Err(Panicked) => std::panic::catch_unwind(report).unwrap_or(exit::FAILURE),
    }
}

/// Await `future`, catching a panic instead of unwinding through the caller.
///
/// `std::panic::catch_unwind` takes a closure, and a future's work happens
/// across polls rather than inside one call, so the guard goes around each
/// individual `poll`. The future is boxed so that pinning it needs no `unsafe`,
/// and it is never polled again after a panic — a future that unwound mid-poll
/// has no defined state to resume from.
///
/// # `AssertUnwindSafe`, at two call sites that differ
///
/// From [`run_guarded`] the assertion is trivially correct: the process writes
/// a bundle and exits, so the only state that outlives the panic is
/// [`FIRST_PANIC`], which the hook wrote and nothing mutates afterwards.
///
/// From [`crate::scheduler`] it is a real claim, because the process *keeps
/// going*: the surviving leaves hold the same `Arc<dyn Exec>` the panicking one
/// did. It holds today because no `Exec` implementation carries interior
/// mutability across an await. A future one that panicked while holding a lock
/// would poison it, and the leaves after it would see the broken half-state
/// this type is normally there to warn about. Weigh that when adding one.
pub(crate) async fn caught<F: Future>(future: F) -> Result<F::Output, Panicked> {
    let mut future = Box::pin(future);
    poll_fn(move |cx| {
        match std::panic::catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Err(_payload) => Poll::Ready(Err(Panicked)),
        }
    })
    .await
}

/// Panic when [`PANIC_HATCH`] is set in the environment, in a build that has
/// debug assertions on.
///
/// Called from [`crate::run`], which is the deepest point that both binaries
/// share, so the panic it raises travels the same path a real one would.
///
/// In a build with debug assertions off — which is every release build, since
/// this workspace overrides no profile — the body below does not exist and this
/// is an empty function the optimizer removes. See [`PANIC_HATCH`] for why the
/// gate is the profile rather than a feature.
pub(crate) fn panic_if_requested() {
    // The one place in this crate whose purpose is to panic. The workspace-wide
    // ban is lifted here and nowhere else, and the `#[allow]` is scoped to this
    // block so nothing can inherit the exemption.
    #[cfg(debug_assertions)]
    #[allow(clippy::panic)]
    {
        if std::env::var_os(PANIC_HATCH).is_some() {
            panic!("deliberate panic requested by {PANIC_HATCH}");
        }
    }
}

/// Write the panic bundle and return the exit code.
///
/// Ordinary `fn` rather than a closure so that [`run_guarded`] can hand it
/// straight to `catch_unwind`.
fn report() -> u8 {
    let detail = FIRST_PANIC.get().cloned().unwrap_or_else(|| {
        "a panic whose location was not recorded — the hook was replaced or never installed"
            .to_owned()
    });
    // Every write here is fallible-and-ignored. The `println!`/`eprintln!`
    // macros panic when the write fails, and this function is the last thing
    // standing between a crash and an exit code — a full disk must not turn a
    // reported panic into an abort with no status at all.
    let _ = writeln!(std::io::stderr(), "vibe-check panicked: {detail}");

    match serde_json::to_string_pretty(&minimal_bundle(&detail)) {
        Ok(json) => {
            let _ = writeln!(std::io::stdout(), "{json}");
        }
        Err(error) => {
            let _ = writeln!(
                std::io::stderr(),
                "the panic bundle could not be rendered: {error}"
            );
        }
    }
    // `std::process::exit` runs no destructors, so nothing else will flush.
    let _ = std::io::stdout().flush();

    exit::FAILURE
}

/// The bundle a panic produces: one escalation, and no claims about anything
/// that had not been resolved when the process died.
///
/// Built through [`Adjudicators::integrity`] like every other policy-integrity
/// fact, so [`Tier::TOP`] arrives the same way it does everywhere else and the
/// verdict is derived rather than asserted.
fn minimal_bundle(detail: &str) -> EvidenceBundle {
    let mut adjudicators = Adjudicators::new();
    adjudicators.integrity().escalate(
        Tier::TOP,
        ReasonCode::InternalPanic,
        detail,
        EvidenceRef::Unattributed,
    );
    let (enforced, advisory) = adjudicators.finish();

    EvidenceBundle {
        schema_version: SchemaVersion::BUNDLE,
        core: BundleCore::new(
            // `bundle_id`, then the four identity fields, then
            // `verdict_digest`. The two digest-shaped ones get `NO_DIGEST`;
            // the identity ones get `UNKNOWN`.
            NO_DIGEST.to_owned(),
            UNKNOWN.to_owned(),
            None,
            UNKNOWN.to_owned(),
            UNKNOWN.to_owned(),
            UNKNOWN.to_owned(),
            Vec::new(),
            BTreeMap::new(),
            NO_DIGEST.to_owned(),
            &enforced,
            &advisory,
        ),
        generator: Generator {
            name: "vibe-check".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            git_sha: None,
            registry_digest: assembly::builtin().digest(),
        },
        adjudication: enforced.into_adjudication(),
        advisory_escalations: advisory.into_escalations(),
        // No requirement was resolved, so every count is zero. That is the
        // truthful confidence for a run that died, and it is what makes the
        // bundle readable as "we know nothing" rather than "we checked and
        // found nothing".
        confidence: Confidence::default(),
        extensions: serde_json::Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibe_check_model::Verdict;

    #[tokio::test]
    async fn a_future_that_does_not_panic_yields_its_value() {
        assert_eq!(caught(async { 7_u8 }).await, Ok(7));
    }

    #[tokio::test]
    async fn a_panic_after_an_await_is_caught_rather_than_unwound() {
        // Across an await point, so the guard is proved to wrap every poll and
        // not only the first one.
        let outcome: Result<(), Panicked> = caught(async {
            tokio::task::yield_now().await;
            panic!("boom");
        })
        .await;
        assert_eq!(outcome, Err(Panicked));
    }

    #[test]
    fn the_panic_bundle_is_a_human_verdict() {
        // The property the issue is about: a crash is still a verdict, and the
        // verdict is `human`. `T2` is what makes the check run block a merge.
        let bundle = minimal_bundle("boom at src/lib.rs:1:1");
        assert_eq!(bundle.core.tier, Tier::T2);
        assert_eq!(bundle.core.verdict, Verdict::Human);
        assert_eq!(bundle.adjudication.escalations.len(), 1);

        let escalation = &bundle.adjudication.escalations[0];
        assert_eq!(escalation.reason, ReasonCode::InternalPanic);
        assert_eq!(escalation.to, Tier::T2);
        assert!(
            escalation.detail.contains("boom at src/lib.rs:1:1"),
            "the escalation must carry where the panic happened: {}",
            escalation.detail
        );
    }

    #[test]
    fn the_panic_bundle_claims_nothing_it_did_not_resolve() {
        // A panic can happen before the repository or the merge base is known.
        // A guessed identity field is worse than an honest sentinel, because
        // the escape-rate loop cannot tell the two apart afterwards.
        let bundle = minimal_bundle("boom");
        assert_eq!(bundle.core.repo, UNKNOWN);
        assert_eq!(bundle.core.head_sha, UNKNOWN);
        assert_eq!(bundle.core.merge_base_sha, UNKNOWN);
        assert_eq!(bundle.core.base_ref, UNKNOWN);
        assert_eq!(bundle.core.pr, None);
        assert_eq!(bundle.confidence.requirements, 0);
        assert!(bundle.advisory_escalations.is_empty());
        assert_eq!(bundle.core.advisory_tier, Tier::T0);
    }

    #[test]
    fn the_digest_fields_are_not_identity_fields() {
        // `bundle_id` and `verdict_digest` are digests, not identity. Giving
        // them the identity sentinel would put a value where a consumer expects
        // one it can key a store on or recompute, and "unknown" is neither
        // obviously absent nor obviously a digest.
        let bundle = minimal_bundle("boom");
        assert_eq!(bundle.core.bundle_id, NO_DIGEST);
        assert_eq!(bundle.core.verdict_digest, NO_DIGEST);
        assert_ne!(bundle.core.bundle_id, UNKNOWN);

        // And it must not be mistakable for one. Every digest this workspace
        // writes lives in the `blake3:` namespace; a replay that recomputed a
        // digest and compared it against this must reject it as absent rather
        // than report a mismatch, which reads as tampering.
        assert!(
            !NO_DIGEST.starts_with("blake3:"),
            "the sentinel must sit outside the digest namespace: {NO_DIGEST}"
        );
    }

    #[test]
    fn the_panic_bundle_is_serializable() {
        // The reporting path has no fallback that could re-render this, so a
        // bundle that fails to serialize would leave stdout empty.
        let json = serde_json::to_string_pretty(&minimal_bundle("boom")).expect("serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            parsed.pointer("/core/verdict"),
            Some(&serde_json::json!("human"))
        );
        assert_eq!(
            parsed.pointer("/adjudication/escalations/0/reason"),
            Some(&serde_json::json!("internal-panic"))
        );
    }

    #[test]
    fn the_hatch_stays_shut_unless_it_is_asked_for() {
        // The seam is only safe because it is inert by default: this is the
        // assertion that a release binary does not panic on its own.
        assert!(
            std::env::var_os(PANIC_HATCH).is_none(),
            "this test asserts the default, so the variable must not be set for it"
        );
        panic_if_requested();
    }
}
