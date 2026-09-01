//! The committed golden bundle, and the two digests it must produce.
//!
//! `clippy.toml` calls the replay corpus "the real guarantee" and describes what
//! it protects: same diff plus same policy yields the same verdict. The corpus
//! itself needs a bundle writer and does not exist. This file is its smallest
//! honest precursor — one bundle, checked in as JSON text, and the two digest
//! values it produces today.
//!
//! # Why the bundle is a committed literal and not built in Rust
//!
//! A fixture constructed from the model's own types regenerates itself. Rename a
//! field, and the constructor is renamed with it, the JSON changes shape, and
//! the digest changes — silently, because nothing in the test ever named the old
//! shape. That fixture proves the canonicalizer is deterministic and nothing
//! else.
//!
//! A committed JSON literal is a statement about the wire form independent of
//! the types that write it. Change a field name and the parse fails or the
//! digest moves, and either way the diff that did it says so. That is the whole
//! point: the digest is over the *document*, so the fixture has to be a
//! document.
//!
//! # When these constants change
//!
//! Legitimately, on exactly two kinds of change: an edit to
//! [`VERDICT_DIGEST_PATHS`] or [`BUNDLE_ID_EXCLUDED_PATHS`], and a change to
//! canonicalization itself. Both are deliberate acts. Anything else moving these
//! numbers is a bug, and the point of committing them is that it cannot happen
//! quietly. Do not update a constant to match a value you did not intend to
//! change.
//!
//! The bundle below exercises every [`EvidenceRef`] variant, both ledgers, both
//! optional `core` fields, and an `extensions` section this build does not
//! understand.

// A test is the one place a panic is the right failure mode, and this file's
// whole job is to fail loudly when a digest moves.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use vibe_check_model::{
    BUNDLE_ID_EXCLUDED_PATHS, EvidenceBundle, VERDICT_DIGEST_PATHS, bundle_id, canonicalize,
    verdict_digest,
};

/// The committed document. Text, not a constructor call — see the module docs.
const GOLDEN: &str = include_str!("golden/bundle.json");

/// The digest of the verdict-bearing subtree of [`GOLDEN`].
const GOLDEN_VERDICT_DIGEST: &str =
    "blake3:dc4661870d025987dbd4835f50e941a4d24fa08e6f433acce81ca7bff18942d0";

/// The content address of the whole of [`GOLDEN`], minus the paths on
/// [`BUNDLE_ID_EXCLUDED_PATHS`].
const GOLDEN_BUNDLE_ID: &str =
    "blake3:b83dc1429ef82e5517d8975c5ea8c7caeee023550970894221500d33896388fe";

fn golden() -> EvidenceBundle {
    serde_json::from_str(GOLDEN).expect(
        "the committed golden bundle must still parse. If this fails, a field \
         it names was renamed or removed — which is a change to the wire \
         contract, not to this test.",
    )
}

#[test]
fn the_golden_bundle_digests_to_the_committed_values() {
    let b = golden();
    assert_eq!(
        verdict_digest(&b).expect("digest").as_str(),
        GOLDEN_VERDICT_DIGEST,
        "the verdict digest of the committed bundle moved. Either the \
         inclusion list changed, or canonicalization changed, or something \
         changed one of them without meaning to — and the third case is what \
         this test is for."
    );
    assert_eq!(
        bundle_id(&b).expect("digest").as_str(),
        GOLDEN_BUNDLE_ID,
        "the bundle id of the committed bundle moved"
    );
}

#[test]
fn the_golden_bundle_survives_a_round_trip_unchanged() {
    // The digests above are over what this build *reads*. If reading and
    // rewriting the document is not the identity, they are digests of something
    // the archive does not contain.
    let b = golden();
    let rewritten = serde_json::to_value(&b).expect("serialize");
    let original: serde_json::Value = serde_json::from_str(GOLDEN).expect("parse");
    assert_eq!(
        canonicalize(&rewritten).expect("canonicalize"),
        canonicalize(&original).expect("canonicalize"),
        "reading and rewriting the golden bundle changed it. A section this \
         build predates must survive the trip — see the extensions field."
    );
}

#[test]
fn the_golden_bundle_populates_every_live_inclusion_path() {
    // A golden whose optional fields are absent digests a smaller document than
    // it appears to, and would still pass the assertions above while covering
    // less than the list claims.
    let value: serde_json::Value = serde_json::from_str(GOLDEN).expect("parse");
    for p in VERDICT_DIGEST_PATHS {
        assert!(
            vibe_check_model::digest::path_resolves(&value, p.path),
            "`{}` is on the verdict inclusion list and is not present in the \
             golden bundle, so the golden does not exercise it",
            p.path
        );
    }

    let mut b = golden();
    b.core.bundle_id = "something else entirely".into();
    assert_eq!(
        bundle_id(&b).expect("digest").as_str(),
        GOLDEN_BUNDLE_ID,
        "the golden carries a `core.bundle_id`, and the bundle id must not \
         depend on it"
    );
}

#[test]
fn a_release_does_not_move_the_golden_verdict_digest() {
    // The property the replay corpus depends on, stated against a committed
    // number rather than against another computed one: cutting a release, or
    // registering an analyzer, must leave a historical verdict digest exactly
    // where it was. Compared against the constant, not against
    // `verdict_digest(&golden())`, because two computed values agreeing proves
    // only that the same code ran twice.
    let mut b = golden();
    b.generator.version = "9.9.9".into();
    b.generator.git_sha = Some("0000000000000000000000000000000000000000".into());
    b.generator.registry_digest = "blake3:ffff".into();
    b.core.bundle_id = "anything".into();
    b.core.verdict_digest = "anything".into();
    b.adjudication.escalations[0].detail = "/tmp/.tmpQ7x2/target/nextest".into();
    b.extensions
        .insert("added_later".into(), serde_json::Value::Bool(true));

    assert_eq!(
        verdict_digest(&b).expect("digest").as_str(),
        GOLDEN_VERDICT_DIGEST,
        "none of these are the verdict, and none of them may move it"
    );
    assert_ne!(
        bundle_id(&b).expect("digest").as_str(),
        GOLDEN_BUNDLE_ID,
        "all but `core.bundle_id` are content, and content is what the bundle \
         id addresses"
    );
}

#[test]
fn the_excluded_paths_are_present_in_the_golden_and_still_ignored() {
    // An exclusion that is not exercised proves nothing. Every path on
    // `BUNDLE_ID_EXCLUDED_PATHS` must actually be in the document, or the
    // assertion that `bundle_id` ignores it is vacuous.
    let value: serde_json::Value = serde_json::from_str(GOLDEN).expect("parse");
    for p in BUNDLE_ID_EXCLUDED_PATHS {
        assert!(
            vibe_check_model::digest::path_resolves(&value, p.path),
            "`{}` is excluded from the bundle id but absent from the golden, \
             so nothing proves it is excluded",
            p.path
        );
    }

    let mut b = golden();
    b.core.bundle_id = "something else entirely".into();
    assert_eq!(
        bundle_id(&b).expect("digest").as_str(),
        GOLDEN_BUNDLE_ID,
        "the golden carries a `core.bundle_id`, so this proves the exclusion \
         rather than assuming it"
    );
}
