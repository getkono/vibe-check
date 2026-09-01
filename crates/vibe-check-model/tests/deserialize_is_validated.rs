//! `Deserialize` must not be a second constructor.
//!
//! Three types in this crate establish an invariant in their constructor and,
//! until this test existed, dropped it on the wire. `Adjudicator::finish`
//! guarantees a verdict that agrees with its tier and a ledger that replays to
//! it; `Adjudicator::escalate` guarantees `to >= from`; `Confidence::tally`
//! guarantees the state counts partition the requirements. A derived
//! `Deserialize` guaranteed none of them, and it is the impl `vibe-check
//! replay` reaches.
//!
//! # Every test here plants the violation
//!
//! A test that asserts a well-formed document parses cannot fail when the
//! validation is deleted. So each law below is stated twice: once as a document
//! that *does* violate it, asserted to be refused, and once as the nearest
//! well-formed document, asserted to be accepted. Delete any check in
//! `accumulator.rs` or `bundle.rs` and the first half of its pair goes green
//! against a value the producer cannot make.
//!
//! Every violating document here was verified to deserialize successfully
//! against the derived impls this replaces.

// A test asserting a document is refused has to be able to fail loudly when it
// is not, which is what these three lints forbid in library code and every
// sibling guard in this directory lifts here for the same reason.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use vibe_check_model::adjudicate::{Adjudication, Adjudicator, Escalation};
use vibe_check_model::bundle::Confidence;
use vibe_check_model::reason::{EvidenceRef, ReasonCode};
use vibe_check_model::tier::{Tier, Verdict};

/// Deserialize, expecting refusal, and return the message so the test can say
/// *which* law fired rather than merely that something did.
fn refused<T: serde::de::DeserializeOwned>(json: &str) -> String {
    match serde_json::from_str::<T>(json) {
        Ok(_) => panic!("expected this document to be refused, but it parsed:\n{json}"),
        Err(error) => error.to_string(),
    }
}

fn accepted<T: serde::de::DeserializeOwned>(json: &str) -> T {
    match serde_json::from_str::<T>(json) {
        Ok(value) => value,
        Err(error) => panic!("expected this document to parse, but: {error}\n{json}"),
    }
}

/// An escalation as JSON, with `from`/`to` chosen by the caller.
fn escalation_json(from: &str, to: &str) -> String {
    format!(
        r#"{{"from":"{from}","to":"{to}","reason":"capability-violated",
            "detail":"2 failures","evidence":{{"kind":"unattributed"}}}}"#
    )
}

// ---------------------------------------------------------------- Escalation

#[test]
fn an_escalation_may_not_lower_the_tier() {
    let message = refused::<Escalation>(&escalation_json("t2", "t0"));
    assert!(
        message.contains("scrutiny only rises"),
        "wrong law fired: {message}"
    );
}

#[test]
fn an_escalation_that_does_not_raise_the_tier_is_still_recorded() {
    // The paired half. `to == from` is not a violation — a second independent
    // reason for review is recorded even though it raises nothing, and refusing
    // it here would throw away most of a real ledger.
    let escalation: Escalation = accepted(&escalation_json("t2", "t2"));
    assert!(!escalation.raised_tier());

    let raising: Escalation = accepted(&escalation_json("t0", "t2"));
    assert!(raising.raised_tier());
}

// -------------------------------------------------------------- Adjudication

#[test]
fn a_verdict_may_not_disagree_with_its_tier() {
    // `t0` implies `auto`. This document claims a human must review a change
    // the tier says needs no attention, and nothing in it says which is true.
    let message = refused::<Adjudication>(r#"{"tier":"t0","verdict":"human","escalations":[]}"#);
    assert!(
        message.contains("disagrees with tier"),
        "wrong law fired: {message}"
    );

    // ...and the reverse direction, which is the one that fails open: a `t2`
    // adjudication wearing an `auto` verdict.
    let message = refused::<Adjudication>(
        r#"{"tier":"t2","verdict":"auto","escalations":[
        {"from":"t0","to":"t2","reason":"capability-violated","detail":"x",
         "evidence":{"kind":"unattributed"}}]}"#,
    );
    assert!(
        message.contains("disagrees with tier"),
        "wrong law fired: {message}"
    );
}

#[test]
fn a_ledger_must_replay_to_the_tier_it_reports() {
    // Every entry is individually well-formed and the verdict agrees with the
    // tier. Only the sequence is a lie: the escalations reach `t2` and the
    // adjudication reports `t0`.
    let json = format!(
        r#"{{"tier":"t0","verdict":"auto","escalations":[{}]}}"#,
        escalation_json("t0", "t2")
    );
    let message = refused::<Adjudication>(&json);
    assert!(message.contains("replays to"), "wrong law fired: {message}");
}

#[test]
fn a_ledger_must_be_continuous() {
    // The ledger is a sequence, not a set: `from` is a statement about the
    // entry before it. Here the second entry starts at `t0` although the first
    // one had already reached `t1`, so this ledger was re-ordered or forged.
    // It still ends at `t2`, so a check that only compared the final tier would
    // pass it.
    let json = format!(
        r#"{{"tier":"t2","verdict":"human","escalations":[{},{}]}}"#,
        escalation_json("t0", "t1"),
        escalation_json("t0", "t2")
    );
    let message = refused::<Adjudication>(&json);
    assert!(
        message.contains("does not replay"),
        "wrong law fired: {message}"
    );
}

#[test]
fn a_ledger_must_start_at_the_bottom() {
    // `Adjudicator::new` starts at `Tier::BOTTOM`, so a first entry that claims
    // to start anywhere else is describing a run that never happened.
    let json = format!(
        r#"{{"tier":"t2","verdict":"human","escalations":[{}]}}"#,
        escalation_json("t1", "t2")
    );
    let message = refused::<Adjudication>(&json);
    assert!(
        message.contains("does not replay"),
        "wrong law fired: {message}"
    );
}

#[test]
fn what_finish_produces_round_trips() {
    // The paired half for all four laws above, stated over a value built the
    // only way a value can be built. If the validation were wrong in the other
    // direction it would reject real bundles, and this is what notices.
    let mut adjudicator = Adjudicator::new();
    adjudicator.escalate(
        Tier::T1,
        ReasonCode::RuleTierAtLeast,
        "public API changed",
        EvidenceRef::Unattributed,
    );
    adjudicator.escalate(
        Tier::T0,
        ReasonCode::CapabilityViolated,
        "a second reason that raises nothing",
        EvidenceRef::Unattributed,
    );
    adjudicator.escalate(
        Tier::T2,
        ReasonCode::UnknownCapability,
        "policy names something this build cannot evaluate",
        EvidenceRef::Unattributed,
    );
    let adjudication = adjudicator.finish();

    let json = serde_json::to_string(&adjudication).expect("serialize");
    let back: Adjudication = accepted(&json);
    assert_eq!(back, adjudication);
    assert_eq!(back.tier, Tier::T2);
    assert_eq!(back.verdict, Verdict::Human);
    assert_eq!(back.escalations.len(), 3);
}

#[test]
fn an_empty_ledger_round_trips() {
    // The `T0`/`auto`/no-escalations bundle is the common case and must not be
    // caught by the replay walk, which starts at `BOTTOM` and has to arrive
    // there.
    let adjudication = Adjudicator::new().finish();
    let json = serde_json::to_string(&adjudication).expect("serialize");
    let back: Adjudication = accepted(&json);
    assert_eq!(back, adjudication);
}

#[test]
fn unknown_keys_are_still_ignored_rather_than_refused() {
    // The strictness asymmetry survives the change. Bundles are archive output:
    // a key written by a newer build must not take the document down. Only the
    // fields that are present are validated.
    let back: Adjudication = accepted(
        r#"{"tier":"t0","verdict":"auto","escalations":[],"written_by_a_newer_build":42}"#,
    );
    assert_eq!(back.tier, Tier::T0);

    let back: Confidence = accepted(r#"{"requirements":0,"a_count_from_the_future":7}"#);
    assert_eq!(back.requirements, 0);
}

// ---------------------------------------------------------------- Confidence

#[test]
fn the_state_counts_must_partition_the_requirements() {
    // The document that motivated this: one requirement, thirty-six answers.
    // `sentence()` printed it verbatim, because the sentence is generated from
    // the counts and so cannot notice that the counts are impossible.
    let message = refused::<Confidence>(
        r#"{"requirements":1,"adopted":9,"ran":9,"skipped":9,"unverified":9}"#,
    );
    assert!(
        message.contains("state counts sum to"),
        "wrong law fired: {message}"
    );

    // The direction that fails open, and the one worth having: a run where four
    // requirements went unanswered, recorded as though none had.
    let message = refused::<Confidence>(r#"{"requirements":8,"adopted":8,"unverified":4}"#);
    assert!(
        message.contains("state counts sum to"),
        "wrong law fired: {message}"
    );
}

#[test]
fn the_state_counts_may_not_overflow_into_agreement() {
    // `usize::MAX` and `1` wrap to `0`. Checked addition is why the sum cannot
    // be made to match by choosing counts that overflow.
    let json = format!(r#"{{"requirements":0,"adopted":{},"ran":1}}"#, usize::MAX);
    let message = refused::<Confidence>(&json);
    assert!(
        message.contains("more than a `usize` can hold"),
        "wrong law fired: {message}"
    );
}

#[test]
fn advisory_may_not_exceed_the_requirements_it_counts() {
    let message = refused::<Confidence>(r#"{"requirements":1,"ran":1,"advisory":99}"#);
    assert!(
        message.contains("advisory requirements out of"),
        "wrong law fired: {message}"
    );
}

#[test]
fn capabilities_may_not_exceed_the_requirements_that_cover_them() {
    let message = refused::<Confidence>(r#"{"requirements":1,"ran":1,"capabilities":4}"#);
    assert!(
        message.contains("capabilities across"),
        "wrong law fired: {message}"
    );
}

#[test]
fn partial_may_not_exceed_the_capabilities_it_is_a_subset_of() {
    // Including the case the `capabilities` field exists to make legible:
    // `capabilities: 0` says the grouping never happened, so a non-zero
    // `partial` claims a disagreement inside a grouping nobody performed.
    let message = refused::<Confidence>(r#"{"requirements":2,"ran":2,"partial":1}"#);
    assert!(
        message.contains("partial across scopes"),
        "wrong law fired: {message}"
    );

    let message =
        refused::<Confidence>(r#"{"requirements":4,"ran":4,"capabilities":2,"partial":3}"#);
    assert!(
        message.contains("partial across scopes"),
        "wrong law fired: {message}"
    );
}

#[test]
fn an_absent_count_still_defaults() {
    // The paired half for the `#[serde(default)]` that moved to the mirror
    // struct. A `confidence` object written before a count existed must still
    // read, and the empty object must still be the zero tally.
    let empty: Confidence = accepted("{}");
    assert_eq!(empty, Confidence::default());

    // A pre-`capabilities`, pre-`advisory` object: the counts that exist are
    // consistent, the ones that do not default to zero.
    let old: Confidence = accepted(r#"{"requirements":3,"adopted":1,"ran":1,"skipped":1}"#);
    assert_eq!(old.requirements, 3);
    assert_eq!(old.capabilities, 0);
    assert_eq!(old.advisory, 0);
}
