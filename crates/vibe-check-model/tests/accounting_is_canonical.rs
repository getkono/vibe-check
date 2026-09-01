//! The escalation ledgers are a function of the resolutions, not of their order.
//!
//! Issue #26 asked for a property the existing tests could not state.
//! `verdict_is_order_independent` in `accumulator.rs` proves the *verdict*
//! survives reordering, and it would keep passing if the ledger were rebuilt in
//! insertion order tomorrow — a verdict is a join, and a join does not care. The
//! ledger is a `Vec` that ends up in a bundle field, so for it the property has
//! to be byte-identity, and byte-identity is what these tests assert.
//!
//! Everything here goes through the crate's public API, which is the second
//! thing being proved: demoting `CapabilityResolution::account` to `pub(crate)`
//! removed a way to account resolutions in an arbitrary order without removing
//! the ability to account them at all.
//!
//! # Why the bytes are JSON
//!
//! Because they can be. They were `Debug` output when these tests were written:
//! `EvidenceRef` was internally tagged and `Requirement(RequirementId)` is a
//! newtype variant over a string, which serde cannot internally tag, so every
//! escalation `account` produces failed to serialize. `reason.rs` now tags it
//! adjacently and the ledger reaches the wire, so the byte-identity property is
//! asserted over the bytes that actually go into a bundle rather than over a
//! stand-in. `the_ledger_serializes` below is the assertion that keeps it
//! honest — if serializing regresses, that test says so directly instead of
//! every property here failing with a `Result` nobody reads.
//!
//! # What this does not cover
//!
//! `Known::get` escalates through `Adjudicators::integrity` directly, outside
//! any `Resolutions` map, and nothing constrains the order in which policy
//! identifiers are resolved. Those escalations are still unordered. That is
//! recorded in the pull request and belongs to the issue that owns policy
//! resolution; it is not reachable from this crate's public API today.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use proptest::prelude::*;
use vibe_check_model::{
    Adjudicators, CapabilityResolution, Confidence, Enforcement, Escalation, EvidenceRef,
    RequirementId, ResolutionState, Resolutions, Tier, UnverifiedReason,
};

/// A shape-valid requirement identifier from a readable name.
///
/// The digest half is fixed filler. These tests are about the *order*
/// resolutions are accounted in, not about how an identifier is derived, and
/// real `RequirementId::derive` output would tie every ordering assertion below
/// to sixteen hex characters no reader can predict. `from_wire` is still the
/// only way in, so a fixture cannot drift away from the shape the wire accepts.
fn requirement(name: &str) -> RequirementId {
    RequirementId::from_wire(format!("req_{name}_0000000000000000"))
        .expect("a well-formed fixture identifier")
}

/// The lane a requirement carries, derived from its identifier.
///
/// Derived rather than positional so that the same identifier gets the same
/// lane in every ordering of the same set. A positional rule would make the
/// shuffled build a *different* input, and the test would prove nothing.
fn lane_for(requirement: &RequirementId) -> Enforcement {
    if requirement.as_str().bytes().map(u32::from).sum::<u32>() % 2 == 0 {
        Enforcement::Enforcing
    } else {
        Enforcement::Advisory
    }
}

/// A resolution that always escalates, so each requirement leaves exactly one
/// ledger entry and the ledger's order is directly readable.
///
/// `MissingEvidence` is deliberately not a policy-integrity reason, so an
/// advisory requirement really does land in the advisory ledger rather than
/// being overridden into the enforcing one.
fn always_escalates() -> CapabilityResolution {
    CapabilityResolution::Unverified {
        reason: UnverifiedReason::MissingEvidence,
    }
}

/// Distinct requirement identifiers, in ascending order.
fn distinct_ids() -> impl Strategy<Value = Vec<RequirementId>> {
    prop::collection::btree_set("[a-z]{1,6}", 2..12)
        .prop_map(|names| names.iter().map(|name| requirement(name)).collect())
}

/// A set of identifiers and the same set in a shuffled order.
fn a_set_and_a_shuffle() -> impl Strategy<Value = (Vec<RequirementId>, Vec<RequirementId>)> {
    distinct_ids().prop_flat_map(|ids| (Just(ids.clone()), Just(ids).prop_shuffle()))
}

/// Insert every identifier in the given order and account the result.
///
/// Returns the enforced and advisory ledgers.
fn account(order: &[RequirementId]) -> (Vec<Escalation>, Vec<Escalation>) {
    let mut resolutions = Resolutions::new();
    for requirement in order {
        let displaced = resolutions.insert(
            requirement.clone(),
            lane_for(requirement),
            always_escalates(),
        );
        assert!(displaced.is_none(), "the fixture uses distinct identifiers");
    }

    let mut adjudicators = Adjudicators::new();
    resolutions.account_into(&mut adjudicators);
    let (enforced, advisory) = adjudicators.finish();
    (
        enforced.into_adjudication().escalations,
        advisory.into_escalations(),
    )
}

/// The requirement each escalation points at.
fn requirements(ledger: &[Escalation]) -> Vec<&RequirementId> {
    ledger
        .iter()
        .map(|escalation| match &escalation.evidence {
            EvidenceRef::Requirement(requirement) => requirement,
            other => panic!("every escalation here points at a requirement, found {other:?}"),
        })
        .collect()
}

/// Whether a ledger's requirements ascend with no repeats.
fn strictly_increasing(ledger: &[Escalation]) -> bool {
    requirements(ledger)
        .windows(2)
        .all(|pair| pair[0] < pair[1])
}

/// Both ledgers rendered to the bytes a bundle would carry.
///
/// `serde_json` rather than `Debug`: a ledger ends up in a bundle field, and
/// the property #26 asks for is about those bytes, not about a rendering that
/// merely correlates with them. `to_string` is deterministic over a value here
/// because nothing in an `Escalation` is unordered — every field of it, and of
/// everything it transitively holds, is a scalar, a `String`, a `Vec` or an
/// `Option`, with struct fields emitted in declaration order. There is no map
/// anywhere in that closure, so no iteration order is being relied on. Any
/// difference between two runs is therefore a difference in the ledgers.
fn as_bytes(ledgers: &(Vec<Escalation>, Vec<Escalation>)) -> (String, String) {
    (
        serde_json::to_string(&ledgers.0).expect("the enforced ledger serializes"),
        serde_json::to_string(&ledgers.1).expect("the advisory ledger serializes"),
    )
}

proptest! {
    /// The ledgers come out in ascending `RequirementId` order whatever order
    /// the resolutions were inserted in.
    ///
    /// Stronger than asserting two runs agree: this pins the *direction*, so
    /// reversing the walk inside `account_into` reddens it. Two runs that both
    /// descend would agree with each other perfectly.
    #[test]
    fn accounting_order_is_requirement_order(ids in distinct_ids().prop_shuffle()) {
        let (enforced, advisory) = account(&ids);

        prop_assert!(
            strictly_increasing(&enforced),
            "enforced ledger is not ascending: {:?}",
            requirements(&enforced)
        );
        prop_assert!(
            strictly_increasing(&advisory),
            "advisory ledger is not ascending: {:?}",
            requirements(&advisory)
        );
        // Every requirement escalated exactly once, into exactly one ledger, so
        // each ledger is a subsequence of one ascending sequence rather than two
        // independently ordered lists that happen to agree.
        prop_assert_eq!(enforced.len() + advisory.len(), ids.len());
    }

    /// The same resolutions serialize identically however they were inserted.
    ///
    /// This is issue #26's "done when", stated as byte-identity rather than as
    /// equality of verdicts. Swapping the `BTreeMap` inside `Resolutions` for an
    /// insertion-ordered `Vec` reddens this; it leaves
    /// `verdict_is_order_independent` green, which is exactly the gap between
    /// the property that existed and the one that was missing.
    #[test]
    fn the_same_resolutions_serialize_identically_in_any_order(
        (forward, shuffled) in a_set_and_a_shuffle()
    ) {
        prop_assert_eq!(as_bytes(&account(&forward)), as_bytes(&account(&shuffled)));
    }

    /// Two independent runs over the same input produce identical bytes.
    ///
    /// The replay-corpus property: nothing in an adjudication may depend on
    /// anything that is not the input.
    #[test]
    fn two_independent_runs_produce_identical_bytes(ids in distinct_ids()) {
        prop_assert_eq!(as_bytes(&account(&ids)), as_bytes(&account(&ids)));
    }
}

#[test]
fn the_ledger_serializes() {
    // This test used to assert the opposite, as a tripwire: `EvidenceRef` was
    // `#[serde(tag = "kind")]`, `Requirement`, `Capability`, `Crate`, `Rule`
    // and `Path` are newtype variants over strings, and serde cannot tag those
    // internally — so serializing failed at run time. Every escalation
    // `account` produces points at a requirement, which put every ledger, every
    // `Adjudication` and every bundle carrying one out of reach of JSON. The
    // tripwire was written to fail the day someone fixed that. Someone did:
    // `reason.rs` is adjacently tagged now, `as_bytes` above compares real
    // bytes, and #21's byte-identical-bundle claim has one fewer blocker.
    //
    // It stays as a positive assertion because `as_bytes` depends on it. If
    // serializing regresses, the properties above fail on an `expect` in a
    // helper, which says nothing about why; this fails on the actual claim.
    let mut resolutions = Resolutions::new();
    let displaced = resolutions.insert(
        requirement("apple"),
        Enforcement::Enforcing,
        always_escalates(),
    );
    assert!(displaced.is_none(), "the fixture uses distinct identifiers");
    let mut adjudicators = Adjudicators::new();
    resolutions.account_into(&mut adjudicators);
    let ledger = adjudicators.finish().0.into_adjudication().escalations;
    assert_eq!(ledger.len(), 1);

    let json = serde_json::to_string(&ledger).expect("the ledger serializes");
    let back: Vec<Escalation> = serde_json::from_str(&json).expect("and deserializes");
    assert_eq!(
        back, ledger,
        "this ledger survives the encoding it is compared through"
    );
    // Deliberately not claiming injectivity. One ledger holding one escalation
    // cannot show that two *different* ledgers get different bytes, which is
    // what `as_bytes` leans on when it reports two runs as equal. That property
    // does hold — every field of `Escalation` is serialized and none is
    // collapsed — but it holds by inspection of the type, not by anything this
    // test executes, and a round trip of a single value is not evidence for it.
}

#[test]
fn both_ledgers_are_ordered() {
    // A mixed set, inserted descending so insertion order and accounting order
    // disagree about every entry, and arranged so that *both* ledgers get more
    // than one escalation. A fix that ordered the enforced ledger and forgot the
    // advisory one would pass a single-ledger test.
    let mut resolutions = Resolutions::new();
    let displaced = resolutions.insert(
        requirement("zebra"),
        Enforcement::Advisory,
        CapabilityResolution::Unverified {
            reason: UnverifiedReason::Inconclusive {
                reason: "no data".into(),
            },
        },
    );
    assert!(displaced.is_none(), "the fixture uses distinct identifiers");
    // Advisory as declared, but an unknown capability is a fact about the
    // policy, so `account` overrides the lane and this lands in the enforced
    // ledger. Its position there is what proves the override is fed by the same
    // ascending pass rather than by a separate one.
    let displaced = resolutions.insert(
        requirement("mango"),
        Enforcement::Advisory,
        CapabilityResolution::Unverified {
            reason: UnverifiedReason::UnknownCapability {
                id: "loom-clean".into(),
            },
        },
    );
    assert!(displaced.is_none(), "the fixture uses distinct identifiers");
    let displaced = resolutions.insert(
        requirement("berry"),
        Enforcement::Advisory,
        always_escalates(),
    );
    assert!(displaced.is_none(), "the fixture uses distinct identifiers");
    let displaced = resolutions.insert(
        requirement("apple"),
        Enforcement::Enforcing,
        always_escalates(),
    );
    assert!(displaced.is_none(), "the fixture uses distinct identifiers");

    let mut adjudicators = Adjudicators::new();
    resolutions.account_into(&mut adjudicators);
    let (enforced, advisory) = adjudicators.finish();
    let enforced = enforced.into_adjudication();

    assert_eq!(
        requirements(&enforced.escalations),
        [&requirement("apple"), &requirement("mango"),],
        "the enforced ledger, including the policy-integrity override, ascends"
    );
    assert_eq!(
        requirements(advisory.escalations()),
        [&requirement("berry"), &requirement("zebra"),],
        "and so does the advisory one"
    );
    assert_eq!(enforced.tier, Tier::TOP);
    assert_eq!(advisory.tier(), Tier::TOP);

    // The documented `primary_cause` behaviour: the lowest `RequirementId` that
    // reached the final tier, not whichever resolution finished first.
    assert_eq!(
        enforced.primary_cause().map(|e| &e.evidence),
        Some(&EvidenceRef::Requirement(requirement("apple"))),
    );
}

#[test]
fn a_duplicate_requirement_is_visible() {
    // Under a scope collision two resolutions can arrive under one identifier.
    // A map that silently kept the last would drop the first, and if the first
    // was the failing one that is a fail-open. `insert` hands the displaced
    // value back so the caller has to decide.
    let mut resolutions = Resolutions::new();
    assert!(
        resolutions
            .insert(
                requirement("tests-pass"),
                Enforcement::Enforcing,
                always_escalates(),
            )
            .is_none()
    );
    let displaced = resolutions.insert(
        requirement("tests-pass"),
        Enforcement::Advisory,
        CapabilityResolution::Unverified {
            reason: UnverifiedReason::NoForge,
        },
    );
    assert_eq!(
        displaced,
        Some((Enforcement::Enforcing, always_escalates())),
        "the displaced resolution is returned, not dropped"
    );
    assert_eq!(resolutions.len(), 1);
}

#[test]
fn the_tally_and_the_ledger_read_the_same_map() {
    // `states` exists so the confidence sentence and the escalations are built
    // from one collection. Assembled separately they can disagree, and a comment
    // that says "4 requirements" above five escalations is a comment nobody can
    // reconcile.
    let ids = ["apple", "berry", "mango"].map(requirement);
    let mut resolutions = Resolutions::new();
    for requirement in &ids {
        let displaced = resolutions.insert(
            requirement.clone(),
            lane_for(requirement),
            always_escalates(),
        );
        assert!(displaced.is_none(), "the fixture uses distinct identifiers");
    }
    assert!(!resolutions.is_empty());

    let tally = Confidence::tally(resolutions.states());
    assert_eq!(tally.requirements, resolutions.len());
    assert_eq!(tally.unverified, resolutions.len());
    assert_eq!(
        tally.advisory,
        resolutions
            .iter()
            .filter(|(_, enforcement, _)| *enforcement == Enforcement::Advisory)
            .count()
    );

    let mut adjudicators = Adjudicators::new();
    resolutions.account_into(&mut adjudicators);
    let (enforced, advisory) = adjudicators.finish();
    assert_eq!(
        enforced.adjudication().escalations.len() + advisory.count(),
        tally.requirements
    );
    assert!(
        resolutions
            .states()
            .all(|(state, _)| state == ResolutionState::Unverified)
    );
}
