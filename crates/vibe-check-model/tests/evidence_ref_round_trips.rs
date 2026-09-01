//! `EvidenceRef` reaches the wire, and the bytes it already wrote did not move.
//!
//! `EvidenceRef` was `#[serde(tag = "kind")]`. Two defects followed, both of
//! which compiled cleanly and failed at run time:
//!
//! 1. `Requirement`, `Capability`, `Crate`, `Rule` and `Path` are newtypes over
//!    `#[serde(transparent)]` string identifiers, and serde's internal tagging
//!    rejects any payload that is not a map. Every escalation the accounting
//!    path produces carries `Requirement`, so no ledger and no bundle holding
//!    one could reach JSON.
//! 2. `PolicyRef` has a field named `kind`, and the tag key was also `"kind"`.
//!    Internally tagged, both landed in one object: `serde_json::to_string`
//!    emitted a duplicate key that would not deserialize, and
//!    `serde_json::to_value` — `serde_json::Map` is a `BTreeMap` here — silently
//!    replaced the tag `"policy"` with the field's value, naming a different,
//!    real variant.
//!
//! Adjacent tagging (`tag = "kind", content = "ref"`) fixes both: string
//! payloads serialize under `ref`, and `PolicyRef` nests one level below the
//! tag it was overwriting.
//!
//! The exhaustive per-variant guard is **not** here. `#[non_exhaustive]` binds
//! on any enum from another crate, and an integration test is another crate, so
//! a `match` written here would need a wildcard arm and would go on compiling
//! when a variant was added. It lives in `reason.rs`'s own `#[cfg(test)]`
//! module, where the enum is local and the `match` is genuinely exhaustive.
//! What is left for this file is what only an external consumer can prove: that
//! the public API serializes, and what the bytes are.
//!
//! Those byte assertions are prospective, not archaeological. There are no JSON
//! fixture files in this repository and no bundle has ever been written, so
//! nothing on disk was consulted or could be — the literals here are the first
//! record of this encoding, written so that the *next* change to it has to move
//! a test.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use vibe_check_model::{
    Adjudicators, CapabilityId, CapabilityResolution, Enforcement, EvidenceRef, PolicyRef,
    RequirementId, Resolutions, UnverifiedReason,
};

/// The one variant the workspace constructs outside a test, byte for byte.
///
/// `Unattributed` is what every non-test construction in the tree reaches for,
/// and it is one of the two variants internal tagging could encode, so it is
/// the variant most likely to have been written by anything. These bytes are
/// unchanged from before the retagging — measured on both sides, not inferred —
/// which is the concrete form of the claim that this change breaks no reader.
///
/// That claim is cheap to make good on right now: nothing has been written yet.
/// The assertion is here for later, when it stops being cheap. Note the
/// narrower scope than "no bytes moved" — see `Flag`, whose encoding did move,
/// documented on the pull request.
#[test]
fn unattributed_keeps_the_bytes_it_already_wrote() {
    assert_eq!(
        serde_json::to_string(&EvidenceRef::Unattributed).expect("serialize"),
        r#"{"kind":"unattributed"}"#
    );
    assert_eq!(
        serde_json::from_str::<EvidenceRef>(r#"{"kind":"unattributed"}"#).expect("deserialize"),
        EvidenceRef::Unattributed
    );
}

/// A string-payload variant lands under `ref`, not spread into the tag object.
///
/// This is the shape the five broken variants now share, pinned as literal
/// bytes so that a later switch to untagged or externally tagged encoding is a
/// visible edit to a test rather than a silent change to every bundle.
#[test]
fn a_string_payload_is_adjacent_to_its_tag() {
    const WIRE: &str = "req_tests-pass_00000000000000000000000000000000";
    let evidence = EvidenceRef::Requirement(
        RequirementId::from_wire(WIRE).expect("a well-formed fixture identifier"),
    );
    assert_eq!(
        serde_json::to_string(&evidence).expect("serialize"),
        format!(r#"{{"kind":"requirement","ref":"{WIRE}"}}"#)
    );
    assert_eq!(
        serde_json::from_str::<EvidenceRef>(&format!(r#"{{"kind":"requirement","ref":"{WIRE}"}}"#))
            .expect("deserialize"),
        evidence
    );

    // The other half of the shape check: `RequirementId` hand-writes
    // `Deserialize` so the wire form is filtered, and an identifier somebody
    // invented rather than derived does not arrive as one.
    assert!(
        serde_json::from_str::<EvidenceRef>(r#"{"kind":"requirement","ref":"req_tests-pass_all"}"#)
            .is_err(),
        "a requirement id that was never derived must not deserialize"
    );
}

/// `Policy` round-trips, which it never has.
///
/// It is constructed nowhere in the tree and no test serialized it, which is
/// why the tag collision was latent rather than loud. `blob_sha` is present
/// here and absent in the sibling test below, because it is
/// `skip_serializing_if` and the absent case is the one where a missing key
/// could be mistaken for a missing tag.
#[test]
fn a_policy_ref_round_trips_with_its_colliding_kind_field() {
    let policy = PolicyRef {
        path: ".vibe-check/policy.toml".into(),
        kind: "rule".into(),
        id: "core-unsafe".into(),
        blob_sha: Some("e41d".into()),
    };
    let json = serde_json::to_string(&EvidenceRef::Policy(policy.clone())).expect("serialize");
    assert_eq!(
        json,
        r#"{"kind":"policy","ref":{"path":".vibe-check/policy.toml","kind":"rule","id":"core-unsafe","blob_sha":"e41d"}}"#
    );
    assert_eq!(
        serde_json::from_str::<EvidenceRef>(&json).expect("deserialize"),
        EvidenceRef::Policy(policy)
    );
}

/// The same, with the optional field skipped.
#[test]
fn a_policy_ref_round_trips_without_a_blob_sha() {
    let policy = PolicyRef {
        path: ".vibe-check/policy.toml".into(),
        kind: "skip".into(),
        id: "core-unsafe".into(),
        blob_sha: None,
    };
    let json = serde_json::to_string(&EvidenceRef::Policy(policy.clone())).expect("serialize");
    assert_eq!(
        json,
        r#"{"kind":"policy","ref":{"path":".vibe-check/policy.toml","kind":"skip","id":"core-unsafe"}}"#
    );
    assert_eq!(
        serde_json::from_str::<EvidenceRef>(&json).expect("deserialize"),
        EvidenceRef::Policy(policy)
    );
}

/// `to_value` and `to_string` agree.
///
/// They did not before. `serde_json::Map` is a `BTreeMap` in this build — no
/// `preserve_order` feature — so the duplicate `kind` key that `to_string`
/// emitted became a single overwritten key in `to_value`, and the two encoders
/// disagreed about which variant the value even was. Anything that renders a
/// bundle through `Value` rather than straight to bytes depended on that.
#[test]
fn the_two_encoders_agree_about_policy() {
    let evidence = EvidenceRef::Policy(PolicyRef {
        path: ".vibe-check/policy.toml".into(),
        kind: "rule".into(),
        id: "core-unsafe".into(),
        blob_sha: None,
    });
    let via_value = serde_json::to_value(&evidence).expect("to_value");
    let via_string: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&evidence).expect("to_string"))
            .expect("reparse");
    assert_eq!(via_value, via_string);
    assert_eq!(
        serde_json::from_value::<EvidenceRef>(via_value).expect("from_value"),
        evidence
    );
}

/// An escalation ledger reaches JSON.
///
/// The end the fix exists for. `Resolutions::account_into` is the workspace's
/// only production `EvidenceRef` construction and it builds `Requirement`, so
/// until now the whole accounting path terminated in a serializer error. This
/// goes through the public API, the same way a bundle writer would.
#[test]
fn an_escalation_ledger_reaches_json() {
    let mut resolutions = Resolutions::new();
    let apple = RequirementId::from_wire("req_apple_00000000000000000000000000000000")
        .expect("a well-formed fixture identifier");
    let displaced = resolutions.insert(
        apple.clone(),
        Enforcement::Enforcing,
        CapabilityResolution::Unverified {
            reason: UnverifiedReason::MissingEvidence,
        },
    );
    assert!(displaced.is_none());
    let mut adjudicators = Adjudicators::new();
    resolutions.account_into(&mut adjudicators);
    let ledger = adjudicators.finish().0.into_adjudication().escalations;
    assert_eq!(ledger.len(), 1);

    let json = serde_json::to_string(&ledger).expect("the ledger serializes");
    assert!(
        json.contains(&format!(r#""kind":"requirement","ref":"{apple}""#)),
        "the escalation names its requirement on the wire: {json}"
    );
    let back: Vec<vibe_check_model::Escalation> =
        serde_json::from_str(&json).expect("the ledger deserializes");
    assert_eq!(back, ledger);
}

/// A capability escalation, the other non-`Unattributed` variant the tree
/// builds, also survives — in a test today, but the accumulator constructs it.
#[test]
fn a_capability_ref_round_trips() {
    let evidence = EvidenceRef::Capability(CapabilityId::new("loom-clean"));
    let json = serde_json::to_string(&evidence).expect("serialize");
    assert_eq!(json, r#"{"kind":"capability","ref":"loom-clean"}"#);
    assert_eq!(
        serde_json::from_str::<EvidenceRef>(&json).expect("deserialize"),
        evidence
    );
}
