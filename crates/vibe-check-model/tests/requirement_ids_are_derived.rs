//! A requirement identifier is a function of what it identifies.
//!
//! `RequirementId` is the key of [`Resolutions`], so two requirements that share
//! an identifier are one entry in that map and one of the two resolutions is
//! displaced. If the displaced one was failing, the run reports a question as
//! answered that nothing ever answered. That is the fail-open this file guards.
//!
//! Three properties, and they are not the same property said three ways:
//!
//! 1. **Distinct scopes get distinct identifiers.** The collision property
//!    proper, over generated crate and path sets.
//! 2. **The same scope gets the same identifier whatever order it was built
//!    in.** The planner unions scopes from a diff walk; two runs that visited
//!    the same crates in different orders describe the same requirement and must
//!    not schedule it twice under two names.
//! 3. **The identifier does not drift.** A committed golden value, because this
//!    string reaches `EvidenceRef::Requirement` inside every bundle and changing
//!    the derivation silently invalidates every historical escalation reference.
//!    A test that recomputed the expected value from the same code would agree
//!    with any change, including a wrong one.
//!
//! # What is *not* asserted here
//!
//! That a `RequirementId` arriving over the wire was derived at all.
//! `from_wire` checks the shape — `req_`, a readable capability, `_`, sixteen
//! hex characters — and cannot check more, because the digest is non-invertible
//! and the wire form does not carry the scope it was taken over. The shape check
//! rejects `req_tests-pass_all`, which is what a person writes when inventing an
//! identifier rather than reading one back. It does not reject sixteen hex
//! characters somebody made up. That gap is real and is documented on
//! `from_wire` rather than papered over here.
//!
//! [`Resolutions`]: vibe_check_model::Resolutions

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use proptest::prelude::*;
use vibe_check_model::{
    CapabilityId, CrateId, RequirementId, RequirementIdError, RequirementScope, ScopeError,
};

/// Build a scope from string slices, panicking on a malformed fixture.
fn scope(crates: &[&str], paths: &[&str]) -> RequirementScope {
    RequirementScope::new(
        crates.iter().map(CrateId::new).collect::<Vec<_>>(),
        paths.to_vec(),
    )
    .expect("a well-formed fixture scope")
}

/// The identifier for a capability over a scope, as a string.
fn derive(capability: &str, crates: &[&str], paths: &[&str]) -> String {
    RequirementId::derive(&CapabilityId::new(capability), &scope(crates, paths))
        .as_str()
        .to_owned()
}

// --- the golden ------------------------------------------------------------

/// The derivation is pinned, because changing it invalidates the past.
///
/// Every identifier in every bundle already written is a claim about this
/// function's output. A change here does not fail loudly — it makes old
/// escalation references point at requirements that no longer exist, which
/// reads as "nothing escalated" to anything that resolves them. So the value is
/// committed rather than recomputed, and updating it is a deliberate act with a
/// migration attached.
#[test]
fn the_derivation_is_pinned() {
    assert_eq!(
        derive(
            "tests-pass",
            &["kono-core"],
            &["crates/kono-core/src/lib.rs"]
        ),
        "req_tests-pass_42be8fad964a6d73"
    );
    assert_eq!(
        derive("tests-pass", &[], &[]),
        "req_tests-pass_e096f630363802f0",
        "the whole-repository scope is pinned too — it is the common case"
    );
}

/// The pinned values above are what the *documented* encoding produces.
///
/// The golden test alone would keep passing if `canonical_bytes` and the
/// documentation drifted apart together — it only pins whatever the code does.
/// This one spells the bytes out by hand, from the prose in `scope.rs`, and
/// hashes them without going through the encoder. Two independent statements of
/// one encoding, which is what makes the goldens above mean something.
#[test]
fn the_pinned_values_are_what_the_documented_encoding_produces() {
    let expected = |bytes: &[u8]| blake3::hash(bytes).to_hex().as_str()[..16].to_owned();

    // capability ⧺ RS ⧺ (crates joined by US) ⧺ RS ⧺ (paths joined by US)
    assert_eq!(
        derive(
            "tests-pass",
            &["kono-core"],
            &["crates/kono-core/src/lib.rs"]
        ),
        format!(
            "req_tests-pass_{}",
            expected(b"tests-pass\x1ekono-core\x1ecrates/kono-core/src/lib.rs")
        )
    );
    assert_eq!(
        derive("tests-pass", &["a", "b"], &["x", "y"]),
        format!(
            "req_tests-pass_{}",
            expected(b"tests-pass\x1ea\x1fb\x1ex\x1fy")
        )
    );
    assert_eq!(
        derive("tests-pass", &[], &[]),
        format!("req_tests-pass_{}", expected(b"tests-pass\x1e\x1e"))
    );
}

/// The form is what the issue specified and what `from_wire` accepts.
#[test]
fn a_derived_id_has_the_documented_form() {
    let id = RequirementId::derive(
        &CapabilityId::new("mutants-in-diff-killed"),
        &scope(&["@workspace"], &[]),
    );
    let rest = id
        .as_str()
        .strip_prefix("req_")
        .expect("every requirement id starts with `req_`");
    let (capability, digest) = rest.rsplit_once('_').expect("a digest field");
    assert_eq!(capability, "mutants-in-diff-killed");
    assert_eq!(digest.len(), 16);
    assert!(
        digest
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    );

    // And the round trip closes: what `derive` mints, `from_wire` accepts.
    assert_eq!(
        RequirementId::from_wire(id.as_str()).expect("derived ids are well-formed"),
        id
    );
}

/// The readable half survives the four hops a leaf identifier has to survive.
///
/// This string becomes a job-matrix entry and a `--id` argument. A capability
/// carrying a `/`, a space, or an uppercase letter would break one of those
/// hops, so `derive` folds anything outside `[a-z0-9.-]` to `-`.
#[test]
fn the_readable_half_is_safe_to_interpolate() {
    let id = derive("Loom Clean/v2", &[], &[]);
    let capability = id
        .strip_prefix("req_")
        .and_then(|rest| rest.rsplit_once('_'))
        .expect("the documented form")
        .0;
    assert_eq!(capability, "loom-clean-v2");
    assert!(RequirementId::from_wire(&id).is_ok());
}

/// Folding the readable half is lossy; the identifier is not.
///
/// If the digest were taken over the folded string this would be a collision,
/// and a collision is the whole failure mode. It is taken over the capability as
/// written, so these two differ.
#[test]
fn two_capabilities_that_read_alike_still_differ() {
    let one = derive("loom clean", &[], &[]);
    let other = derive("loom/clean", &[], &[]);
    assert_eq!(
        one.rsplit_once('_').expect("a digest").0,
        other.rsplit_once('_').expect("a digest").0,
        "the readable halves are deliberately identical"
    );
    assert_ne!(one, other, "and the identifiers are not");
}

// --- order independence ----------------------------------------------------

/// The same scope built in a different order is the same identifier.
#[test]
fn insertion_order_does_not_reach_the_identifier() {
    let forwards = derive(
        "tests-pass",
        &["a", "b", "c"],
        &["src/one.rs", "src/two.rs"],
    );
    let backwards = derive(
        "tests-pass",
        &["c", "b", "a"],
        &["src/two.rs", "src/one.rs"],
    );
    let with_repeats = derive(
        "tests-pass",
        &["b", "a", "c", "a"],
        &["src/two.rs", "src/one.rs", "src/two.rs"],
    );
    assert_eq!(forwards, backwards);
    assert_eq!(forwards, with_repeats);
}

/// Which set a name landed in is part of the identity.
///
/// A single-separator encoding would make these two the same requirement, which
/// is the exact shape of the bug: a crate named `b` and a path named `b` are
/// different scopes and must not resolve into one map entry.
#[test]
fn a_crate_and_a_path_are_not_interchangeable() {
    assert_ne!(
        derive("tests-pass", &["b"], &[]),
        derive("tests-pass", &[], &["b"])
    );
    assert_ne!(
        derive("tests-pass", &["a", "b"], &[]),
        derive("tests-pass", &["a"], &["b"])
    );
}

/// The capability is part of the identity, not only of the label.
#[test]
fn two_capabilities_over_one_scope_are_two_requirements() {
    let scope = &["kono-core"];
    assert_ne!(
        derive("tests-pass", scope, &[]),
        derive("miri-clean", scope, &[])
    );
}

// --- what a scope refuses --------------------------------------------------

/// A traversing path is an error, not a silent normalization.
///
/// The issue's own example. `foo/../bar` and `bar` may or may not name the same
/// file — that depends on symlinks — so a scope that rewrote one into the other
/// would be claiming something it cannot know.
#[test]
fn a_scope_refuses_what_it_cannot_encode_unambiguously() {
    let rejected: Vec<(&str, &str)> = vec![
        ("foo/../bar", "traversal"),
        ("./src", "leading ./"),
        ("src/", "trailing slash"),
        (".", "the current directory"),
        ("..", "the parent directory"),
        ("", "empty"),
        ("a//b", "an empty component"),
        ("/etc/passwd", "absolute"),
        ("a\u{1f}b", "a unit separator"),
        ("a\u{1e}b", "a record separator"),
    ];
    for (path, why) in rejected {
        assert!(
            RequirementScope::new(Vec::<CrateId>::new(), [path]).is_err(),
            "{path:?} must be rejected: {why}"
        );
    }
    assert_eq!(
        RequirementScope::new([CrateId::new("")], Vec::<String>::new()),
        Err(ScopeError::EmptyCrate)
    );
}

// --- the wire filter -------------------------------------------------------

/// `from_wire` rejects an identifier nobody derived.
#[test]
fn an_invented_identifier_does_not_come_back_off_the_wire() {
    for bad in [
        "req_tests-pass_all",
        "tests-pass",
        "req_tests-pass",
        "req_tests-pass_DD2E8DBBEC9C4C22",
        "req_tests-pass_dd2e8dbbec9c4c2",
        "req_tests-pass_dd2e8dbbec9c4c222",
        "req_tests_pass_dd2e8dbbec9c4c22",
        "req_tests pass_dd2e8dbbec9c4c22",
        "",
    ] {
        assert!(
            RequirementId::from_wire(bad).is_err(),
            "{bad:?} must not read back as a requirement id"
        );
    }
    assert_eq!(
        RequirementId::from_wire("nope"),
        Err(RequirementIdError::MissingPrefix {
            got: "nope".to_owned()
        })
    );
}

/// And so does deserialization, which is the hop that matters.
///
/// A bundle or a plan document is where a hand-invented identifier would
/// actually arrive from. `#[derive(Deserialize)]` with `#[serde(transparent)]`
/// would accept any string at all, so `RequirementId` writes its own — the same
/// move `LeafId` makes, for the same reason.
#[test]
fn deserialization_runs_the_same_filter() {
    let id = RequirementId::derive(&CapabilityId::new("tests-pass"), &scope(&["a"], &[]));
    let json = serde_json::to_string(&id).expect("serialize");
    assert_eq!(json, format!("\"{id}\""), "the wire form is a bare string");
    assert_eq!(
        serde_json::from_str::<RequirementId>(&json).expect("deserialize"),
        id
    );

    let error = serde_json::from_str::<RequirementId>(r#""req_tests-pass_all""#)
        .expect_err("must not deserialize");
    assert!(
        error.to_string().contains("hex"),
        "the serde error carries the rule that failed, got {error}"
    );
}

// --- the collision property ------------------------------------------------

/// A scope's inputs, as generated: crate names and repository-relative paths
/// drawn from alphabets `RequirementScope::new` accepts.
fn scope_inputs() -> impl Strategy<Value = (BTreeSet<String>, BTreeSet<String>)> {
    (
        prop::collection::btree_set("[a-z@][a-z0-9._@-]{0,5}", 0..4),
        prop::collection::btree_set("[a-z][a-z0-9._-]{0,4}(/[a-z][a-z0-9._-]{0,4}){0,2}", 0..4),
    )
}

proptest! {
    /// Distinct *(capability, scope)* pairs get distinct identifiers.
    ///
    /// Stated over the pair rather than the scope alone because the capability
    /// is digested too: a requirement is the pair, and the pair is what must be
    /// injective into the map key.
    #[test]
    fn distinct_requirements_never_collide(
        capability_one in "[a-z][a-z-]{0,8}",
        (crates_one, paths_one) in scope_inputs(),
        capability_two in "[a-z][a-z-]{0,8}",
        (crates_two, paths_two) in scope_inputs(),
    ) {
        let same = capability_one == capability_two
            && crates_one == crates_two
            && paths_one == paths_two;

        let one = RequirementId::derive(
            &CapabilityId::new(&capability_one),
            &RequirementScope::new(
                crates_one.iter().map(CrateId::new).collect::<Vec<_>>(),
                paths_one.iter().cloned().collect::<Vec<_>>(),
            ).expect("generated members are accepted"),
        );
        let two = RequirementId::derive(
            &CapabilityId::new(&capability_two),
            &RequirementScope::new(
                crates_two.iter().map(CrateId::new).collect::<Vec<_>>(),
                paths_two.iter().cloned().collect::<Vec<_>>(),
            ).expect("generated members are accepted"),
        );

        if same {
            prop_assert_eq!(one, two, "the same requirement derives the same identifier");
        } else {
            prop_assert_ne!(one, two, "two requirements must not share a map key");
        }
    }

    /// Whatever `derive` mints, `from_wire` and serde accept.
    ///
    /// The two directions have to agree, or an identifier this build wrote into
    /// a bundle is one the next build refuses to read.
    #[test]
    fn every_derived_identifier_survives_the_wire(
        capability in ".{0,20}",
        (crates, paths) in scope_inputs(),
    ) {
        let id = RequirementId::derive(
            &CapabilityId::new(&capability),
            &RequirementScope::new(
                crates.iter().map(CrateId::new).collect::<Vec<_>>(),
                paths.into_iter().collect::<Vec<_>>(),
            ).expect("generated members are accepted"),
        );
        prop_assert_eq!(
            RequirementId::from_wire(id.as_str()).expect("a derived id is well-formed"),
            id.clone()
        );
        let json = serde_json::to_string(&id).expect("serialize");
        prop_assert_eq!(
            serde_json::from_str::<RequirementId>(&json).expect("deserialize"),
            id
        );
    }

    /// The order the planner unioned a scope in does not reach the identifier.
    #[test]
    fn shuffling_the_inputs_changes_nothing(
        (crates, paths) in scope_inputs(),
        seed in any::<u64>(),
    ) {
        // A cheap deterministic rotation: enough to change the argument order
        // without needing a shuffling strategy over two collections at once.
        let rotate = |mut members: Vec<String>| {
            if !members.is_empty() {
                #[allow(clippy::cast_possible_truncation)]
                let by = (seed as usize) % members.len();
                members.rotate_left(by);
            }
            members
        };

        let ascending = RequirementScope::new(
            crates.iter().map(CrateId::new).collect::<Vec<_>>(),
            paths.iter().cloned().collect::<Vec<_>>(),
        ).expect("accepted");
        let rotated = RequirementScope::new(
            rotate(crates.into_iter().collect()).into_iter().map(CrateId::new).collect::<Vec<_>>(),
            rotate(paths.into_iter().collect()),
        ).expect("accepted");

        let capability = CapabilityId::new("tests-pass");
        prop_assert_eq!(
            RequirementId::derive(&capability, &ascending),
            RequirementId::derive(&capability, &rotated)
        );
    }
}
