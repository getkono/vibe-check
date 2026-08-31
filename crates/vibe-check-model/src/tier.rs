//! The scrutiny lattice and the verdict derived from it.
//!
//! Adjudication is a join-semilattice: tiers combine with `max`, never with
//! assignment. Everything a pull request can do — introduce `unsafe`, fail to
//! produce a parseable artifact, edit its own policy — can only push scrutiny
//! *up*. Nothing in a pull request can lower its own tier, because no operation
//! exists that lowers a tier.
//!
//! [`Verdict`] is a total function of [`Tier`] rather than an independently
//! stored field. Storing both would admit a state where they disagree, and the
//! first thing anyone would write is a helper that sets one without the other.

use serde::{Deserialize, Serialize};
use std::fmt;

/// How much scrutiny a change requires.
///
/// Ordered `T0 < T1 < T2`. The derived [`Ord`] *is* the lattice order, and this
/// is the only place that order is defined — nothing else should compare tiers
/// by hand.
///
/// This is one of the two deliberately closed enums in the model (see
/// [`crate::ids`] for why identifiers are open strings instead). The set is
/// small, it is the core of the safety argument, and adding a variant *should*
/// break every match arm until each has been reconsidered.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    /// No human attention required; the evidence is sufficient on its own.
    T0,
    /// The change alters an interface; a reviewer should look at the shape of it.
    T1,
    /// A human must review this change.
    T2,
}

impl Tier {
    /// The identity of [`join`](Self::join): the least scrutiny.
    ///
    /// Every adjudication starts here and rises.
    pub const BOTTOM: Self = Self::T0;

    /// The absorbing element of [`join`](Self::join): the most scrutiny.
    ///
    /// Because `T2` is the top, a "ceiling of human review" and a "floor of
    /// human review" are the same statement. That is why `gate-integrity`
    /// capping a verdict needs no ceiling machinery — it is an ordinary
    /// escalation to `TOP`, and escalation stays the only mutator.
    pub const TOP: Self = Self::T2;

    /// Combine two tiers, taking the greater. The lattice join.
    #[must_use]
    pub fn join(self, other: Self) -> Self {
        if self >= other { self } else { other }
    }

    /// The most scrutiny demanded by any tier in the iterator.
    ///
    /// Returns [`Tier::BOTTOM`] for an empty iterator, which is correct: no
    /// demands means nothing has asked for scrutiny yet. It is *not* a claim
    /// that the change is safe — that only follows once every required
    /// capability has been accounted for.
    #[must_use]
    pub fn most_scrutiny(tiers: impl IntoIterator<Item = Self>) -> Self {
        tiers.into_iter().fold(Self::BOTTOM, Self::join)
    }

    /// The verdict this tier implies. Total, and the only mapping.
    #[must_use]
    pub fn verdict(self) -> Verdict {
        match self {
            Self::T0 => Verdict::Auto,
            Self::T1 => Verdict::InterfaceReview,
            Self::T2 => Verdict::Human,
        }
    }

    /// The process exit code this tier implies.
    ///
    /// These are a stable public interface — scripts and CI branch on them — so
    /// they are defined here next to the tier rather than in the CLI, where they
    /// would be easy to change by accident.
    ///
    /// Note that `1` is deliberately *not* in this range: it means vibe-check
    /// itself failed. A tool failure must never be reported as `auto`.
    #[must_use]
    pub fn exit_code(self) -> u8 {
        match self {
            Self::T0 => 0,
            Self::T1 => 10,
            Self::T2 => 20,
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::T0 => "T0",
            Self::T1 => "T1",
            Self::T2 => "T2",
        })
    }
}

/// What should happen to the pull request.
///
/// Derived from [`Tier`]; never stored independently and never constructed from
/// anything else.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Merge without human review.
    Auto,
    /// A reviewer should check the interface change.
    InterfaceReview,
    /// A human must review.
    Human,
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::InterfaceReview => "interface-review",
            Self::Human => "human",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn any_tier() -> impl Strategy<Value = Tier> {
        prop_oneof![Just(Tier::T0), Just(Tier::T1), Just(Tier::T2)]
    }

    proptest! {
        /// A join never returns less than either input. This is the property the
        /// entire "verdicts only move up" guarantee rests on.
        #[test]
        fn join_is_an_upper_bound(a in any_tier(), b in any_tier()) {
            let j = a.join(b);
            prop_assert!(j >= a);
            prop_assert!(j >= b);
        }

        /// Order of evidence must not change the verdict. Capabilities resolve
        /// concurrently, so a non-commutative join would make verdicts depend on
        /// which job finished first.
        #[test]
        fn join_is_commutative(a in any_tier(), b in any_tier()) {
            prop_assert_eq!(a.join(b), b.join(a));
        }

        #[test]
        fn join_is_associative(a in any_tier(), b in any_tier(), c in any_tier()) {
            prop_assert_eq!(a.join(b).join(c), a.join(b.join(c)));
        }

        /// Seeing the same evidence twice must not change anything — retried
        /// jobs and re-delivered artifacts are normal.
        #[test]
        fn join_is_idempotent(a in any_tier()) {
            prop_assert_eq!(a.join(a), a);
        }

        #[test]
        fn bottom_is_the_identity(a in any_tier()) {
            prop_assert_eq!(a.join(Tier::BOTTOM), a);
        }

        /// Once human review is demanded, nothing can talk it back down.
        #[test]
        fn top_absorbs(a in any_tier()) {
            prop_assert_eq!(a.join(Tier::TOP), Tier::TOP);
        }

        /// The verdict mapping preserves order: more scrutiny never yields a
        /// more permissive verdict.
        #[test]
        fn verdict_is_monotone(a in any_tier(), b in any_tier()) {
            prop_assume!(a <= b);
            prop_assert!(a.verdict() <= b.verdict());
        }

        /// Same, for the exit code, which is what CI actually branches on.
        #[test]
        fn exit_code_is_monotone(a in any_tier(), b in any_tier()) {
            prop_assume!(a <= b);
            prop_assert!(a.exit_code() <= b.exit_code());
        }

        #[test]
        fn most_scrutiny_is_the_maximum(tiers in prop::collection::vec(any_tier(), 0..12)) {
            let got = Tier::most_scrutiny(tiers.iter().copied());
            let want = tiers.iter().copied().max().unwrap_or(Tier::BOTTOM);
            prop_assert_eq!(got, want);
        }
    }

    #[test]
    fn empty_evidence_is_bottom_not_a_safety_claim() {
        assert_eq!(Tier::most_scrutiny([]), Tier::T0);
    }

    #[test]
    fn exit_code_one_is_reserved_for_tool_failure() {
        // Nothing in the lattice may produce `1`; that code means vibe-check
        // itself failed, and must be distinguishable from `auto`.
        for tier in [Tier::T0, Tier::T1, Tier::T2] {
            assert_ne!(tier.exit_code(), 1);
        }
    }

    #[test]
    fn serializes_to_the_documented_wire_form() {
        assert_eq!(
            serde_json::to_string(&Tier::T2).expect("serialize"),
            r#""t2""#
        );
        assert_eq!(
            serde_json::to_string(&Verdict::InterfaceReview).expect("serialize"),
            r#""interface-review""#
        );
    }
}
