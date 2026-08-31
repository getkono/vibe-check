//! `ResolutionState::collapse` is a meet, proved the way `Tier::join` is proved.
//!
//! `core.capability_states` is keyed by a bare capability while resolution
//! happens per *(capability × scope)* requirement, so the map's entries are an
//! aggregate: several states become one. `collapse` is that aggregation, and an
//! aggregation over a set is only well defined if the binary operation
//! underneath it is a semilattice. Otherwise "the state of `tests-pass`" is a
//! function of the order the scopes finished in — and scopes finish in whatever
//! order the runners return, which is not a property of the pull request.
//!
//! That is the same claim `Tier::join` already carries in `tier.rs`, for the
//! same reason, so it gets the same treatment. The laws:
//!
//! - **commutative** — two scopes, no first one;
//! - **associative** — three or more scopes group however the caller folds;
//! - **idempotent** — a re-delivered artifact or a retried job cannot move it.
//!
//! Plus the two that say which way the operation leans: `Unverified` absorbs,
//! so nothing can talk an unanswered scope back up, and `Run` is the identity,
//! so the most confident scope never lowers another.
//!
//! # Why binary, when the field needs a fold
//!
//! Every law here is a statement about a binary operation. Stated over an
//! iterator-shaped `collapse(states) -> Option<Self>` they would each need a
//! concatenation of vectors and an `Option` unwrap in the middle, and what the
//! assertion actually pinned would be hard to see — which is how a property
//! test ends up passing for a reason unrelated to its name. So `collapse` is
//! binary, exactly like `Tier::join`, and `collapse_all` is the fold over it.
//! `collapse_all_is_the_least_confident_state` is what ties the fold back to
//! the operation, by checking it against an independently computed minimum
//! rather than against another fold.
//!
//! Out of this crate's `src/` because it is about the contract of a frozen
//! bundle field, not about the internals of an enum, and it uses nothing that
//! is not public.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use vibe_check_model::ResolutionState;

fn any_state() -> impl Strategy<Value = ResolutionState> {
    prop_oneof![
        Just(ResolutionState::Adopt),
        Just(ResolutionState::Run),
        Just(ResolutionState::Skip),
        Just(ResolutionState::Unverified),
    ]
}

proptest! {
    /// Order of evidence must not change the entry. Scopes resolve
    /// concurrently, so a non-commutative collapse would make a frozen bundle
    /// field depend on which runner finished first.
    #[test]
    fn collapse_is_commutative(a in any_state(), b in any_state()) {
        prop_assert_eq!(a.collapse(b), b.collapse(a));
    }

    /// A capability required for three crates must not depend on how the
    /// caller nests the fold.
    #[test]
    fn collapse_is_associative(a in any_state(), b in any_state(), c in any_state()) {
        prop_assert_eq!(a.collapse(b).collapse(c), a.collapse(b.collapse(c)));
    }

    /// The same scope reported twice — a retried job, a re-delivered artifact —
    /// is the same information, not more of it.
    #[test]
    fn collapse_is_idempotent(a in any_state()) {
        prop_assert_eq!(a.collapse(a), a);
    }

    /// A collapse never returns more confidence than either input had, and
    /// never returns a state neither input was in.
    #[test]
    fn collapse_is_a_lower_bound(a in any_state(), b in any_state()) {
        let got = a.collapse(b);
        prop_assert!(got.confidence_rank() <= a.confidence_rank());
        prop_assert!(got.confidence_rank() <= b.confidence_rank());
        prop_assert!(got == a || got == b);
    }

    /// Once a scope goes unanswered, no other scope can talk the entry back up.
    /// This is the fail-closed direction, and the reason the rank order is
    /// minimised rather than maximised.
    #[test]
    fn unverified_absorbs(a in any_state()) {
        prop_assert_eq!(a.collapse(ResolutionState::Unverified), ResolutionState::Unverified);
    }

    /// The most confident state is the identity, so a scope that ran cleanly
    /// never drags another scope's entry down.
    #[test]
    fn run_is_the_identity(a in any_state()) {
        prop_assert_eq!(a.collapse(ResolutionState::Run), a);
    }

    /// The fold agrees with an independently computed minimum, and an empty
    /// fold is `None` rather than an invented state.
    #[test]
    fn collapse_all_is_the_least_confident_state(
        states in prop::collection::vec(any_state(), 0..12),
    ) {
        let got = ResolutionState::collapse_all(states.iter().copied());
        let want = states
            .iter()
            .copied()
            .min_by_key(|state| state.confidence_rank());
        prop_assert_eq!(got, want);
        prop_assert_eq!(got.is_none(), states.is_empty());
    }

    /// Shuffling the requirements a capability resolved from cannot change its
    /// entry. Commutativity and associativity imply this; it is asserted anyway
    /// because it is the property the bundle actually depends on, and stating
    /// it directly means a future rewrite of `collapse_all` — a `sort` and a
    /// `first`, say — is checked against the thing that matters rather than
    /// against the laws it was derived from.
    #[test]
    fn collapse_all_ignores_order(
        states in prop::collection::vec(any_state(), 1..12),
        rotation in 0usize..12,
    ) {
        let mut rotated = states.clone();
        rotated.rotate_left(rotation % states.len());
        prop_assert_eq!(
            ResolutionState::collapse_all(states.iter().copied()),
            ResolutionState::collapse_all(rotated),
        );
    }
}
