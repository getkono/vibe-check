//! The escalation accumulator.
//!
//! # Why this module has no children
//!
//! Rust field privacy is *module*-scoped, not type-scoped. A private field is
//! reachable from the module that declares it and from every descendant module.
//! So the guarantee below — that [`Adjudicator::tier`] can only be changed by
//! [`Adjudicator::escalate`] — holds exactly as long as this module has no child
//! modules other than `tests`.
//!
//! **Do not add submodules here.** If this file needs to grow, put the new code
//! in a sibling under `adjudicate/` instead, where it will have to go through
//! the public API like every other caller.

use serde::{Deserialize, Serialize};

use crate::reason::{EvidenceRef, ReasonCode};
use crate::tier::{Tier, Verdict};

/// One recorded rise in scrutiny.
///
/// # Reconstruction is checked
///
/// [`Adjudicator::escalate`] is the only *producer*, and it computes
/// `to = from.join(at_least)`, so `to >= from` for every escalation it writes.
/// `Deserialize` is hand-written rather than derived so that reading one back
/// out of a bundle admits exactly the values `escalate` could have written. See
/// the module note on [`ReplayError`] for why a violating document is refused
/// rather than repaired.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[non_exhaustive]
pub struct Escalation {
    /// The tier before this escalation.
    ///
    /// A statement about the *sequence*, and the reason a finished ledger is
    /// never sorted: re-order the entries and this field stops agreeing with
    /// the previous entry's [`to`](Self::to), so the ledger no longer replays
    /// to the tier it reports. Determinism is bought on the accounting input
    /// instead — see [`Resolutions`](crate::resolution::Resolutions).
    pub from: Tier,
    /// The tier after it.
    ///
    /// May equal `from`. A second, independent reason for human review does not
    /// raise the tier any further, but it is still a reason the verdict is what
    /// it is, and dropping it would make the comment claim a single cause when
    /// there were three.
    pub to: Tier,
    /// Why, as a stable groupable code.
    pub reason: ReasonCode,
    /// Why, in a sentence a human can act on.
    pub detail: String,
    /// What this points at.
    pub evidence: EvidenceRef,
}

impl Escalation {
    /// Whether this escalation actually raised the tier, as opposed to
    /// restating a level already reached.
    ///
    /// Renderers use this to lead with the first cause while still listing the
    /// rest.
    #[must_use]
    pub fn raised_tier(&self) -> bool {
        self.to > self.from
    }
}

/// Accumulates escalations, and nothing else.
///
/// Starts at [`Tier::BOTTOM`] and can only rise. The type deliberately does
/// **not** provide: `set_tier`, a public `tier` field, any `&mut Tier` accessor,
/// `DerefMut`, or a way to build an [`Adjudication`] other than
/// [`finish`](Self::finish).
///
/// "Verdicts only ever move up in scrutiny" is therefore a property of the
/// lattice and this API, not a rule that reviewers have to keep enforcing.
///
/// There is intentionally no `Default`: an adjudicator is always created for a
/// specific evaluation, and `Default::default()` is exactly the kind of call
/// that shows up in the middle of a function that should have threaded the real
/// one through.
#[derive(Debug)]
pub struct Adjudicator {
    /// The accumulated tier. Private, and only `escalate` writes it.
    tier: Tier,
    /// Append-only record of every escalation, in the order they occurred.
    ///
    /// Which is the order the engine accounted resolutions in, and that order
    /// is not the engine's to pick:
    /// [`Resolutions::account_into`](crate::resolution::Resolutions::account_into)
    /// walks its map ascending by
    /// [`RequirementId`](crate::ids::RequirementId). So this ledger is a
    /// function of which requirements resolved and how, not of the order the
    /// tools happened to finish in — which matters because it ends up in a
    /// bundle field.
    ledger: Vec<Escalation>,
}

impl Adjudicator {
    /// A fresh adjudicator at [`Tier::BOTTOM`].
    // `new_without_default` is allowed rather than satisfied, and scoped to this
    // constructor rather than the whole impl so a second one cannot inherit the
    // exemption. The lint's advice is right in general and wrong here: an
    // adjudicator is always created for a specific evaluation, and
    // `Default::default()` is exactly the call that shows up in the middle of a
    // function that should have threaded the real one through. See the note on
    // the type.
    #[allow(clippy::new_without_default)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            tier: Tier::BOTTOM,
            ledger: Vec::new(),
        }
    }

    /// Raise the tier to at least `at_least`, recording why.
    ///
    /// **This is the only method on this type that takes `&mut self`**, and the
    /// only way the tier ever changes. `reason` and `evidence` are required
    /// rather than optional, so an escalation that cannot explain itself is not
    /// expressible.
    ///
    /// Every call is recorded, including calls that do not raise the tier —
    /// see [`Escalation::to`].
    pub fn escalate(
        &mut self,
        at_least: Tier,
        reason: ReasonCode,
        detail: impl Into<String>,
        evidence: EvidenceRef,
    ) {
        let from = self.tier;
        self.tier = self.tier.join(at_least);
        self.ledger.push(Escalation {
            from,
            to: self.tier,
            reason,
            detail: detail.into(),
            evidence,
        });
    }

    /// The tier reached so far.
    #[must_use]
    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// The verdict implied by the tier reached so far.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        self.tier.verdict()
    }

    /// The escalations recorded so far.
    #[must_use]
    pub fn ledger(&self) -> &[Escalation] {
        &self.ledger
    }

    /// Finish, producing an immutable [`Adjudication`].
    ///
    /// Takes `self` by value so that a finished verdict cannot be amended. If
    /// you need to escalate, do it before calling this.
    #[must_use]
    pub fn finish(self) -> Adjudication {
        Adjudication {
            verdict: self.tier.verdict(),
            tier: self.tier,
            escalations: self.ledger,
        }
    }
}

/// A finished verdict.
///
/// Immutable by construction: every field is populated by
/// [`Adjudicator::finish`], and there is no other *producer* — no `new`, no
/// `Default`, and no `From<Tier>`.
///
/// # Reconstruction is checked
///
/// This type is read back out of recorded bundles, which is the path
/// `vibe-check replay` takes, so `Deserialize` can *reconstruct* one. That is
/// not a second producer: it is hand-written rather than derived, and it admits
/// only values [`Adjudicator::finish`] could have written — the verdict agrees
/// with the tier, and the ledger replays to it. A derived `Deserialize` would
/// have been the second producer, and the more dangerous one, because it is the
/// one a replay reaches. See [`ReplayError`].
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[non_exhaustive]
pub struct Adjudication {
    /// The tier reached.
    pub tier: Tier,
    /// The verdict, derived from the tier.
    pub verdict: Verdict,
    /// Every escalation, in accounting order.
    ///
    /// A bundle field, and therefore part of what any digest over the
    /// adjudication covers. See [`Resolutions`](crate::resolution::Resolutions)
    /// for why that order is the ascending
    /// [`RequirementId`](crate::ids::RequirementId) sequence rather than the
    /// order capabilities finished resolving in.
    pub escalations: Vec<Escalation>,
}

impl Adjudication {
    /// The escalations that actually raised the tier.
    pub fn raising(&self) -> impl Iterator<Item = &Escalation> {
        self.escalations.iter().filter(|e| e.raised_tier())
    }

    /// The first escalation that reached the final tier, if any.
    ///
    /// This is the "driven by" line in the pull-request comment.
    ///
    /// "First" means first *in accounting order*, so for a ledger built by
    /// [`Resolutions::account_into`](crate::resolution::Resolutions::account_into)
    /// this is the escalation of the lowest
    /// [`RequirementId`](crate::ids::RequirementId) that reached the final
    /// tier. Alphabetical, not causal — several independent requirements can
    /// each reach the same tier on their own and no ordering among them is
    /// truer than another. What this guarantees is that the same inputs always
    /// name the same one, so the comment does not churn between re-runs.
    #[must_use]
    pub fn primary_cause(&self) -> Option<&Escalation> {
        self.escalations.iter().find(|e| e.to == self.tier)
    }
}

/// Why a recorded adjudication is not one this build can read back.
///
/// # Why a violating document is refused, not repaired
///
/// [`EvidenceRef`]'s wire-format note says a renderer meeting something it does
/// not understand "must degrade to showing the reason code rather than failing
/// to display a verdict", and refusing to parse is the outcome that note exists
/// to avoid. That contract is about *unfamiliar* input — a variant, a key, or a
/// capability written by a build newer than this one — where refusal throws away
/// information the reader could have used. None of the three faults below is
/// unfamiliar input.
///
/// Every value they range over is closed and frozen. [`Tier`] and [`Verdict`]
/// are two of the model's deliberately closed enums, and
/// [`Tier::verdict`](crate::tier::Tier::verdict) is total and is "the only
/// mapping". So there is no build, present or future, that legitimately emits a
/// document where the verdict disagrees with the tier or the ledger does not
/// replay to it: within a schema major, such a document is corrupt or forged,
/// never merely newer. Refusing it discards nothing true.
///
/// The alternative — deserialize and then escalate — is not available at this
/// seam, and that is the decisive argument rather than a matter of taste.
/// Escalating needs an [`Adjudicator`] to escalate *into*, and
/// [`serde::Deserialize`] has no channel to carry one; a `Deserialize` impl can
/// only return a value. The only value it could return is a *repaired* one —
/// overwriting the recorded verdict with `tier.verdict()`, or the recorded tier
/// with the ledger's replay. Repair is worse than refusal in both directions it
/// can go: it destroys the evidence that the record was wrong, and because
/// bundles are read and rewritten by design (see [`crate::schema`]), the next
/// writer emits the repaired document as if it had always been consistent. A
/// bundle that lies about its own verdict would be laundered into one that
/// merely looks right.
///
/// So the choice here is [`crate::ids::LeafId`]'s: hand-write `Deserialize` and
/// fail the parse. What is lost is a reader's ability to display the verdict of
/// a self-contradicting bundle — and such a bundle has no verdict to display,
/// because displaying either of its two disagreeing answers would be a guess
/// presented as a record.
///
/// Tolerating genuinely *unknown* vocabulary is a different problem with a
/// different answer, and it is #29's.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReplayError {
    /// An escalation whose tier fell.
    ///
    /// [`Adjudicator::escalate`] computes `to = from.join(at_least)`, so this is
    /// unreachable from the producer. Admitting it would make
    /// [`Escalation::raised_tier`] — and with it the claim that scrutiny only
    /// rises — false for a replayed ledger.
    #[error("escalation lowers the tier, from {from} to {to}; scrutiny only rises")]
    TierFell {
        /// The tier the escalation claims to start from.
        from: Tier,
        /// The lower tier it claims to reach.
        to: Tier,
    },
    /// The verdict does not agree with the tier beside it.
    ///
    /// [`Adjudicator::finish`] writes `verdict: self.tier.verdict()`, and
    /// [`Tier::verdict`](crate::tier::Tier::verdict) is the only mapping there
    /// is. `tier.rs` refuses to store the two independently precisely because
    /// that "would admit a state where they disagree"; a derived `Deserialize`
    /// admitted it anyway.
    #[error("verdict {verdict:?} disagrees with tier {tier}, which implies {expected:?}")]
    VerdictDisagrees {
        /// The recorded tier.
        tier: Tier,
        /// The recorded verdict.
        verdict: Verdict,
        /// The verdict the recorded tier implies.
        expected: Verdict,
    },
    /// An escalation does not start where the previous one ended.
    ///
    /// The ledger is a sequence, not a set: [`Escalation::from`] is "a statement
    /// about the *sequence*, and the reason a finished ledger is never sorted".
    /// Walking it from [`Tier::BOTTOM`] must visit every entry's `from` in turn,
    /// which is what "the ledger replays" means.
    #[error(
        "escalation {index} starts at {found}, but the ledger had reached {expected}; \
         the ledger does not replay"
    )]
    LedgerDiscontinuous {
        /// Position of the offending escalation.
        index: usize,
        /// The tier the walk had reached.
        expected: Tier,
        /// The tier the escalation claims to start from.
        found: Tier,
    },
    /// The ledger replays to a different tier than the one recorded.
    ///
    /// The ledger is the audit trail for the verdict. If it does not arrive at
    /// the recorded tier, one of the two is fiction and nothing in the document
    /// says which.
    #[error("ledger replays to {replayed}, but the adjudication records {recorded}")]
    LedgerDoesNotReplay {
        /// The tier the adjudication claims.
        recorded: Tier,
        /// The tier its own escalations arrive at.
        replayed: Tier,
    },
}

// Hand-written rather than derived, so the check runs on the wire — the same
// move as `LeafId`, for the reasons set out on `ReplayError`.
//
// The mirror struct is `#[derive]`d and deliberately *not*
// `deny_unknown_fields`: bundles preserve rather than reject what they do not
// understand (see `crate::schema`), and the derived impl this replaces ignored
// unknown keys. Validation is about the fields that are here, not about the
// ones that are not.
impl<'de> Deserialize<'de> for Escalation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            from: Tier,
            to: Tier,
            reason: ReasonCode,
            detail: String,
            evidence: EvidenceRef,
        }

        let wire = Wire::deserialize(deserializer)?;
        if wire.to < wire.from {
            return Err(serde::de::Error::custom(ReplayError::TierFell {
                from: wire.from,
                to: wire.to,
            }));
        }
        Ok(Self {
            from: wire.from,
            to: wire.to,
            reason: wire.reason,
            detail: wire.detail,
            evidence: wire.evidence,
        })
    }
}

// The other half of the same move. Note the `escalations` field deserializes
// through the impl above, so `to >= from` is already established for every entry
// by the time this validates the sequence they form.
impl<'de> Deserialize<'de> for Adjudication {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            tier: Tier,
            verdict: Verdict,
            escalations: Vec<Escalation>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::from_wire(wire.tier, wire.verdict, wire.escalations).map_err(serde::de::Error::custom)
    }
}

impl Adjudication {
    /// Rebuild a recorded adjudication, or say why it is not one.
    ///
    /// Private, and reachable only from [`Deserialize`]: this is a *checker*
    /// standing in front of the wire, not a second producer. Making it public
    /// would hand out the `From<Tier>` the type documents itself as not having.
    ///
    /// The walk is exactly what `Adjudicator` does forwards. It starts at
    /// [`Tier::BOTTOM`], because that is where `Adjudicator::new` starts, and
    /// every entry must pick up where the last one left off — which is what
    /// makes this the parse-time form of `ledger_replays_to_the_same_tier`.
    fn from_wire(
        tier: Tier,
        verdict: Verdict,
        escalations: Vec<Escalation>,
    ) -> Result<Self, ReplayError> {
        let expected = tier.verdict();
        if verdict != expected {
            return Err(ReplayError::VerdictDisagrees {
                tier,
                verdict,
                expected,
            });
        }

        let mut replayed = Tier::BOTTOM;
        for (index, escalation) in escalations.iter().enumerate() {
            if escalation.from != replayed {
                return Err(ReplayError::LedgerDiscontinuous {
                    index,
                    expected: replayed,
                    found: escalation.from,
                });
            }
            replayed = escalation.to;
        }
        if replayed != tier {
            return Err(ReplayError::LedgerDoesNotReplay {
                recorded: tier,
                replayed,
            });
        }

        Ok(Self {
            tier,
            verdict,
            escalations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::CapabilityId;
    use proptest::prelude::*;

    fn any_tier() -> impl Strategy<Value = Tier> {
        prop_oneof![Just(Tier::T0), Just(Tier::T1), Just(Tier::T2)]
    }

    fn escalate_to(adj: &mut Adjudicator, tier: Tier) {
        adj.escalate(
            tier,
            ReasonCode::RuleTierAtLeast,
            "test",
            EvidenceRef::Unattributed,
        );
    }

    #[test]
    fn starts_at_the_bottom() {
        let adj = Adjudicator::new();
        assert_eq!(adj.tier(), Tier::T0);
        assert_eq!(adj.verdict(), Verdict::Auto);
        assert!(adj.ledger().is_empty());
    }

    #[test]
    fn records_non_raising_escalations() {
        // Three independent reasons for human review. The tier stops rising
        // after the first, but all three are why the verdict is `human`, and a
        // comment that mentions only `unsafe` is a comment that misleads.
        let mut adj = Adjudicator::new();
        adj.escalate(
            Tier::T2,
            ReasonCode::RuleTierAtLeast,
            "rule `core-unsafe` requires T2",
            EvidenceRef::Unattributed,
        );
        adj.escalate(
            Tier::T2,
            ReasonCode::CapabilityUnverified,
            "loom-clean unverified",
            EvidenceRef::Capability(CapabilityId::new("loom-clean")),
        );
        adj.escalate(
            Tier::T2,
            ReasonCode::GateIntegrity,
            "modifies .github/workflows",
            EvidenceRef::Unattributed,
        );

        let done = adj.finish();
        assert_eq!(done.tier, Tier::T2);
        assert_eq!(done.escalations.len(), 3);
        // Only the first actually moved the needle.
        assert_eq!(done.raising().count(), 1);
        assert_eq!(
            done.primary_cause().map(|e| e.reason),
            Some(ReasonCode::RuleTierAtLeast)
        );
    }

    #[test]
    fn gate_integrity_forces_human_from_any_starting_point() {
        // The specification says gate-integrity "caps the verdict at human
        // tier". Read as a ceiling it would be the only downward operation in
        // the system. Because T2 is the top of the lattice, a floor of T2 and a
        // ceiling of T2 are the same thing, so it is an ordinary escalation and
        // `escalate` remains the sole mutator.
        for start in [Tier::T0, Tier::T1, Tier::T2] {
            let mut adj = Adjudicator::new();
            escalate_to(&mut adj, start);
            adj.escalate(
                Tier::TOP,
                ReasonCode::GateIntegrity,
                "pull request modifies its own gates",
                EvidenceRef::Unattributed,
            );
            assert_eq!(adj.finish().verdict, Verdict::Human);
        }
    }

    proptest! {
        /// No sequence of escalations can lower the tier. This is the whole
        /// safety property, stated as a test.
        #[test]
        fn tier_never_decreases(steps in prop::collection::vec(any_tier(), 0..24)) {
            let mut adj = Adjudicator::new();
            let mut previous = adj.tier();
            for step in steps {
                escalate_to(&mut adj, step);
                prop_assert!(adj.tier() >= previous);
                previous = adj.tier();
            }
        }

        /// The final tier is exactly the maximum demanded, regardless of the
        /// order the demands arrived in. Capabilities resolve concurrently, so
        /// order is not something we control.
        #[test]
        fn final_tier_is_the_maximum_demanded(steps in prop::collection::vec(any_tier(), 0..24)) {
            let mut adj = Adjudicator::new();
            for step in steps.iter().copied() {
                escalate_to(&mut adj, step);
            }
            let want = steps.iter().copied().max().unwrap_or(Tier::BOTTOM);
            prop_assert_eq!(adj.tier(), want);
        }

        /// Shuffling the escalations changes the ledger order but never the
        /// verdict.
        #[test]
        fn verdict_is_order_independent(mut steps in prop::collection::vec(any_tier(), 0..16)) {
            let mut forward = Adjudicator::new();
            for step in steps.iter().copied() {
                escalate_to(&mut forward, step);
            }
            steps.reverse();
            let mut backward = Adjudicator::new();
            for step in steps.iter().copied() {
                escalate_to(&mut backward, step);
            }
            prop_assert_eq!(forward.finish().verdict, backward.finish().verdict);
        }

        /// Every ledger entry is consistent: `to` is the join of `from` and
        /// whatever was demanded, so the ledger can be replayed to recompute the
        /// tier.
        #[test]
        fn ledger_replays_to_the_same_tier(steps in prop::collection::vec(any_tier(), 0..16)) {
            let mut adj = Adjudicator::new();
            for step in steps.iter().copied() {
                escalate_to(&mut adj, step);
            }
            let done = adj.finish();
            let replayed = done
                .escalations
                .iter()
                .fold(Tier::BOTTOM, |acc, e| acc.join(e.to));
            prop_assert_eq!(replayed, done.tier);
        }
    }
}
