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
//! the public API serializes, and that the bytes already in fixtures are
//! unchanged.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use vibe_check_model::{
    Adjudicators, CapabilityId, CapabilityResolution, Enforcement, EvidenceRef, PolicyRef,
    RequirementId, Resolutions, UnverifiedReason,
};

/// The one variant the workspace already writes, byte for byte.
///
/// Every JSON fixture in the tree carries `Unattributed` — it is one of the two
/// variants the internal tagging could encode — so these exact bytes are the
/// whole of `EvidenceRef`'s observable history. Asserting them as a literal is
/// the evidence that changing the tagging broke no reader that exists: had the
/// encoding of this variant moved, every stored bundle would have to be
/// migrated, and this test would say so rather than a reviewer having to.
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
    let evidence = EvidenceRef::Requirement(RequirementId::new("req_tests-pass_all"));
    assert_eq!(
        serde_json::to_string(&evidence).expect("serialize"),
        r#"{"kind":"requirement","ref":"req_tests-pass_all"}"#
    );
    assert_eq!(
        serde_json::from_str::<EvidenceRef>(r#"{"kind":"requirement","ref":"req_tests-pass_all"}"#)
            .expect("deserialize"),
        evidence
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
    resolutions.insert(
        RequirementId::new("req_apple"),
        Enforcement::Enforcing,
        CapabilityResolution::Unverified {
            reason: UnverifiedReason::MissingEvidence,
        },
    );
    let mut adjudicators = Adjudicators::new();
    resolutions.account_into(&mut adjudicators);
    let ledger = adjudicators.finish().0.into_adjudication().escalations;
    assert_eq!(ledger.len(), 1);

    let json = serde_json::to_string(&ledger).expect("the ledger serializes");
    assert!(
        json.contains(r#""kind":"requirement","ref":"req_apple""#),
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
