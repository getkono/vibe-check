//! Advisory requirements, expressed as a second adjudicator.
//!
//! # Why two accumulators and not a ceiling
//!
//! "Advisory" reads like "cap this requirement's contribution at `T0`", and a
//! cap is a *downward* operation. There is no such operation anywhere in this
//! system, deliberately: [`Adjudicator::escalate`] is the only mutator, and the
//! note on [`Tier::TOP`] explains why even `gate-integrity`'s "cap at human
//! review" is an ordinary escalation rather than ceiling machinery. Adding a
//! ceiling here would make lowering a tier expressible for the first time.
//!
//! So advisory is not a weaker escalation. It is an escalation of a *different
//! ledger*. [`Adjudicators`] holds two [`Adjudicator`]s that never observe each
//! other; each one is the same append-only, only-rises accumulator it was
//! before, and `accumulator.rs` needs no change at all to support this.
//!
//! # Why routing lives in `account`
//!
//! [`CapabilityResolution::account`](crate::resolution::CapabilityResolution::account)
//! earns its safety argument from being the single consumer of a resolution,
//! and therefore the single place the fail-closed rules have to be correct.
//! Choosing a lane *is* one of those rules — a policy-integrity fact is never
//! advisory, whatever the requirement asked for — so the choice is made there,
//! next to every other rule, rather than at call sites where it would have to be
//! audited one by one.
//!
//! That is why [`Adjudicators::route`] is `pub(crate)` and not `pub`. Outside
//! this crate the only obtainable `&mut Adjudicator` is
//! [`Adjudicators::integrity`], which is the enforcing lane; a downstream crate
//! cannot express "resolve this unknown capability against the advisory lane",
//! because it cannot name the advisory lane at all.
//!
//! # Why this module has no children
//!
//! `enforcing` and `advisory` are private fields of the same type, so a child
//! module could swap them, or hand out `&mut self.advisory` under another name.
//! Either is a downward operation wearing a different word. The guard lives in
//! `tests/accumulator_invariants.rs`, next to the one protecting the
//! accumulator itself.

use serde::{Deserialize, Serialize};

use super::{Adjudication, Adjudicator, Escalation};
use crate::tier::{Tier, Verdict};

/// Whether a requirement's outcome may raise the enforced tier.
///
/// An enum rather than a `bool`: M5 needs `AdvisoryWhenInconclusive` for
/// test-negation, and a `bool` cannot grow that.
///
/// Deliberately **not** `#[non_exhaustive]`, for the reason given on
/// [`ReasonCode`](crate::reason::ReasonCode): this enum is internal to our own
/// crates, and adding a variant *should* break every match arm until each site
/// has been reconsidered. When `AdvisoryWhenInconclusive` arrives, the compiler
/// is the checklist.
///
/// There is no `Default`. A requirement that forgot to say which it is would
/// default to whichever the author of that `impl` found convenient, and one of
/// the two choices silently disables a gate.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Enforcement {
    /// The outcome may raise the enforced tier, and so the verdict.
    Enforcing,
    /// The outcome is recorded and reported, but only in the advisory ledger.
    ///
    /// Note this is a property of a *requirement*, not of the run. The
    /// repository-wide `mode` key is a different mechanism, applied to the
    /// check-run conclusion after adjudication, and the two compose.
    Advisory,
}

impl Enforcement {
    /// The stable wire string for this value.
    ///
    /// Must agree with the serde representation; a test below asserts it does.
    /// The renderer prints this and policy documents are parsed into it, so a
    /// drift between the two would be a policy key that silently stops matching.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enforcing => "enforcing",
            Self::Advisory => "advisory",
        }
    }
}

/// The two independent ledgers a run accumulates into.
///
/// The enforcing ledger becomes the verdict. The advisory ledger becomes
/// [`BundleCore::advisory_tier`](crate::bundle::BundleCore::advisory_tier) and
/// [`EvidenceBundle::advisory_escalations`](crate::bundle::EvidenceBundle::advisory_escalations),
/// and never feeds back into the tier.
///
/// There is intentionally no `Default`, for the same reason
/// [`Adjudicator`] has none.
#[derive(Debug)]
pub struct Adjudicators {
    /// The ledger that becomes the verdict.
    enforcing: Adjudicator,
    /// The ledger that is reported and never enforced.
    advisory: Adjudicator,
}

impl Adjudicators {
    /// A fresh pair, both at [`Tier::BOTTOM`].
    // Allowed rather than satisfied, and scoped to this constructor so a second
    // one cannot inherit the exemption — the same reasoning as
    // `Adjudicator::new`, which this type is a pair of.
    #[allow(clippy::new_without_default)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            enforcing: Adjudicator::new(),
            advisory: Adjudicator::new(),
        }
    }

    /// The ledger an outcome with this enforcement belongs in.
    ///
    /// `pub(crate)` on purpose. The routing decision belongs to `account`, which
    /// applies the policy-integrity override first; a caller that could pick a
    /// lane directly could pick the advisory one for an unknown capability, and
    /// that is a two-token gate disable. Guarded by `routing_is_not_public`.
    pub(crate) fn route(&mut self, enforcement: Enforcement) -> &mut Adjudicator {
        match enforcement {
            Enforcement::Enforcing => &mut self.enforcing,
            Enforcement::Advisory => &mut self.advisory,
        }
    }

    /// The ledger for facts about the policy itself: always the enforcing one.
    ///
    /// An identifier this build cannot resolve is a fact about the *policy*, not
    /// a result about the code, and policy integrity is never advisory. This is
    /// the only `&mut Adjudicator` reachable from outside this crate, and it is
    /// what [`Known::get`](crate::known::Known::get) is fed — so `known.rs`'s
    /// hard-coded [`Tier::TOP`] stays correct and stays untouched.
    pub fn integrity(&mut self) -> &mut Adjudicator {
        &mut self.enforcing
    }

    /// The enforced tier so far. Read-only, for progress reporting.
    #[must_use]
    pub fn enforced_tier(&self) -> Tier {
        self.enforcing.tier()
    }

    /// The advisory tier so far. Read-only, for progress reporting.
    #[must_use]
    pub fn advisory_tier(&self) -> Tier {
        self.advisory.tier()
    }

    /// Finish both ledgers.
    ///
    /// The two results are distinct types rather than a pair of the same type,
    /// so a call site cannot bind them transposed and write the advisory tier
    /// into [`BundleCore::tier`](crate::bundle::BundleCore::tier).
    #[must_use]
    pub fn finish(self) -> (EnforcedAdjudication, AdvisoryAdjudication) {
        let advisory = self.advisory.finish();
        (
            EnforcedAdjudication {
                inner: self.enforcing.finish(),
            },
            AdvisoryAdjudication {
                tier: advisory.tier,
                escalations: advisory.escalations,
            },
        )
    }
}

/// A finished adjudication that is allowed to become a verdict.
///
/// Produced only by [`Adjudicators::finish`]. This is the only type from which
/// [`BundleCore::tier`](crate::bundle::BundleCore::tier) and
/// [`BundleCore::verdict`](crate::bundle::BundleCore::verdict) are written.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EnforcedAdjudication {
    /// The finished enforcing ledger.
    inner: Adjudication,
}

impl EnforcedAdjudication {
    /// The enforced tier.
    #[must_use]
    pub fn tier(&self) -> Tier {
        self.inner.tier
    }

    /// The verdict, derived from the enforced tier.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        self.inner.verdict
    }

    /// The full adjudication, for rendering.
    #[must_use]
    pub fn adjudication(&self) -> &Adjudication {
        &self.inner
    }

    /// Take the adjudication, for the bundle's `adjudication` field.
    #[must_use]
    pub fn into_adjudication(self) -> Adjudication {
        self.inner
    }
}

/// A finished advisory ledger: a tier and its escalations, and no verdict.
///
/// Deliberately not an [`Adjudication`] and deliberately without a `verdict`
/// accessor. [`Adjudicator::finish`] always sets `verdict: tier.verdict()`, so
/// exposing the inner adjudication would put a second, more permissive verdict
/// within reach of a bundle that must carry exactly one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AdvisoryAdjudication {
    /// The tier the advisory ledger reached.
    tier: Tier,
    /// Every advisory escalation, in order.
    escalations: Vec<Escalation>,
}

impl AdvisoryAdjudication {
    /// The tier the advisory-routed outcomes reached **on their own**.
    ///
    /// Not the counterfactual by itself: this ledger never sees the enforcing
    /// lane's escalations, so the tier that would have been enforced had every
    /// requirement been enforcing is `enforced.tier().join(advisory.tier())`.
    /// See `the_advisory_tier_is_not_the_counterfactual_on_its_own`.
    #[must_use]
    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// Every advisory escalation, in order.
    #[must_use]
    pub fn escalations(&self) -> &[Escalation] {
        &self.escalations
    }

    /// Take the escalations, for the bundle's `advisory_escalations` field.
    #[must_use]
    pub fn into_escalations(self) -> Vec<Escalation> {
        self.escalations
    }

    /// The advisory escalations that actually raised the advisory tier.
    pub fn raising(&self) -> impl Iterator<Item = &Escalation> {
        self.escalations.iter().filter(|e| e.raised_tier())
    }

    /// How many advisory escalations were recorded.
    ///
    /// The headline number: *"3 requirements were advisory and 1 of them
    /// failed"* is built from this and [`raising`](Self::raising).
    #[must_use]
    pub fn count(&self) -> usize {
        self.escalations.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::{Evidence, EvidenceFacts, ParsedEvidence, Provenance};
    use crate::ids::{CapabilityId, ParserId, RequirementId};
    use crate::known::{Known, UnknownKind};
    use crate::reason::{EvidenceRef, PolicyRef, ReasonCode};
    use crate::resolution::{CapabilityResolution, Judgement, SkipReason, UnverifiedReason};
    use jiff::Timestamp;
    use jiff::civil::Date;
    use proptest::prelude::*;

    fn requirement() -> RequirementId {
        RequirementId::from_wire("req_tests-pass_00000000000000000000000000000000")
            .expect("a well-formed fixture identifier")
    }

    fn measured() -> Box<Evidence> {
        Box::new(Evidence::from_parsed(
            ParsedEvidence::new(
                CapabilityId::new("tests-pass"),
                ParserId::new("junit@1"),
                EvidenceFacts::default(),
            ),
            Provenance::Executed {
                plan_digest: "blake3:abcd".into(),
                exit_code: 0,
                started_at: Timestamp::UNIX_EPOCH,
                duration_ms: 1,
                toolchain: "1.97.1".into(),
            },
        ))
    }

    /// Resolutions that are *not* facts about the policy, covering all three
    /// escalation outcomes: nothing, `T1`, and `TOP`.
    fn any_non_integrity_resolution() -> impl Strategy<Value = CapabilityResolution> {
        prop_oneof![
            Just(CapabilityResolution::Ran {
                evidence: measured(),
                judgement: Judgement::Satisfied,
            }),
            Just(CapabilityResolution::Ran {
                evidence: measured(),
                judgement: Judgement::Violated {
                    detail: "2 failures".into(),
                },
            }),
            Just(CapabilityResolution::Skipped {
                reason: SkipReason::Derived {
                    detail: "no unsafe in changed hunks".into(),
                },
            }),
            Just(CapabilityResolution::Skipped {
                reason: SkipReason::Declared {
                    policy_ref: PolicyRef {
                        path: ".vibe-check/policy.toml".into(),
                        kind: "skip".into(),
                        id: "macros-no-miri".into(),
                        blob_sha: None,
                    },
                    reason: "proc-macro crate forbids unsafe".into(),
                    owner: "@kono/platform".into(),
                    expires: Date::constant(2027, 1, 1),
                },
            }),
            Just(CapabilityResolution::Unverified {
                reason: UnverifiedReason::MissingEvidence,
            }),
        ]
    }

    fn any_enforcement() -> impl Strategy<Value = Enforcement> {
        prop_oneof![Just(Enforcement::Enforcing), Just(Enforcement::Advisory)]
    }

    fn account_all(pairs: &[(CapabilityResolution, Enforcement)]) -> Adjudicators {
        let mut adjudicators = Adjudicators::new();
        for (resolution, enforcement) in pairs {
            resolution.account(&requirement(), *enforcement, &mut adjudicators);
        }
        adjudicators
    }

    #[test]
    fn enforcement_matches_its_wire_form() {
        // `as_str` and the serde representation must agree: policy documents are
        // parsed into this and the renderer prints it, so a drift would be a key
        // that silently stops matching.
        for enforcement in [Enforcement::Enforcing, Enforcement::Advisory] {
            let json = serde_json::to_string(&enforcement).expect("serialize");
            assert_eq!(json, format!(r#""{}""#, enforcement.as_str()));
        }
    }

    #[test]
    fn both_ledgers_start_at_the_bottom() {
        let adjudicators = Adjudicators::new();
        assert_eq!(adjudicators.enforced_tier(), Tier::BOTTOM);
        assert_eq!(adjudicators.advisory_tier(), Tier::BOTTOM);
    }

    #[test]
    fn integrity_is_the_enforcing_lane() {
        let mut adjudicators = Adjudicators::new();
        adjudicators.integrity().escalate(
            Tier::T1,
            ReasonCode::UnknownCapability,
            "policy names something this build cannot evaluate",
            EvidenceRef::Unattributed,
        );
        assert_eq!(adjudicators.enforced_tier(), Tier::T1);
        assert_eq!(adjudicators.advisory_tier(), Tier::BOTTOM);
    }

    #[test]
    fn an_unknown_identifier_escalates_the_enforced_ledger() {
        // The `known.rs` half of the two-token gate disable. `integrity()` is the
        // only `&mut Adjudicator` a caller outside this crate can obtain, so
        // there is no advisory lane for an unresolved identifier to reach.
        let mut adjudicators = Adjudicators::new();
        let unknown = Known::<CapabilityId>::unresolved(
            "tetss-pass",
            UnknownKind::Capability,
            EvidenceRef::Unattributed,
        );

        assert!(unknown.get(adjudicators.integrity()).is_none());

        let (enforced, advisory) = adjudicators.finish();
        assert_eq!(enforced.tier(), Tier::TOP);
        assert_eq!(enforced.verdict(), Verdict::Human);
        assert_eq!(advisory.tier(), Tier::BOTTOM);
        assert_eq!(advisory.count(), 0);
    }

    #[test]
    fn an_advisory_policy_integrity_fact_still_raises_the_enforced_tier() {
        // The paired half of `advisory_pairs_do_not_move_the_enforced_tier`.
        // Containment holds over resolutions that are results about the code;
        // this is the sequence that is deliberately *not* contained, and without
        // it the property above would read as "advisory can never escalate".
        let pairs = [(
            CapabilityResolution::Unverified {
                reason: UnverifiedReason::UnknownCapability {
                    id: "tetss-pass".into(),
                },
            },
            Enforcement::Advisory,
        )];

        let (enforced, advisory) = account_all(&pairs).finish();
        assert_eq!(enforced.tier(), Tier::TOP);
        assert_eq!(enforced.verdict(), Verdict::Human);
        assert_eq!(advisory.tier(), Tier::BOTTOM);

        // Deleting the advisory pair leaves nothing, which is exactly why the
        // containment property must exclude policy-integrity resolutions.
        let (deleted, _) = account_all(&[]).finish();
        assert_eq!(deleted.tier(), Tier::BOTTOM);
    }

    #[test]
    fn the_advisory_tier_is_not_the_counterfactual_on_its_own() {
        // The advisory tier is the join of the advisory-routed outcomes and
        // nothing else. It is tempting to document it as "what would have been
        // enforced had every requirement been enforcing", and that is false:
        // this ledger never sees the enforcing lane.
        //
        // Here the enforcing lane reaches T1 on a declared waiver while the
        // advisory lane stays at BOTTOM. Read `advisory_tier` as the
        // counterfactual and you get T0 — less scrutiny than was actually
        // enforced, which is not a coherent reading of "had everything counted".
        //
        // This matters far beyond a doc comment: `advisory_tier` is in the
        // frozen core, the escape-rate loop reads it across a repository's whole
        // history, and a consumer that implements "what would we have blocked"
        // as `advisory_tier` undercounts every run whose enforcing lane was
        // louder than its advisory one.
        let pairs = [
            (
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
                        expires: Date::constant(2027, 1, 1),
                    },
                },
                Enforcement::Enforcing,
            ),
            (
                CapabilityResolution::Ran {
                    evidence: measured(),
                    judgement: Judgement::Satisfied,
                },
                Enforcement::Advisory,
            ),
        ];

        let (enforced, advisory) = account_all(&pairs).finish();

        assert_eq!(enforced.tier(), Tier::T1);
        assert_eq!(
            advisory.tier(),
            Tier::BOTTOM,
            "the advisory lane saw only a satisfied requirement"
        );
        assert_eq!(
            enforced.tier().join(advisory.tier()),
            Tier::T1,
            "the counterfactual is the join of the two, and it is never below `tier`"
        );
    }

    proptest! {
        /// The counterfactual is never below what was actually enforced. Stated
        /// as a property because the false reading — `advisory_tier` alone — is
        /// exactly the one that violates it, and it violates it silently.
        #[test]
        fn the_counterfactual_is_never_below_the_enforced_tier(
            pairs in prop::collection::vec(
                (any_non_integrity_resolution(), any_enforcement()),
                0..24,
            )
        ) {
            let (enforced, advisory) = account_all(&pairs).finish();
            let counterfactual = enforced.tier().join(advisory.tier());

            prop_assert!(counterfactual >= enforced.tier());
            prop_assert!(counterfactual >= advisory.tier());
        }
    }

    #[test]
    fn the_advisory_ledger_records_what_it_did_not_enforce() {
        let pairs = [(
            CapabilityResolution::Ran {
                evidence: measured(),
                judgement: Judgement::Violated {
                    detail: "2 failures".into(),
                },
            },
            Enforcement::Advisory,
        )];

        let (enforced, advisory) = account_all(&pairs).finish();
        assert_eq!(enforced.tier(), Tier::BOTTOM);
        assert!(enforced.adjudication().escalations.is_empty());
        assert_eq!(advisory.tier(), Tier::TOP);
        assert_eq!(advisory.count(), 1);
        assert_eq!(advisory.raising().count(), 1);
        assert_eq!(
            advisory.escalations()[0].reason,
            ReasonCode::CapabilityViolated
        );
    }

    proptest! {
        /// Advisory containment, stated so it can fail: over resolutions that are
        /// results about the *code*, the enforced tier is exactly the tier of the
        /// same sequence with every advisory pair deleted.
        ///
        /// Restricted to non-policy-integrity resolutions on purpose — see
        /// `an_advisory_policy_integrity_fact_still_raises_the_enforced_tier`,
        /// which is the other half of the statement.
        #[test]
        fn advisory_pairs_do_not_move_the_enforced_tier(
            pairs in prop::collection::vec(
                (any_non_integrity_resolution(), any_enforcement()),
                0..24,
            )
        ) {
            let all = account_all(&pairs);

            // `account` speaks `Adjudicators`, so the deleted subsequence is fed
            // to a second pair whose advisory lane is never touched. Its
            // enforcing lane is a bare `Adjudicator` reached the only way there
            // is to reach one.
            let kept: Vec<_> = pairs
                .into_iter()
                .filter(|(_, enforcement)| *enforcement == Enforcement::Enforcing)
                .collect();
            let deleted = account_all(&kept);

            prop_assert_eq!(all.enforced_tier(), deleted.enforced_tier());
            prop_assert_eq!(deleted.advisory_tier(), Tier::BOTTOM);
        }

        /// Neither ledger depends on the order outcomes arrived in. Capabilities
        /// resolve concurrently, so order is not something we control.
        #[test]
        fn both_ledgers_are_order_independent(
            mut pairs in prop::collection::vec(
                (any_non_integrity_resolution(), any_enforcement()),
                0..16,
            )
        ) {
            let forward = account_all(&pairs);
            pairs.reverse();
            let backward = account_all(&pairs);

            prop_assert_eq!(forward.enforced_tier(), backward.enforced_tier());
            prop_assert_eq!(forward.advisory_tier(), backward.advisory_tier());
        }
    }
}
