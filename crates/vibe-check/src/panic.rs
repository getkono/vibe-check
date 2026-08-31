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
//! Every identity field therefore carries [`UNKNOWN`] rather than a
//! best-effort guess: a bundle that says `head_sha: "unknown"` is honest, while
//! one that says `head_sha: "HEAD"` is a claim nobody checked.

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

/// The environment variable that asks `run` to panic on purpose.
///
/// A test seam, not a feature: it is absent from `--help`, absent from
/// `action.yml`, and does nothing unless someone sets it. It exists because the
/// property this module is for — *a panic exits `1` and emits a `human`
/// bundle* — is only provable by panicking in a real process, and a
/// `#[cfg(feature = …)]` hatch would not do: `mise run test` builds with
/// `--all-features`, so a feature-gated panic would be compiled into the very
/// binary the gate runs.
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
/// The one function both binaries call. Every outcome — a verdict, an error, a
/// panic, and a panic *while reporting* a panic — leaves through here as a `u8`
/// the caller passes to `std::process::exit`, so neither shim has to know that
/// any of this happened.
pub async fn run_guarded(cli: Cli) -> u8 {
    match caught(crate::run(cli)).await {
        Ok(Ok(code)) => code,
        Ok(Err(report)) => {
            // Report the failure, then exit with the reserved failure code.
            // Never 0, and never a verdict code: "we could not tell" is not a
            // verdict, and a pipeline must be able to tell the difference.
            eprintln!("{report:?}");
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
/// `AssertUnwindSafe` is correct here for the reason it is usually wrong: the
/// only state that outlives the panic is [`FIRST_PANIC`], which the hook wrote
/// and nothing mutates afterwards.
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

/// Panic when [`PANIC_HATCH`] is set in the environment.
///
/// Called from [`crate::run`], which is the deepest point that both binaries
/// share, so the panic it raises travels the same path a real one would.
// The one function in this crate whose entire purpose is to panic. The
// workspace-wide ban is lifted here and nowhere else, and the `#[allow]` is
// scoped to this function so nothing can inherit the exemption.
#[allow(clippy::panic)]
pub(crate) fn panic_if_requested() {
    if std::env::var_os(PANIC_HATCH).is_some() {
        panic!("deliberate panic requested by {PANIC_HATCH}");
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
    eprintln!("vibe-check panicked: {detail}");

    match serde_json::to_string_pretty(&minimal_bundle(&detail)) {
        Ok(json) => println!("{json}"),
        Err(error) => eprintln!("the panic bundle could not be rendered: {error}"),
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
            UNKNOWN.to_owned(),
            UNKNOWN.to_owned(),
            None,
            UNKNOWN.to_owned(),
            UNKNOWN.to_owned(),
            UNKNOWN.to_owned(),
            Vec::new(),
            BTreeMap::new(),
            UNKNOWN.to_owned(),
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
        assert_eq!(bundle.core.pr, None);
        assert_eq!(bundle.confidence.requirements, 0);
        assert!(bundle.advisory_escalations.is_empty());
        assert_eq!(bundle.core.advisory_tier, Tier::T0);
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
