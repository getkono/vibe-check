//! How a required capability turned out.
//!
//! # Two axes, not one
//!
//! [`CapabilityResolution`] says **how** the question was answered — adopted,
//! run, skipped, or unverified. [`Judgement`] says **what** the answer was.
//! Collapsing them into one enum looks tidier and immediately breaks: "adopted,
//! and it failed" and "adopted, but the test binary would not compile" have
//! nowhere to live, and the second one is exactly the case the test-negation
//! probe is built on.
//!
//! # Fail-closed lives here
//!
//! `CapabilityResolution::account` is the single consumer of a resolution, and
//! it is where the two rules that make the whole system safe are applied without
//! exception:
//!
//! - an unverified capability escalates to [`Tier::TOP`]
//! - a live human-authored waiver escalates to [`Tier::T1`]
//! - a waiver whose `expires` date has passed escalates to [`Tier::TOP`], with
//!   [`ReasonCode::ExpiredSkip`]. It stops being a waiver: an authorisation
//!   with a date on it that nothing compares against is not an authorisation,
//!   it is a comment. The comparison is against the decision time — the head
//!   commit's committer date — never the wall clock, so re-running last
//!   month's pull request still gives last month's verdict
//! - an unknown identifier is a fact about the *policy*, not a result about the
//!   code, and policy integrity is never advisory
//!
//! Because there is one consumer, there is one place to audit. That is also why
//! choosing between the enforced and advisory ledgers happens here rather than
//! at call sites: it is one of the fail-closed rules, and it belongs next to the
//! others.
//!
//! # Accounting happens in one order, always
//!
//! [`Resolutions`] is the accounting stage's input: a `BTreeMap` keyed by
//! [`RequirementId`]. [`Resolutions::account_into`] walks it in ascending key
//! order, unconditionally and with no second mode, so both escalation ledgers
//! are a function of *which* requirements resolved and how — never of the order
//! the engine happened to finish resolving them in.
//!
//! That matters because the ledgers are bundle fields. Capabilities resolve
//! concurrently and completion order depends on how long each tool happened to
//! take; a ledger accumulated in completion order would give two runs of the
//! same commit two different serializations, and any digest covering the
//! adjudication would disagree with itself. `LocalScheduler::dispatch` already
//! sorts its leaf ids for exactly this reason; this is the same discipline one
//! layer up.
//!
//! # The order goes on the input, not on the ledger
//!
//! Sorting a finished ledger would be a lie.
//! [`Escalation::from`](crate::adjudicate::Escalation::from) records the tier
//! *before* that escalation, which is a statement about the sequence: re-order
//! the entries and `from` no longer agrees with the previous entry's `to`, and
//! the ledger stops replaying to the tier it reports. Deriving `Ord` on
//! [`Escalation`](crate::adjudicate::Escalation) would also force it onto
//! [`EvidenceRef`], making that enum's variant declaration order a permanent
//! semantic commitment inside a crate whose whole purpose is to be frozen.
//!
//! So the total order is imposed on the input, where [`RequirementId`] already
//! supplies one — lexicographic bytes, alphabetical, deterministic, and nothing
//! new to invent.
//!
//! That is also why `CapabilityResolution::account` is `pub(crate)` rather than
//! `pub`: it accounts one resolution at a time, so a caller holding it could
//! account a pile of them in any order it liked. Outside this crate the only
//! way to account anything is [`Resolutions::account_into`], which has exactly
//! one. Guarded by `accounting_is_not_public` in
//! `tests/accumulator_invariants.rs`.

use std::collections::BTreeMap;

use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::adjudicate::{Adjudicators, Enforcement};
use crate::evidence::Evidence;
use crate::ids::{CapabilityId, ParserId, RequirementId};
use crate::reason::{EvidenceRef, PolicyRef, ReasonCode};
use crate::tier::Tier;
use crate::time::DecisionTime;

/// What the evidence says.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Judgement {
    /// The capability's question is answered "yes".
    Satisfied,
    /// The question is answered "no".
    Violated {
        /// What went wrong, in a sentence.
        detail: String,
    },
    /// The evidence does not answer the question either way.
    ///
    /// A benchmark that did not converge, a coverage run with no data, a
    /// negation test whose added tests would not compile against the base
    /// commit. Treated as unverified by `CapabilityResolution::account`, so an
    /// inconclusive result never reads as a pass.
    Inconclusive {
        /// Why no conclusion could be drawn.
        reason: String,
    },
}

impl Judgement {
    /// Whether this judgement can support the capability being satisfied.
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        matches!(self, Self::Satisfied)
    }
}

/// Why a capability was not evaluated, in a way that is acceptable.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum SkipReason {
    /// The engine determined the capability does not apply.
    ///
    /// No `unsafe` in the changed hunks, so Miri has nothing to check. Free: no
    /// escalation, because nobody made a judgement call — the change simply does
    /// not raise the question.
    Derived {
        /// What was observed, for the record.
        detail: String,
    },
    /// A policy waiver declares the capability not applicable.
    ///
    /// Costs [`Tier::T1`] while it is live. A human wrote this down, and a
    /// change that relies on a human's waiver is precisely the change that
    /// should not merge unattended. Long-lived waivers becoming permanently
    /// mildly annoying is the intended behaviour; the escape-rate loop is how
    /// they get retired.
    ///
    /// Past its `expires` date it costs [`Tier::TOP`] instead — see that
    /// field.
    Declared {
        /// The policy entry that granted it.
        policy_ref: PolicyRef,
        /// Why it was granted.
        reason: String,
        /// Who owns it.
        owner: String,
        /// When it lapses.
        ///
        /// `CapabilityResolution::account` compares this against the
        /// [`DecisionTime`](crate::time::DecisionTime)'s UTC civil date — the
        /// head commit's committer date, never the wall clock, so that
        /// re-running an old pull request gives the same verdict it had.
        ///
        /// The waiver is **live through the whole of this day**: it is expired
        /// only when the decision date is strictly greater. That sense is
        /// pinned by name in
        /// `the_expiry_boundary_is_inclusive_of_the_expiry_day`, because the
        /// operator alone is exactly the detail a later refactor flips without
        /// noticing — which is how this field came to be documented as compared
        /// against something while nothing compared it. Once expired the skip
        /// escalates [`Tier::TOP`] with [`ReasonCode::ExpiredSkip`] rather than
        /// [`Tier::T1`] with [`ReasonCode::DeclaredSkip`].
        expires: Date,
    },
}

impl SkipReason {
    /// Whether this skip was a human decision rather than an engine deduction.
    #[must_use]
    pub fn is_declared(&self) -> bool {
        matches!(self, Self::Declared { .. })
    }
}

/// Why a capability could not be answered.
///
/// Every variant escalates. This type is the destination for every failure in
/// the resolution pipeline: downstream crates provide `From` conversions into
/// it from their own error types, and — crucially — into *nothing else*. A parse
/// failure has exactly one thing it can become.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum UnverifiedReason {
    /// An artifact was found but could not be parsed.
    Unparseable {
        /// The parser that was tried.
        parser: ParserId,
        /// What went wrong.
        detail: String,
    },
    /// A check reported success but produced no machine-readable artifact.
    ///
    /// This is the case the whole adoption design exists to refuse. A green
    /// check named `tests` may have run a subset, excluded a feature, or skipped
    /// a target; its name is not evidence.
    NoArtifact {
        /// What was found instead.
        detail: String,
    },
    /// Evidence that should have been produced never arrived.
    MissingEvidence,
    /// An artifact could not be tied to this head commit.
    StaleArtifact {
        /// The commit the artifact claims.
        produced_from: String,
        /// The commit we needed.
        expected: String,
    },
    /// Evidence came from a workflow this pull request modifies.
    ///
    /// Attacker-controlled by construction: a change that edits the workflow
    /// producing its own evidence can make that evidence say anything.
    GatesModified {
        /// Which gate paths the change touches.
        paths: Vec<String>,
    },
    /// Answering would exceed the configured cost budget.
    ///
    /// Unverified rather than skipped: running out of compute must never be
    /// silently safe.
    BudgetExceeded {
        /// The capability's declared cost class.
        cost: String,
        /// The configured ceiling.
        max: String,
    },
    /// The evidence did not answer the question.
    Inconclusive {
        /// Why not.
        reason: String,
    },
    /// The tool could not be run.
    ExecutionFailed {
        /// What went wrong.
        detail: String,
    },
    /// No plan could be produced for this capability.
    PlanFailed {
        /// What went wrong.
        detail: String,
    },
    /// Policy names a capability this build does not implement.
    UnknownCapability {
        /// The name policy used.
        id: String,
    },
    /// Policy names a parser this build does not implement.
    UnknownParser {
        /// The name policy used.
        id: String,
    },
    /// Adoption needs a forge and there is none, e.g. running locally.
    NoForge,
    /// The artifact declares a schema newer than this build supports.
    SchemaTooNew {
        /// What it declared.
        found: u32,
        /// What we support.
        supported: u32,
    },
    /// The artifact exceeded a size limit.
    Oversized {
        /// What the limit was and what was seen.
        detail: String,
    },
}

impl UnverifiedReason {
    /// The reason code this escalates with.
    #[must_use]
    pub fn reason_code(&self) -> ReasonCode {
        match self {
            Self::UnknownCapability { .. } => ReasonCode::UnknownCapability,
            Self::UnknownParser { .. } => ReasonCode::UnknownParser,
            Self::BudgetExceeded { .. } => ReasonCode::BudgetExceeded,
            Self::GatesModified { .. } => ReasonCode::GatesModified,
            Self::StaleArtifact { .. } => ReasonCode::AdoptionStale,
            _ => ReasonCode::CapabilityUnverified,
        }
    }

    /// Whether this is a fact about the **policy** rather than about the code.
    ///
    /// Never advisory. `enforcement = "advisory"` written next to a typo'd
    /// capability name would otherwise be a two-token gate disable: the
    /// requirement names something this build cannot evaluate, the failure to
    /// evaluate it lands in a ledger nothing enforces, and the gate is gone.
    /// `CapabilityResolution::account` overrides the caller's [`Enforcement`]
    /// to the enforcing lane whenever this is true.
    ///
    /// An exhaustive `match` rather than a `matches!`, so that a new
    /// [`UnverifiedReason`] variant has to be classified by whoever adds it.
    #[must_use]
    pub fn is_policy_integrity(&self) -> bool {
        match self {
            Self::UnknownCapability { .. } | Self::UnknownParser { .. } => true,
            Self::Unparseable { .. }
            | Self::NoArtifact { .. }
            | Self::MissingEvidence
            | Self::StaleArtifact { .. }
            | Self::GatesModified { .. }
            | Self::BudgetExceeded { .. }
            | Self::Inconclusive { .. }
            | Self::ExecutionFailed { .. }
            | Self::PlanFailed { .. }
            | Self::NoForge
            | Self::SchemaTooNew { .. }
            | Self::Oversized { .. } => false,
        }
    }

    /// A sentence a maintainer can act on.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::Unparseable { parser, detail } => {
                format!("artifact could not be parsed by `{parser}`: {detail}")
            }
            Self::NoArtifact { detail } => format!(
                "no machine-readable artifact: {detail}. \
                 A passing check is not evidence; it must upload a parseable result."
            ),
            Self::MissingEvidence => {
                "expected evidence was never produced; the job may have been skipped or failed"
                    .into()
            }
            Self::StaleArtifact {
                produced_from,
                expected,
            } => format!(
                "artifact was produced from {produced_from} but the head is {expected}; \
                 re-run the producing workflow for this commit"
            ),
            Self::GatesModified { paths } => {
                // Sorted for the same reason accounting is ordered (#26). This
                // sentence reaches a bundle field, and a set of modified gate
                // paths collected by walking a diff arrives in whatever order
                // the walk produced; two runs of the same commit would then
                // write two different sentences. A local copy, so the variant's
                // own field is untouched and this stays display-only — no
                // contract change, and nothing downstream may read the order
                // back out of the prose.
                let mut paths = paths.clone();
                paths.sort();
                format!(
                    "evidence comes from a workflow this change modifies ({}); \
                     adopted results cannot be trusted here",
                    paths.join(", ")
                )
            }
            Self::BudgetExceeded { cost, max } => {
                format!("cost class `{cost}` exceeds the configured maximum `{max}`")
            }
            Self::Inconclusive { reason } => format!("evidence was inconclusive: {reason}"),
            Self::ExecutionFailed { detail } => format!("could not run the tool: {detail}"),
            Self::PlanFailed { detail } => format!("could not plan the capability: {detail}"),
            Self::UnknownCapability { id } => format!(
                "policy requires capability `{id}`, which this build does not implement; \
                 upgrade vibe-check or declare it in the policy"
            ),
            Self::UnknownParser { id } => {
                format!("policy names parser `{id}`, which this build does not implement")
            }
            Self::NoForge => {
                "adoption needs access to the forge; running locally without a token".into()
            }
            Self::SchemaTooNew { found, supported } => format!(
                "evidence declares schema v{found} but this build supports up to v{supported}; \
                 upgrade vibe-check"
            ),
            Self::Oversized { detail } => format!("artifact exceeded a size limit: {detail}"),
        }
    }
}

/// Which of the four states a requirement landed in.
///
/// Split out from [`CapabilityResolution`] so counts and the bundle's state
/// table can be built without cloning the evidence.
///
/// # This is the answering *method*, not the answer
///
/// `Run` means vibe-check ran something, whatever that something reported:
/// [`CapabilityResolution::state`] maps `Ran { judgement }` to `Run` for every
/// judgement, a violation included. Whether the answers passed lives in the
/// adjudication and its escalations, never here. Anything that reads a state as
/// an outcome will read a failing test run as good news.
///
/// # No derived ordering
///
/// `PartialOrd`/`Ord` are deliberately not derived. A derived order is
/// declaration order — `Adopt` < `Run` < `Skip` < `Unverified` — which is not
/// the confidence order and puts `Adopt` *below* `Unverified`. Aggregating a
/// capability's scopes with `min` under that order reports `adopt` for a
/// capability whose other scope went unanswered, which is a fail-open written
/// into a frozen bundle field. Without the derive that line does not compile,
/// anywhere in the workspace, and the only way to combine two states is
/// [`collapse`](Self::collapse). `tests/nothing_orders_a_resolution_state.rs`
/// keeps the derive from coming back.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionState {
    /// An existing artifact answered it.
    Adopt,
    /// vibe-check ran something to answer it.
    Run,
    /// Declared not applicable.
    Skip,
    /// Expected, and unavailable.
    Unverified,
}

impl ResolutionState {
    /// Order by how much is actually known, least first:
    /// `Unverified` < `Skip` < `Adopt` < `Run`.
    ///
    /// This is the order [`collapse`](Self::collapse) minimises over, and the
    /// only order this type has — there is no derived [`Ord`], for the reason
    /// given on the enum.
    ///
    /// Two of the three steps are worth stating outright, because they are
    /// what a consumer bucketing capabilities by state inherits:
    ///
    /// - **`Skip` below `Adopt`** is deliberate. A skipped scope was never
    ///   answered — a waiver or a declared inapplicability is a decision *not*
    ///   to measure — while an adopted one was answered by an artifact. So a
    ///   capability that ran clean on one crate and was waived on another
    ///   collapses to `skip`, and a consumer counting "measured capabilities"
    ///   by state must not count it. Ranking `Skip` above `Adopt` would hide
    ///   the waiver behind the crate that happened to pass, which is the same
    ///   fail-open in a quieter register.
    /// - **`Adopt` below `Run`** is the debatable one, and it is inert:
    ///   escalation is driven by the resolution, not by this rank, so which of
    ///   the two answered methods ranks first changes a label and never a
    ///   verdict.
    #[must_use]
    pub fn confidence_rank(self) -> u8 {
        match self {
            Self::Unverified => 0,
            Self::Skip => 1,
            Self::Adopt => 2,
            Self::Run => 3,
        }
    }

    /// Combine two states for the same capability: least confident wins.
    ///
    /// A requirement is a *(capability × scope)* pair, so one capability can
    /// resolve more than once — `tests-pass` adopted for `kono-core` and
    /// unverified for `kono-net` is two resolutions.
    /// [`BundleCore::capability_states`](crate::bundle::BundleCore::capability_states)
    /// is keyed by a bare [`CapabilityId`](crate::ids::CapabilityId), so those
    /// resolutions have to become one entry, and this is the rule that makes
    /// them one: the [`confidence_rank`](Self::confidence_rank) minimum. The
    /// pair above collapses to `Unverified`, never to `Adopt`, because an entry
    /// reading `adopt` while some scope went unanswered is a fail-open written
    /// into the one part of the bundle that can never be corrected later.
    ///
    /// Binary rather than iterator-shaped, so the laws it must obey are
    /// directly expressible: `collapse` is a meet — commutative, associative,
    /// idempotent, with `Unverified` absorbing and `Run` the identity — exactly
    /// as [`Tier::join`](crate::tier::Tier::join) is a join.
    /// `tests/collapse_is_a_semilattice.rs` proves all five. Commutativity is
    /// the load-bearing one: scopes resolve concurrently, so a rule that
    /// depended on which finished first would write scheduling noise into a
    /// frozen bundle field.
    ///
    /// What collapses is the answering *method*. `Run` is the identity, so a
    /// capability that ran and reported a violation for one crate and adopted a
    /// satisfied artifact for another collapses to `Adopt` — the run is not
    /// "worse" here, it is more confident. Correct for a map of how each
    /// question was answered, and the reason nothing may read an outcome out of
    /// that map; the failing run is in the adjudication, where it escalated.
    ///
    /// Collapsing is lossy, and the loss is countable —
    /// [`Confidence::partial`](crate::bundle::Confidence::partial) — though
    /// nothing can feed that count until a resolution carries the capability it
    /// answers. See
    /// [`tally_by_capability`](crate::bundle::Confidence::tally_by_capability).
    #[must_use]
    pub fn collapse(self, other: Self) -> Self {
        if other.confidence_rank() < self.confidence_rank() {
            other
        } else {
            self
        }
    }

    /// [`collapse`](Self::collapse) across every state a capability resolved
    /// to, or `None` when it resolved to none.
    ///
    /// `None` rather than a defaulted state: no variant means "nothing asked
    /// for this", and the nearest candidates lie in opposite directions —
    /// `Skip` claims someone decided it was inapplicable, `Unverified` claims
    /// someone expected it. A capability with no requirements has no entry in
    /// `capability_states`, which is what `None` says.
    #[must_use]
    pub fn collapse_all(states: impl IntoIterator<Item = Self>) -> Option<Self> {
        states.into_iter().reduce(Self::collapse)
    }
}

/// How a required capability was resolved.
///
/// Four states, closed deliberately: this is the specification's own hard rule,
/// and everything downstream branches on it exhaustively.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum CapabilityResolution {
    /// An existing artifact answered the question.
    Adopted {
        /// The normalized evidence.
        evidence: Box<Evidence>,
        /// What it says.
        judgement: Judgement,
    },
    /// vibe-check ran a tool to answer the question.
    Ran {
        /// The normalized evidence.
        evidence: Box<Evidence>,
        /// What it says.
        judgement: Judgement,
    },
    /// The question does not apply.
    Skipped {
        /// Why not.
        reason: SkipReason,
    },
    /// The question could not be answered.
    Unverified {
        /// Why not.
        reason: UnverifiedReason,
    },
}

impl CapabilityResolution {
    /// Which of the four states this is.
    #[must_use]
    pub fn state(&self) -> ResolutionState {
        match self {
            Self::Adopted { .. } => ResolutionState::Adopt,
            Self::Ran { .. } => ResolutionState::Run,
            Self::Skipped { .. } => ResolutionState::Skip,
            Self::Unverified { .. } => ResolutionState::Unverified,
        }
    }

    /// Whether this resolution is a fact about the policy rather than about the
    /// code, and therefore may never be routed to the advisory ledger.
    ///
    /// An exhaustive `match` rather than a `_` arm: a fifth resolution state
    /// would have to be classified here before it compiles.
    fn is_policy_integrity(&self) -> bool {
        match self {
            Self::Unverified { reason } => reason.is_policy_integrity(),
            Self::Adopted { .. } | Self::Ran { .. } | Self::Skipped { .. } => false,
        }
    }

    /// Apply this resolution to the verdict.
    ///
    /// **The only consumer of a resolution**, and therefore the only place the
    /// fail-closed rules need to be correct:
    ///
    /// | resolution | enforcement | effect |
    /// |---|---|---|
    /// | satisfied, from measured evidence | either | nothing |
    /// | violated | as given | escalate that ledger to [`Tier::TOP`] |
    /// | inconclusive | as given | escalate that ledger to [`Tier::TOP`] — an inconclusive result is not a pass |
    /// | satisfied, but only *declared* | as given | escalate that ledger to [`Tier::TOP`] — an assertion is not a measurement |
    /// | engine-derived skip | either | nothing |
    /// | policy-declared waiver, still live | as given | escalate that ledger to [`Tier::T1`] |
    /// | policy-declared waiver, expired | as given | escalate that ledger to [`Tier::TOP`] — an expired waiver authorises nothing |
    /// | unverified | as given | escalate that ledger to [`Tier::TOP`] |
    /// | unverified, unknown capability or parser | **overridden** | escalate the *enforced* ledger to [`Tier::TOP`] |
    ///
    /// Note there is no path through this function in which an unanswered
    /// question leaves both tiers alone, and no path in which a fact about the
    /// policy reaches the advisory ledger.
    ///
    /// # `at` is the committer date, and the only time this reads
    ///
    /// The waiver rows are the two the decision time separates, and `at` is
    /// what separates them: the head commit's committer date, wrapped in
    /// [`DecisionTime`](crate::time::DecisionTime) so that nothing else can
    /// arrive here wearing the right name. A wall clock in this position would
    /// make a waiver live when the pull request was opened and dead when CI
    /// re-ran it a month later — the same commit, two verdicts.
    ///
    /// It stays a parameter rather than a field on [`Resolutions`] or on
    /// [`Adjudicators`] because there is one caller that legitimately has *no*
    /// committer date — the internal-panic path, which builds an
    /// [`Adjudicators`] precisely when the run fell over before obtaining one.
    /// A field there would have to be fabricated, which is the hole
    /// [`DecisionTime`](crate::time::DecisionTime) exists to close.
    ///
    /// The expiry comparison is deliberately not exposed as a predicate on
    /// [`SkipReason`]: this function is the single consumer of a resolution,
    /// and a public "is this waiver dead?" is a second consumer waiting for a
    /// caller.
    ///
    /// `pub(crate)`, not `pub`. This accounts *one* resolution, so a caller that
    /// could reach it could account a whole set in an order of its choosing —
    /// and both ledgers are bundle fields. [`Resolutions::account_into`] is the
    /// only way in from outside the crate, and it has exactly one order.
    /// Guarded by `accounting_is_not_public`.
    pub(crate) fn account(
        &self,
        requirement: &RequirementId,
        enforcement: Enforcement,
        at: DecisionTime,
        adjudicators: &mut Adjudicators,
    ) {
        // The routing rule, and the only line of this function that is new. An
        // unknown identifier is a fact about the policy; policy integrity is
        // never advisory, whatever the requirement asked for.
        let lane = if self.is_policy_integrity() {
            Enforcement::Enforcing
        } else {
            enforcement
        };
        let adjudicator = adjudicators.route(lane);

        let evidence_ref = EvidenceRef::Requirement(requirement.clone());
        match self {
            Self::Adopted {
                evidence,
                judgement,
            }
            | Self::Ran {
                evidence,
                judgement,
            } => {
                // A declaration masquerading as measured evidence would be the
                // cheapest possible way to fake a pass. It cannot satisfy.
                if !evidence.provenance().is_measured() {
                    adjudicator.escalate(
                        Tier::TOP,
                        ReasonCode::CapabilityUnverified,
                        format!(
                            "`{}` is backed only by a declaration, not a measurement",
                            evidence.capability()
                        ),
                        evidence_ref,
                    );
                    return;
                }
                match judgement {
                    Judgement::Satisfied => {}
                    Judgement::Violated { detail } => adjudicator.escalate(
                        Tier::TOP,
                        ReasonCode::CapabilityViolated,
                        format!("`{}` failed: {detail}", evidence.capability()),
                        evidence_ref,
                    ),
                    Judgement::Inconclusive { reason } => adjudicator.escalate(
                        Tier::TOP,
                        ReasonCode::CapabilityUnverified,
                        format!("`{}` was inconclusive: {reason}", evidence.capability()),
                        evidence_ref,
                    ),
                }
            }
            Self::Skipped { reason } => match reason {
                SkipReason::Derived { .. } => {}
                SkipReason::Declared {
                    policy_ref,
                    reason: why,
                    owner,
                    expires,
                } => {
                    // Date-to-date, and strict. The waiver's granularity is a
                    // day — `expires` is a civil date, not an instant — so the
                    // committer timestamp is reduced to its UTC civil date
                    // before the two are compared, and the waiver is live
                    // through the whole of the day it names. Strictness is the
                    // sense the ledger message beside it already implies:
                    // "expires 2027-01-01" reads as good on that day.
                    //
                    // Exactly one escalation on either side of the boundary.
                    // Two would put two rows in a ledger that is a bundle
                    // field, for one requirement, which is not what the
                    // ordering guarantees downstream are stated over.
                    let decision = at.utc_date();
                    if decision > *expires {
                        adjudicator.escalate(
                            Tier::TOP,
                            ReasonCode::ExpiredSkip,
                            format!(
                                "waived by {policy_ref} ({why}); owner {owner}. \
                                 That waiver lapsed on {expires} and this commit is \
                                 dated {decision}; an expired waiver authorises \
                                 nothing. Renew it or answer the capability."
                            ),
                            evidence_ref,
                        );
                    } else {
                        adjudicator.escalate(
                            Tier::T1,
                            ReasonCode::DeclaredSkip,
                            format!(
                                "waived by {policy_ref} ({why}); owner {owner}, \
                                 expires {expires}"
                            ),
                            evidence_ref,
                        );
                    }
                }
            },
            Self::Unverified { reason } => adjudicator.escalate(
                Tier::TOP,
                reason.reason_code(),
                reason.detail(),
                evidence_ref,
            ),
        }
    }

    /// The capability this answers, when evidence is present.
    #[must_use]
    pub fn capability(&self) -> Option<&CapabilityId> {
        match self {
            Self::Adopted { evidence, .. } | Self::Ran { evidence, .. } => {
                Some(evidence.capability())
            }
            Self::Skipped { .. } | Self::Unverified { .. } => None,
        }
    }
}

/// Every requirement's outcome, in the one order accounting may use.
///
/// A `BTreeMap` keyed by [`RequirementId`] rather than a `Vec` of pairs, and
/// that choice is the whole of issue #26: with a `Vec`, "account these in the
/// order they finished" is expressible, and the two escalation ledgers it
/// produces are bundle fields. With a map there is no order to choose —
/// [`account_into`](Self::account_into) walks the keys ascending and has no
/// other mode.
///
/// It also gives [`RequirementId`] a second reason to be right: the identifier
/// stops being a label carried alongside a resolution and becomes the key that
/// decides where its escalation lands in the ledger.
///
/// # No `Serialize`
///
/// Deliberate. M3 owns the wire form of a resolution set, and a wire form
/// invented before its consumer exists is a guess that a frozen crate then has
/// to keep. The types inside are serializable; the container is not, yet.
///
/// # No `FromIterator`
///
/// `collect()` would silently swallow a duplicate [`RequirementId`], which is
/// the one thing [`insert`](Self::insert) exists to make visible.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Resolutions(BTreeMap<RequirementId, (Enforcement, CapabilityResolution)>);

impl Resolutions {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record how one requirement resolved, returning whatever it displaced.
    ///
    /// The displaced value is returned rather than dropped so that a duplicate
    /// [`RequirementId`] is *visible*. Two resolutions under one identifier is a
    /// scope collision, and a silent last-wins would resolve it by dropping
    /// whichever arrived first — which, if that was the failing one, is a
    /// fail-open. The caller decides; this type refuses to decide quietly.
    ///
    /// `#[must_use]` is what makes "the caller decides" true rather than
    /// aspirational. Without it `resolutions.insert(id, e, r);` compiles as a
    /// statement and throws the displaced value away, which is the silent
    /// last-wins this method exists to prevent, reintroduced by a semicolon.
    /// Discarding it is still allowed — with `let _ =`, which is a thing a
    /// reviewer can see.
    #[must_use = "the displaced resolution is a scope collision; drop it explicitly \
                  with `let _ =` if that is really what you mean"]
    pub fn insert(
        &mut self,
        requirement: RequirementId,
        enforcement: Enforcement,
        resolution: CapabilityResolution,
    ) -> Option<(Enforcement, CapabilityResolution)> {
        self.0.insert(requirement, (enforcement, resolution))
    }

    /// Account every resolution, ascending by [`RequirementId`].
    ///
    /// **This is the fix for #26.** The iteration order is the map's, so it is
    /// a property of the data rather than of the engine's scheduling, and two
    /// runs over the same resolutions produce byte-identical ledgers.
    ///
    /// Both ledgers are fed by this one pass: `account` routes each resolution
    /// to the enforcing or advisory lane *inside* the call, so each ledger is a
    /// subsequence of one strictly increasing [`RequirementId`] sequence.
    /// There is no second mechanism for the advisory ledger to fall out of step
    /// with, because there is no second mechanism.
    ///
    /// # One decision time per evaluation, by construction
    ///
    /// `at` is the head commit's committer date, and it is forwarded unchanged
    /// to every resolution in the walk. Waiver expiry is the decision that
    /// reads it, and taking it once here rather than once per requirement means
    /// two requirements in one run cannot be judged against two different
    /// dates — the same reason the walk has one order rather than one per
    /// caller.
    pub fn account_into(&self, at: DecisionTime, adjudicators: &mut Adjudicators) {
        for (requirement, (enforcement, resolution)) in &self.0 {
            resolution.account(requirement, *enforcement, at, adjudicators);
        }
    }

    /// The `(state, enforcement)` pairs
    /// [`Confidence::tally`](crate::bundle::Confidence::tally) counts.
    ///
    /// Exposed here so that the ledger and the confidence sentence are built
    /// from the same map. A tally assembled from a separate collection is a
    /// tally that can disagree with the escalations printed beside it.
    pub fn states(&self) -> impl Iterator<Item = (ResolutionState, Enforcement)> + '_ {
        self.0
            .values()
            .map(|(enforcement, resolution)| (resolution.state(), *enforcement))
    }

    /// Every entry, ascending by [`RequirementId`].
    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (&RequirementId, Enforcement, &CapabilityResolution)> {
        self.0
            .iter()
            .map(|(requirement, (enforcement, resolution))| (requirement, *enforcement, resolution))
    }

    /// How many requirements resolved.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether no requirement resolved.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::{EvidenceFacts, ParsedEvidence, Provenance};
    use crate::tier::Verdict;
    use jiff::Timestamp;

    fn requirement() -> RequirementId {
        RequirementId::from_wire("req_tests-pass_00000000000000000000000000000000")
            .expect("a well-formed fixture identifier")
    }

    /// A decision time at midnight UTC on the given civil date.
    ///
    /// Every expiry assertion below is a statement about two civil dates, so
    /// the fixture is written as one; the timestamp is an implementation
    /// detail of getting there. `the_expiry_boundary_is_the_committer_dates_utc_date`
    /// is the exception and builds its own, because the whole point of that
    /// one is a time of day that straddles a zone boundary.
    fn decision_at(year: i16, month: i8, day: i8) -> DecisionTime {
        let at: Timestamp = Date::constant(year, month, day)
            .to_zoned(jiff::tz::TimeZone::UTC)
            .expect("a valid civil date at midnight UTC")
            .timestamp();
        DecisionTime::from_committer_date(at)
    }

    /// The waiver every expiry test below is written against.
    fn waiver(expires: Date) -> CapabilityResolution {
        CapabilityResolution::Skipped {
            reason: SkipReason::Declared {
                policy_ref: PolicyRef {
                    path: ".vibe-check/policy.toml".into(),
                    kind: "skip".into(),
                    id: "macros-no-miri".into(),
                    blob_sha: None,
                },
                reason: "proc-macro crate forbids unsafe".into(),
                owner: "@kono/platform".into(),
                expires,
            },
        }
    }

    /// The single escalation `resolution` produces on the enforcing lane.
    fn only_escalation(
        resolution: &CapabilityResolution,
        at: DecisionTime,
    ) -> crate::adjudicate::Escalation {
        let mut adjudicators = Adjudicators::new();
        resolution.account(
            &requirement(),
            Enforcement::Enforcing,
            at,
            &mut adjudicators,
        );
        let mut escalations = adjudicators.finish().0.into_adjudication().escalations;
        assert_eq!(
            escalations.len(),
            1,
            "one requirement contributes one ledger row, whichever side of the \
             expiry boundary it fell on"
        );
        escalations.remove(0)
    }

    fn evidence_with(provenance: Provenance) -> Box<Evidence> {
        Box::new(Evidence::from_parsed(
            ParsedEvidence::new(
                CapabilityId::new("tests-pass"),
                ParserId::new("junit@1"),
                EvidenceFacts::default(),
            ),
            provenance,
        ))
    }

    fn measured() -> Box<Evidence> {
        evidence_with(Provenance::Executed {
            plan_digest: "blake3:abcd".into(),
            exit_code: 0,
            started_at: Timestamp::UNIX_EPOCH,
            duration_ms: 1,
            toolchain: "1.97.1".into(),
        })
    }

    /// The decision time the resolutions that carry no date are accounted at.
    ///
    /// Only the two waiver rows of `account`'s table read it, so every other
    /// test here is indifferent to its value.
    fn some_decision_time() -> DecisionTime {
        decision_at(2026, 3, 4)
    }

    /// The two tiers this resolution produces under `enforcement`.
    fn tiers_of(resolution: &CapabilityResolution, enforcement: Enforcement) -> (Tier, Tier) {
        tiers_of_at(resolution, enforcement, some_decision_time())
    }

    fn tiers_of_at(
        resolution: &CapabilityResolution,
        enforcement: Enforcement,
        at: DecisionTime,
    ) -> (Tier, Tier) {
        let mut adjudicators = Adjudicators::new();
        resolution.account(&requirement(), enforcement, at, &mut adjudicators);
        let (enforced, advisory) = adjudicators.finish();
        (enforced.tier(), advisory.tier())
    }

    fn verdict_of(resolution: &CapabilityResolution, enforcement: Enforcement) -> Verdict {
        verdict_of_at(resolution, enforcement, some_decision_time())
    }

    fn verdict_of_at(
        resolution: &CapabilityResolution,
        enforcement: Enforcement,
        at: DecisionTime,
    ) -> Verdict {
        let mut adjudicators = Adjudicators::new();
        resolution.account(&requirement(), enforcement, at, &mut adjudicators);
        adjudicators.finish().0.verdict()
    }

    #[test]
    fn satisfied_measured_evidence_leaves_the_verdict_alone() {
        assert_eq!(
            verdict_of(
                &CapabilityResolution::Ran {
                    evidence: measured(),
                    judgement: Judgement::Satisfied,
                },
                Enforcement::Enforcing
            ),
            Verdict::Auto
        );
    }

    #[test]
    fn every_unverified_reason_forces_human_review() {
        // Exhaustive on purpose: a new variant that forgot to escalate would be
        // a silent pass, which is the failure mode this whole design is about.
        let reasons = [
            UnverifiedReason::Unparseable {
                parser: ParserId::new("junit@1"),
                detail: "not xml".into(),
            },
            UnverifiedReason::NoArtifact {
                detail: "check `tests` succeeded but uploaded nothing".into(),
            },
            UnverifiedReason::MissingEvidence,
            UnverifiedReason::StaleArtifact {
                produced_from: "aaaa".into(),
                expected: "bbbb".into(),
            },
            UnverifiedReason::GatesModified {
                paths: vec![".github/workflows/ci.yml".into()],
            },
            UnverifiedReason::BudgetExceeded {
                cost: "extreme".into(),
                max: "high".into(),
            },
            UnverifiedReason::Inconclusive {
                reason: "no data".into(),
            },
            UnverifiedReason::ExecutionFailed {
                detail: "miri not installed".into(),
            },
            UnverifiedReason::PlanFailed {
                detail: "no config".into(),
            },
            UnverifiedReason::UnknownCapability {
                id: "loom-clean".into(),
            },
            UnverifiedReason::UnknownParser {
                id: "loom-json@1".into(),
            },
            UnverifiedReason::NoForge,
            UnverifiedReason::SchemaTooNew {
                found: 9,
                supported: 1,
            },
            UnverifiedReason::Oversized {
                detail: "72 MiB > 64 MiB".into(),
            },
        ];
        for reason in reasons {
            let detail = reason.detail();
            assert_eq!(
                verdict_of(
                    &CapabilityResolution::Unverified {
                        reason: reason.clone()
                    },
                    Enforcement::Enforcing
                ),
                Verdict::Human,
                "{reason:?} must escalate"
            );
            assert!(!detail.is_empty(), "{reason:?} must explain itself");
        }
    }

    #[test]
    fn a_green_check_with_no_artifact_says_so_in_the_message() {
        // The message is the product here: a maintainer seeing this needs to
        // understand that their passing job was not enough and why.
        let reason = UnverifiedReason::NoArtifact {
            detail: "check `tests` succeeded but uploaded nothing".into(),
        };
        let detail = reason.detail();
        assert!(detail.contains("not evidence"), "{detail}");
        assert!(detail.contains("parseable"), "{detail}");
    }

    #[test]
    fn an_inconclusive_answer_is_not_a_pass() {
        assert_eq!(
            verdict_of(
                &CapabilityResolution::Ran {
                    evidence: measured(),
                    judgement: Judgement::Inconclusive {
                        reason: "added tests do not compile against base".into()
                    },
                },
                Enforcement::Enforcing
            ),
            Verdict::Human
        );
    }

    #[test]
    fn a_declaration_cannot_satisfy_a_capability() {
        // Even claiming Satisfied: the provenance is checked before the
        // judgement, so writing "this is fine" in policy is not a way through.
        assert_eq!(
            verdict_of(
                &CapabilityResolution::Adopted {
                    evidence: evidence_with(Provenance::Declared {
                        by: "policy#skip:x".into(),
                        reason: "trust me".into(),
                    }),
                    judgement: Judgement::Satisfied,
                },
                Enforcement::Enforcing
            ),
            Verdict::Human
        );
    }

    #[test]
    fn derived_skips_are_free_and_declared_waivers_are_not() {
        let derived = CapabilityResolution::Skipped {
            reason: SkipReason::Derived {
                detail: "no unsafe in changed hunks".into(),
            },
        };
        assert_eq!(verdict_of(&derived, Enforcement::Enforcing), Verdict::Auto);

        // A human waiver is reviewable, not free — and the decision time is
        // named rather than left to a default, because `InterfaceReview` is the
        // answer only while the waiver is live. Accounted a day after
        // 2027-01-01 this same fixture is `Human`, and a test asserting
        // `InterfaceReview` without saying when would be asserting the right
        // thing for the wrong reason.
        let declared = waiver(Date::constant(2027, 1, 1));
        assert_eq!(
            verdict_of_at(&declared, Enforcement::Enforcing, decision_at(2026, 6, 1)),
            Verdict::InterfaceReview
        );
    }

    #[test]
    fn an_expired_waiver_costs_more_than_a_live_one() {
        // The defect this pair exists for: `expires` used to be interpolated
        // into a message and compared against nothing, so a waiver three years
        // dead and one written yesterday produced the same tier, the same
        // reason code, and the same verdict.
        let declared = waiver(Date::constant(2027, 1, 1));

        let live = decision_at(2026, 6, 1);
        let expired = decision_at(2027, 6, 1);

        assert_eq!(
            verdict_of_at(&declared, Enforcement::Enforcing, live),
            Verdict::InterfaceReview
        );
        assert_eq!(
            verdict_of_at(&declared, Enforcement::Enforcing, expired),
            Verdict::Human,
            "an expired waiver authorises nothing, so the capability is \
             unanswered and a human has to look"
        );

        // Both halves matter. The verdict is what an exit code derives from;
        // the reason code is what a maintainer reads to find out that renewing
        // the waiver — rather than answering the capability — is the cheap fix.
        assert_eq!(
            only_escalation(&declared, live).reason,
            ReasonCode::DeclaredSkip
        );
        let escalated = only_escalation(&declared, expired);
        assert_eq!(escalated.reason, ReasonCode::ExpiredSkip);
        assert_eq!(escalated.to, Tier::TOP);
        assert!(
            escalated.detail.contains("2027-01-01") && escalated.detail.contains("2027-06-01"),
            "the message names both the expiry and the commit's date: {}",
            escalated.detail
        );
    }

    #[test]
    fn expiry_is_measured_against_the_committer_date_and_not_the_wall_clock() {
        // The load-bearing one, and it is written so that no clock-reading
        // implementation can pass it in either direction.
        //
        // A waiver that lapsed in 2020, evaluated at a decision time in 2019,
        // is live — under any wall clock this machine will ever have, it is
        // unconditionally dead. Re-running a three-year-old pull request has to
        // give the verdict it had, or the replay property is prose.
        let ancient = waiver(Date::constant(2020, 1, 1));
        assert_eq!(
            verdict_of_at(&ancient, Enforcement::Enforcing, decision_at(2019, 6, 1)),
            Verdict::InterfaceReview,
            "the committer date is 2019, and in 2019 this waiver had six months left"
        );

        // The mirror, which a "clock, but clamped" implementation still fails:
        // a waiver good until 2099, evaluated at a decision time in 2100, is
        // expired. No wall clock reaches 2100.
        let distant = waiver(Date::constant(2099, 1, 1));
        assert_eq!(
            verdict_of_at(&distant, Enforcement::Enforcing, decision_at(2100, 1, 1)),
            Verdict::Human,
            "the committer date is 2100, and by 2100 this waiver is a year dead"
        );
    }

    #[test]
    fn the_expiry_boundary_is_the_committer_dates_utc_date() {
        // A commit half an hour before midnight UTC on the expiry day. In UTC
        // that is 2026-12-31 and the waiver is live; a runner at UTC+13 would
        // call the same instant 2027-01-01 and, on a `>=` reading, kill it.
        // Pinning the accessor to UTC is what makes one commit have one date.
        //
        // No `TZ` mutation to demonstrate the second runner: the environment is
        // process-global and these tests run in one process. The assertion is
        // that the outcome follows from the *UTC* date, which is a claim about
        // this code and not about the machine running it.
        let at: Timestamp = "2026-12-31T23:30:00Z"
            .parse()
            .expect("a well-formed fixture timestamp");
        let decision = DecisionTime::from_committer_date(at);
        assert_eq!(decision.utc_date(), Date::constant(2026, 12, 31));

        let declared = waiver(Date::constant(2026, 12, 31));
        assert_eq!(
            verdict_of_at(&declared, Enforcement::Enforcing, decision),
            Verdict::InterfaceReview,
            "the UTC date is still the expiry day, so the waiver is still live"
        );
        assert_eq!(
            only_escalation(&declared, decision).reason,
            ReasonCode::DeclaredSkip
        );
    }

    #[test]
    fn the_expiry_boundary_is_inclusive_of_the_expiry_day() {
        // Named for the answer so that a later refactor cannot flip `>` to `>=`
        // and stay green. The ledger message renders "expires 2027-01-01",
        // which a reader takes to mean the waiver is good on that day; the
        // comparison must agree with the sentence printed beside it.
        let declared = waiver(Date::constant(2027, 1, 1));

        assert_eq!(
            only_escalation(&declared, decision_at(2027, 1, 1)).reason,
            ReasonCode::DeclaredSkip,
            "on the expiry day itself the waiver is live"
        );
        assert_eq!(
            only_escalation(&declared, decision_at(2027, 1, 2)).reason,
            ReasonCode::ExpiredSkip,
            "and the day after, it is not"
        );
    }

    #[test]
    fn an_advisory_failure_does_not_move_the_enforced_tier() {
        let violated = CapabilityResolution::Ran {
            evidence: measured(),
            judgement: Judgement::Violated {
                detail: "2 tests failed".into(),
            },
        };
        let (enforced, _) = tiers_of(&violated, Enforcement::Advisory);
        assert_eq!(enforced, Tier::BOTTOM);
        assert_eq!(enforced.verdict(), Verdict::Auto);
    }

    #[test]
    fn an_advisory_failure_does_move_the_advisory_tier() {
        // The other half: advisory is not "ignored", it is "recorded elsewhere".
        // If it were dropped, `core.tier == t0` would be indistinguishable from
        // "nothing that failed counted", which is the measurement the
        // escape-rate loop exists to make.
        let violated = CapabilityResolution::Ran {
            evidence: measured(),
            judgement: Judgement::Violated {
                detail: "2 tests failed".into(),
            },
        };
        let (_, advisory) = tiers_of(&violated, Enforcement::Advisory);
        assert_eq!(advisory, Tier::TOP);
    }

    #[test]
    fn a_policy_integrity_fact_is_never_advisory() {
        // The two-token gate disable, refused. A requirement naming a capability
        // this build cannot evaluate escalates the *enforced* ledger whatever
        // its `enforcement` says, because the failure is in the policy and not
        // in the code the policy was asked about.
        //
        // Table-driven over both policy-integrity variants and both enforcement
        // values, so the advisory case cannot be the one that was forgotten.
        let integrity = [
            UnverifiedReason::UnknownCapability {
                id: "tetss-pass".into(),
            },
            UnverifiedReason::UnknownParser {
                id: "junti@1".into(),
            },
        ];

        for reason in integrity {
            assert!(reason.is_policy_integrity(), "{reason:?}");
            for enforcement in [Enforcement::Enforcing, Enforcement::Advisory] {
                let resolution = CapabilityResolution::Unverified {
                    reason: reason.clone(),
                };
                let (enforced, advisory) = tiers_of(&resolution, enforcement);
                assert_eq!(
                    enforced,
                    Tier::TOP,
                    "{reason:?} under {enforcement:?} must escalate the enforced tier"
                );
                assert_eq!(
                    enforced.verdict(),
                    Verdict::Human,
                    "{reason:?} under {enforcement:?} must demand a human"
                );
                assert_eq!(
                    advisory,
                    Tier::BOTTOM,
                    "{reason:?} must not reach the advisory ledger at all"
                );
            }
        }
    }

    #[test]
    fn every_other_unverified_reason_is_a_result_not_a_policy_fact() {
        // The classification's other side. A parse failure is a fact about the
        // code's evidence and may legitimately be advisory; misfiling one as
        // policy integrity would quietly make advisory mean nothing.
        for reason in [
            UnverifiedReason::MissingEvidence,
            UnverifiedReason::NoForge,
            UnverifiedReason::Inconclusive {
                reason: "no data".into(),
            },
            // #46: `GatesModified` being overrulable by `enforcement = "advisory"`
            // is a deferred decision, not a settled one. Evidence from a workflow
            // the change itself modifies is attacker-controlled, and the argument
            // for admitting it to the advisory lane is that the change also trips
            // gate-integrity by touching `.vibe-check/`. #46 owns revisiting that;
            // this assertion records today's behaviour and must not be read as
            // endorsing it.
            UnverifiedReason::GatesModified {
                paths: vec![".github/workflows/ci.yml".into()],
            },
        ] {
            assert!(!reason.is_policy_integrity(), "{reason:?}");
            let (enforced, advisory) = tiers_of(
                &CapabilityResolution::Unverified { reason },
                Enforcement::Advisory,
            );
            assert_eq!(enforced, Tier::BOTTOM);
            assert_eq!(advisory, Tier::TOP);
        }
    }

    #[test]
    fn gate_paths_are_reported_in_a_stable_order() {
        // #26 on the display side. These paths come from walking a diff, and
        // this sentence ends up in a bundle field; two runs of the same commit
        // must not write two different strings. Sorted on a local copy, so the
        // variant's own field still holds exactly what the caller put in it —
        // display-only, no contract change.
        let unsorted = UnverifiedReason::GatesModified {
            paths: vec![
                ".github/workflows/release.yml".into(),
                ".github/workflows/ci.yml".into(),
            ],
        };
        let sorted = UnverifiedReason::GatesModified {
            paths: vec![
                ".github/workflows/ci.yml".into(),
                ".github/workflows/release.yml".into(),
            ],
        };
        assert_eq!(unsorted.detail(), sorted.detail());
        assert!(
            unsorted
                .detail()
                .contains(".github/workflows/ci.yml, .github/workflows/release.yml"),
            "{}",
            unsorted.detail()
        );

        let UnverifiedReason::GatesModified { paths } = &unsorted else {
            panic!("constructed as `GatesModified`")
        };
        assert_eq!(paths[0], ".github/workflows/release.yml");
    }

    #[test]
    fn least_confident_wins_when_aggregating_across_crates() {
        let mut states = [
            ResolutionState::Run,
            ResolutionState::Unverified,
            ResolutionState::Adopt,
        ];
        states.sort_by_key(|s| s.confidence_rank());
        assert_eq!(states[0], ResolutionState::Unverified);
    }

    #[test]
    fn collapsing_a_capability_keeps_the_least_confident_scope() {
        // The case the frozen `capability_states` key forces: one capability,
        // two scopes, one map entry. The entry must be the unverified one.
        assert_eq!(
            ResolutionState::Adopt.collapse(ResolutionState::Unverified),
            ResolutionState::Unverified
        );
        assert_eq!(
            ResolutionState::collapse_all([
                ResolutionState::Run,
                ResolutionState::Skip,
                ResolutionState::Adopt,
            ]),
            Some(ResolutionState::Skip)
        );
    }

    #[test]
    fn the_confidence_order_is_not_the_declaration_order() {
        // Declaration order runs `Adopt` < `Run` < `Skip` < `Unverified`, so a
        // derived `Ord` would make `Adopt` the minimum of the pair below and
        // report the scope that passed for a capability whose other scope went
        // unanswered. `ResolutionState` therefore has no derived ordering —
        // that `min` does not compile — and this pins the order that replaced
        // it. `tests/nothing_orders_a_resolution_state.rs` keeps the derive off.
        let by_confidence = [
            ResolutionState::Unverified,
            ResolutionState::Skip,
            ResolutionState::Adopt,
            ResolutionState::Run,
        ];
        let mut ranks = by_confidence.map(ResolutionState::confidence_rank);
        ranks.sort_unstable();
        assert_eq!(
            ranks,
            by_confidence.map(ResolutionState::confidence_rank),
            "the four states are listed least-confident first"
        );
        assert_eq!(
            ResolutionState::collapse_all([ResolutionState::Adopt, ResolutionState::Unverified]),
            Some(ResolutionState::Unverified)
        );
    }

    #[test]
    fn a_state_is_the_answering_method_and_not_the_answer() {
        // `Run` outranks `Adopt`, so a capability whose only real run *failed*
        // collapses to `adopt`. That is the map saying how the question was
        // answered, not how it turned out — the failing run escalated through
        // the adjudicator, which is where outcomes live. Anything that reads
        // `capability_states` as a pass/fail table reads this backwards.
        let ran_and_failed = CapabilityResolution::Ran {
            evidence: measured(),
            judgement: Judgement::Violated {
                detail: "2 tests failed".into(),
            },
        };
        assert_eq!(ran_and_failed.state(), ResolutionState::Run);
        assert_eq!(
            ResolutionState::collapse_all([ran_and_failed.state(), ResolutionState::Adopt]),
            Some(ResolutionState::Adopt),
            "the least confident *method* wins; the failing run is not the \
             least confident thing here"
        );
    }

    #[test]
    fn a_capability_nothing_required_collapses_to_nothing() {
        assert_eq!(ResolutionState::collapse_all([]), None);
    }
}
