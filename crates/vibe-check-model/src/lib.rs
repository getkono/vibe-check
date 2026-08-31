//! The frozen vocabulary of vibe-check.
//!
//! Every other crate in the workspace speaks these types. Changing them is the
//! refactor the architecture exists to avoid, so this crate is deliberately
//! small, has no I/O, no async, and a dependency list that should not grow.
//!
//! # The three ideas
//!
//! **Identifiers are interned strings, not enums.** A capability, risk flag, or
//! parser named by a document this build has never seen must still be
//! *representable*, because you cannot fail closed over something you refused to
//! parse. See [`ids`] and [`known`].
//!
//! **Scrutiny only rises.** [`tier::Tier`] is a join-semilattice and
//! [`adjudicate::Adjudicator`] exposes exactly one mutator,
//! [`escalate`](adjudicate::Adjudicator::escalate). Nothing in a pull request
//! can lower its own tier because no operation lowers a tier. An advisory
//! requirement is not an exception: it escalates a *second* accumulator
//! ([`adjudicate::Adjudicators`]) rather than a weakened one, and only the
//! enforced ledger ever becomes a verdict.
//!
//! **Unverified is not a pass.** The four states in
//! [`CapabilityResolution`](resolution::CapabilityResolution) are how a question
//! was answered; the [`Judgement`](resolution::Judgement) inside two of them is
//! what the answer was. Keeping those separate is what lets "adopted, and it
//! failed" and "adopted, but the test binary would not compile" both exist,
//! which the anti-gaming probes depend on.

// `unwrap`/`expect`/`panic` are denied in library code — an adjudicator that
// panics produces a non-verdict, and a non-verdict is indistinguishable from a
// pass to anything reading an exit code. In tests they are the idiomatic way to
// fail, so the ban is lifted there and only there.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod adjudicate;
pub mod bundle;
pub mod evidence;
pub mod ids;
pub mod known;
pub mod location;
pub mod reason;
pub mod resolution;
pub mod schema;
pub mod tier;

pub use adjudicate::{
    Adjudication, Adjudicator, Adjudicators, AdvisoryAdjudication, EnforcedAdjudication,
    Enforcement, Escalation,
};
pub use bundle::{BundleCore, Confidence, EvidenceBundle, Generator};
pub use evidence::{
    CaseOutcome, CaseStatus, Evidence, EvidenceFacts, LocatedFinding, Metric, ParsedEvidence,
    Provenance, RawRef, Severity,
};
pub use ids::{
    AdoptionSourceId, AnalyzerId, CapabilityId, CrateId, FactKey, LaneId, MetricKey, ParserId,
    RequirementId, RiskFlagId, RuleId,
};
pub use known::{Known, UnknownKind};
pub use location::{LineRange, Location};
pub use reason::{EvidenceRef, PolicyRef, ReasonCode};
pub use resolution::{
    CapabilityResolution, Judgement, ResolutionState, Resolutions, SkipReason, UnverifiedReason,
};
pub use schema::SchemaVersion;
pub use tier::{Tier, Verdict};
