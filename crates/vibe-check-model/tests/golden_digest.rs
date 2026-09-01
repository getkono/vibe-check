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
///
/// Both constants moved once, deliberately, when this branch merged #31. The
/// golden's escalation named its requirement `r_9f3c1a`, and `RequirementId`
/// now refuses anything that is not `req_<name>_<32 hex>` — so the old value
/// could not be deserialized at all, the fixture had to change, and the
/// digests followed it. That is the intended-change case the module docs
/// describe, not canonicalization drift.
const GOLDEN_VERDICT_DIGEST: &str =
    "blake3:503609e33409024b863e2b4c4c33f2bbbaed35086176f3fe98f8bfabe14d7bed";

/// The content address of the whole of [`GOLDEN`], minus the paths on
/// [`BUNDLE_ID_EXCLUDED_PATHS`].
const GOLDEN_BUNDLE_ID: &str =
    "blake3:67c1772a0f87ad6b1268300140d037aef5132ced916800fc8af646276fbb7310";

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
fn a_top_level_section_from_a_newer_build_reaches_the_bundle_id() {
    // The half of the guarantee that holds. `EvidenceBundle` carries a
    // flattened `extensions`, so a section this build has never heard of
    // survives the read and changes the content address.
    let mut value: serde_json::Value = serde_json::from_str(GOLDEN).expect("parse");
    value
        .as_object_mut()
        .expect("object")
        .insert("coverage".into(), serde_json::json!({ "line_pct": 91 }));
    let b: EvidenceBundle = serde_json::from_value(value).expect("parse");

    assert!(b.extensions.contains_key("coverage"));
    assert_ne!(
        bundle_id(&b).expect("digest").as_str(),
        GOLDEN_BUNDLE_ID,
        "a section nobody thought about must change the identity of the \
         document that carries it"
    );
}

#[test]
fn a_nested_key_from_a_newer_build_is_dropped_and_the_bundle_id_does_not_notice() {
    // The half that does NOT hold, asserted on purpose.
    //
    // Only `EvidenceBundle` has a flattened bag. `BundleCore`, `Generator`,
    // `Adjudication`, `Escalation`, `Confidence`, `Provenance`, `EvidenceRef`
    // and `Location` have neither a bag nor `deny_unknown_fields`, so serde
    // drops an unknown key inside any of them on read — and `bundle_id` is
    // computed from what was read. So `bundle_id` addresses the top-level
    // sections, not the whole document, and two documents that differ only
    // below the root are the same artifact as far as it is concerned.
    //
    // This test exists so the limit is visible rather than inherited. The test
    // above it passes while the general claim is false everywhere below the
    // root, because the golden's own unknown sections happen to sit at the
    // root — it is scoped exactly where the property holds.
    //
    // The fix is nested extension bags, which is #28. It is deliberately NOT
    // done here: giving `BundleCore` a flattened `extensions` would add a field
    // to the permanently frozen vocabulary, which is the one change AGENTS.md
    // §1 forbids outright. It also narrows AGENTS.md §5 — unknown keys in
    // bundles are preserved *at the top level* — and that narrowing is now
    // written down in `digest.rs` rather than left as a surprise.
    //
    // When #28 lands, this test fails. That is what it is for: change the
    // assertions to `assert!(present)` and `assert_ne!`, and delete the gap
    // paragraph in `digest.rs`.
    let mut value: serde_json::Value = serde_json::from_str(GOLDEN).expect("parse");
    value["core"]["evaluated_at"] = serde_json::json!("2026-08-31T00:00:00Z");
    value["generator"]["host"] = serde_json::json!("gha-runner-7");
    value["confidence"]["future_count"] = serde_json::json!(7);
    value["adjudication"]["escalations"][0]["future"] = serde_json::json!("x");

    let b: EvidenceBundle = serde_json::from_value(value).expect("parse");
    let rewritten = serde_json::to_value(&b).expect("serialize");

    assert!(
        rewritten["core"].get("evaluated_at").is_none(),
        "if this key now survives, nested bags landed — see #28 and the note \
         above"
    );
    assert!(rewritten["generator"].get("host").is_none());
    assert!(rewritten["confidence"].get("future_count").is_none());
    assert!(
        rewritten["adjudication"]["escalations"][0]
            .get("future")
            .is_none()
    );

    assert_eq!(
        bundle_id(&b).expect("digest").as_str(),
        GOLDEN_BUNDLE_ID,
        "four nested keys a newer build wrote, and the content address of the \
         artifact is unchanged. Recorded, not endorsed."
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

/// Overwrite every value `path` reaches in `doc` with `replacement`.
///
/// A deliberately independent walker: the point of the test below is that a
/// path on the exclusion list is *actually* ignored, and reusing the crate's
/// own projection would let one bug hide another. `[]` fans out over an array;
/// every other segment is an object key. Returns how many values it changed,
/// so a path that reached nothing cannot pass for a path that was ignored.
fn overwrite_at(doc: &mut serde_json::Value, path: &str, replacement: &serde_json::Value) -> usize {
    fn go(v: &mut serde_json::Value, segs: &[&str], replacement: &serde_json::Value) -> usize {
        let Some((head, tail)) = segs.split_first() else {
            *v = replacement.clone();
            return 1;
        };
        if *head == "[]" {
            return match v.as_array_mut() {
                Some(items) => items.iter_mut().map(|i| go(i, tail, replacement)).sum(),
                None => 0,
            };
        }
        match v.get_mut(*head) {
            Some(child) => go(child, tail, replacement),
            None => 0,
        }
    }
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    go(doc, &segs, replacement)
}

#[test]
fn every_excluded_path_is_present_in_the_golden_and_provably_ignored() {
    // The inclusion side is checked generically, one assertion per entry. The
    // exclusion side used to prove exactly one path — `core.bundle_id` — by
    // name, which meant a second entry could be added and be a complete no-op
    // with every test still green. On an exclusion list a no-op entry fails
    // open: the path stays in the digest while the list says it does not.
    //
    // So: for each entry, overwrite what it reaches in the golden and require
    // the bundle id not to move. `overwrite_at` returns a count, so an entry
    // whose path reaches nothing fails here rather than passing vacuously.
    let original: serde_json::Value = serde_json::from_str(GOLDEN).expect("parse");

    for p in BUNDLE_ID_EXCLUDED_PATHS {
        assert!(
            vibe_check_model::digest::path_resolves(&original, p.path),
            "`{}` is excluded from the bundle id but absent from the golden, \
             so nothing proves it is excluded",
            p.path
        );

        let mut mutated = original.clone();
        let hits = overwrite_at(
            &mut mutated,
            p.path,
            &serde_json::json!("mutated-by-the-exclusion-test"),
        );
        assert!(
            hits > 0,
            "`{}` reached no value in the golden, so this iteration proves \
             nothing. A path ending in `[]` is the usual cause.",
            p.path
        );

        let b: EvidenceBundle = serde_json::from_value(mutated).unwrap_or_else(|e| {
            panic!(
                "overwriting `{}` produced a document that will not parse: {e}. \
                 An exclusion path must name a leaf whose value can be \
                 replaced, not a whole typed subtree.",
                p.path
            )
        });
        assert_eq!(
            bundle_id(&b).expect("digest").as_str(),
            GOLDEN_BUNDLE_ID,
            "`{}` is on the bundle id exclusion list, and changing it moved \
             the bundle id anyway",
            p.path
        );
    }
}

#[test]
fn the_exclusion_test_would_notice_a_path_that_is_not_excluded() {
    // The control. Without it, the loop above passes just as happily if
    // `bundle_id` ignores everything — a canonicalizer that returned a
    // constant would be green. `core/head_sha` is not on the exclusion list,
    // so the same mutation must move the id.
    let mut mutated: serde_json::Value = serde_json::from_str(GOLDEN).expect("parse");
    assert_eq!(
        overwrite_at(&mut mutated, "core/head_sha", &serde_json::json!("0000")),
        1
    );
    let b: EvidenceBundle = serde_json::from_value(mutated).expect("parse");
    assert_ne!(bundle_id(&b).expect("digest").as_str(), GOLDEN_BUNDLE_ID);
}
