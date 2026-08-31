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
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Escalation {
    /// The tier before this escalation.
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
/// [`Adjudicator::finish`], and there is no other constructor, no `Default`, and
/// no `From<Tier>`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Adjudication {
    /// The tier reached.
    pub tier: Tier,
    /// The verdict, derived from the tier.
    pub verdict: Verdict,
    /// Every escalation, in order.
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
    #[must_use]
    pub fn primary_cause(&self) -> Option<&Escalation> {
        self.escalations.iter().find(|e| e.to == self.tier)
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
