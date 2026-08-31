//! The evidence bundle: one JSON document, three renderings.
//!
//! The bundle is the product. A pull-request comment, a static HTML artifact,
//! and `vibe-check.json` are all views of this one value, so they cannot
//! disagree about what happened.
//!
//! # The frozen core
//!
//! [`BundleCore`] never changes. Not "changes rarely" — never. The escape-rate
//! loop reads bundles going back as far as the repository does, and downstream
//! tools read the same fields; if `core` were allowed to churn, every historical
//! comparison would need a migration and most would quietly stop being
//! comparable.
//!
//! Everything outside `core` is free to evolve: additive-only within a major
//! version, migrated forward across majors. That is the trade — a small
//! immutable contract in exchange for an unconstrained one around it.
//!
//! # Self-describing on purpose
//!
//! The bundle carries each capability's *question* alongside its result. A
//! renderer that has never heard of `mutants-in-diff-killed` can still say what
//! was asked and what the answer was. Without that, every new capability would
//! require a renderer release before anyone could read its output.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::adjudicate::{AdvisoryAdjudication, EnforcedAdjudication, Enforcement, Escalation};
use crate::ids::{CapabilityId, RiskFlagId};
use crate::resolution::ResolutionState;
use crate::schema::SchemaVersion;
use crate::tier::{Tier, Verdict};

/// The part of the bundle that is guaranteed stable forever.
///
/// Adding a field here is a breaking change to every consumer and to the entire
/// historical record. If something seems to belong in `core`, it almost
/// certainly belongs in the bundle around it instead.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct BundleCore {
    /// Stable identifier for this bundle, derived from content digests rather
    /// than from a timestamp or a random source, so that regenerating the same
    /// evaluation yields the same identifier.
    pub bundle_id: String,
    /// `owner/repo`.
    pub repo: String,
    /// Pull request number, when there is one. Absent for local runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    /// The commit whose changes were classified.
    pub head_sha: String,
    /// The base branch.
    pub base_ref: String,
    /// The merge base actually used — computed, never taken from the event
    /// payload, which reports the base branch tip at an earlier moment and
    /// drifts as the base branch moves.
    pub merge_base_sha: String,
    /// The tier reached.
    pub tier: Tier,
    /// The verdict, derived from the tier.
    pub verdict: Verdict,
    /// The tier the advisory-routed outcomes reached **on their own**. Never
    /// contributes to `tier`.
    ///
    /// This is the join of the advisory lane's escalations and nothing else.
    /// The enforcing lane's escalations do not enter it, so it is *not* the
    /// counterfactual on its own:
    ///
    /// > The tier that would have been enforced had every requirement been
    /// > enforcing is `tier.join(advisory_tier)`.
    ///
    /// Read it as `advisory_tier` alone and you undercount every bundle whose
    /// enforcing lane was louder than its advisory lane — including the very
    /// common case of a run with no advisory requirements at all, where this is
    /// `t0` while `tier` is whatever was actually enforced. The two fields are
    /// the measurement together: their disagreement is what distinguishes
    /// "nothing failed" from "nothing that failed counted".
    ///
    /// Deliberately does **not** escalate when it exceeds `tier`. That would
    /// reintroduce exactly the blocking behaviour advisory exists to remove;
    /// the requirement is to report loudly, not to report and block.
    ///
    /// On the digest inclusion list — see #27 and #45, which write the first
    /// bundle and the digest that covers it.
    pub advisory_tier: Tier,
    /// Every risk flag the classifier emitted, sorted.
    pub flag_ids: Vec<RiskFlagId>,
    /// How each required capability was resolved.
    pub capability_states: BTreeMap<CapabilityId, ResolutionState>,
    /// Digest over the canonicalized verdict-bearing subtree.
    ///
    /// Excludes timestamps and durations, so the same inputs produce the same
    /// digest. This is what the replay test compares.
    pub verdict_digest: String,
}

impl BundleCore {
    /// The **only** place a `BundleCore` is constructed.
    ///
    /// The two ledgers arrive as distinct types, so `tier` and `verdict` can
    /// only come from the enforced adjudication and `advisory_tier` can only
    /// come from the advisory one; transposing them is not expressible. A
    /// workspace-wide guard test (`tests/bundle_core_construction.rs`) asserts
    /// this is the sole construction site, which is what makes that claim hold
    /// for the whole workspace rather than only for this function.
    ///
    /// The fields stay `pub` because every reader needs them and the frozen
    /// contract is about wire names, not about access.
    // `too_many_arguments` is allowed rather than satisfied, and scoped to this
    // constructor so nothing else inherits the exemption. The lint's advice —
    // group these into a struct — would mean a second type that mirrors `core`
    // field for field, and a second type that must be kept in step with a frozen
    // one is a worse hazard than a long signature. Identity fields are what they
    // are; the point of the function is the last two parameters.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        bundle_id: String,
        repo: String,
        pr: Option<u64>,
        head_sha: String,
        base_ref: String,
        merge_base_sha: String,
        flag_ids: Vec<RiskFlagId>,
        capability_states: BTreeMap<CapabilityId, ResolutionState>,
        verdict_digest: String,
        enforced: &EnforcedAdjudication,
        advisory: &AdvisoryAdjudication,
    ) -> Self {
        // Written out rather than as `Self { .. }` so the guard test can find
        // this literal by name and prove there is exactly one of them.
        BundleCore {
            bundle_id,
            repo,
            pr,
            head_sha,
            base_ref,
            merge_base_sha,
            tier: enforced.tier(),
            verdict: enforced.verdict(),
            advisory_tier: advisory.tier(),
            flag_ids,
            capability_states,
            verdict_digest,
        }
    }
}

/// What produced a bundle.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Generator {
    /// Always `vibe-check`.
    pub name: String,
    /// The binary's version.
    pub version: String,
    /// The commit it was built from, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
    /// Digest over the capability declarations and analyzer registrations that
    /// were actually in force.
    ///
    /// Without this the escape-rate loop compares verdicts produced under
    /// different rules and treats them as one population, which makes every tier
    /// proposal it emits unjustifiable.
    pub registry_digest: String,
}

/// How much of the picture we actually have.
///
/// Rendered as the confidence sentence: *"T2 on 8 capabilities, 3 advisory,
/// 2 adopted, 5 run, 1 unverified."* Counting is over **requirements**, not
/// capabilities: a capability can be adopted for one crate and run for another,
/// so there is no single state to report for it.
///
/// `#[serde(default)]` at the container level, not only on the field that holds
/// one. The bundle's `confidence` key already defaults when it is *absent*, but
/// that does nothing for a `confidence` object written before a count existed:
/// serde would reject the object for the missing field and take the whole
/// bundle down with it. Counts are additive by nature, and `bundle.rs`'s own
/// contract is additive-only within a major version.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct Confidence {
    /// Total requirements considered.
    pub requirements: usize,
    /// Answered by an existing artifact.
    pub adopted: usize,
    /// Answered by running something.
    pub ran: usize,
    /// Declared not applicable.
    pub skipped: usize,
    /// Expected and unavailable.
    pub unverified: usize,
    /// Requirements whose per-crate states disagreed.
    pub partial: usize,
    /// Requirements whose outcome could not raise the enforced tier.
    ///
    /// Counted across all four states, not only failures: "3 requirements were
    /// advisory" is the number a reader needs in order to know how much of the
    /// verdict was not being enforced. Orthogonal to the other counts, so it
    /// does not sum with them.
    pub advisory: usize,
}

impl Confidence {
    /// Tally a set of resolution states and the enforcement they carried.
    ///
    /// Takes the pair rather than the state alone: a count of advisory
    /// requirements that had to be assembled separately would be a count that
    /// can disagree with the states beside it.
    #[must_use]
    pub fn tally(states: impl IntoIterator<Item = (ResolutionState, Enforcement)>) -> Self {
        let mut c = Self::default();
        for (state, enforcement) in states {
            c.requirements += 1;
            if enforcement == Enforcement::Advisory {
                c.advisory += 1;
            }
            match state {
                ResolutionState::Adopt => c.adopted += 1,
                ResolutionState::Run => c.ran += 1,
                ResolutionState::Skip => c.skipped += 1,
                ResolutionState::Unverified => c.unverified += 1,
            }
        }
        c
    }

    /// The confidence sentence, without the leading tier.
    ///
    /// Generated rather than hand-written, so it cannot drift from the counts it
    /// describes.
    #[must_use]
    pub fn sentence(&self) -> String {
        let mut parts = Vec::new();
        // Advisory leads. Burying it after the state counts is how a comment
        // ends up truthfully saying "8 capabilities, 1 unverified" about a run
        // in which the unverified one was never going to block anything.
        if self.advisory > 0 {
            parts.push(format!("{} advisory", self.advisory));
        }
        if self.adopted > 0 {
            parts.push(format!("{} adopted", self.adopted));
        }
        if self.ran > 0 {
            parts.push(format!("{} run", self.ran));
        }
        if self.skipped > 0 {
            parts.push(format!("{} skipped", self.skipped));
        }
        if self.unverified > 0 {
            parts.push(format!("{} unverified", self.unverified));
        }
        let noun = if self.requirements == 1 {
            "capability"
        } else {
            "capabilities"
        };
        if parts.is_empty() {
            format!("{} {noun}", self.requirements)
        } else {
            format!("{} {noun} · {}", self.requirements, parts.join(" · "))
        }
    }
}

/// The complete record of one evaluation.
///
/// Read leniently: unknown fields are preserved rather than rejected, so that an
/// older build reading and rewriting a newer bundle does not destroy data. This
/// is the opposite of how policy documents are read, and the asymmetry is
/// deliberate — see [`crate::schema`].
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EvidenceBundle {
    /// Schema major version. First key in the document.
    pub schema_version: SchemaVersion,
    /// The frozen part.
    pub core: BundleCore,
    /// What produced this.
    pub generator: Generator,
    /// The verdict and every escalation that led to it.
    pub adjudication: crate::adjudicate::Adjudication,
    /// Every escalation the advisory ledger recorded and did not enforce.
    ///
    /// Outside `core` on purpose: it is a list, it is additive, and a `Vec` has
    /// a `Default` where an `Adjudication` does not — so an older bundle that
    /// predates advisory reads back as "no advisory escalations" rather than
    /// failing to parse. It is deliberately not an `Adjudication`: that type
    /// always carries a verdict derived from its tier, and a bundle must carry
    /// exactly one verdict.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advisory_escalations: Vec<Escalation>,
    /// Requirement counts.
    #[serde(default)]
    pub confidence: Confidence,
    /// Fields written by a newer build.
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extensions: serde_json::Map<String, serde_json::Value>,
}

impl EvidenceBundle {
    /// Whether this bundle can be read by a build that writes `current`.
    #[must_use]
    pub fn readable_by(&self, current: SchemaVersion) -> bool {
        self.schema_version.readable_by(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjudicate::{Adjudicators, Enforcement};
    use crate::reason::{EvidenceRef, ReasonCode};

    /// A bundle with both ledgers non-empty and at *different* tiers.
    ///
    /// `tier` is `t1` and `advisory_tier` is `t2`, so a transposition of the two
    /// is visible rather than a no-op, and `adjudication` carries a real
    /// escalation for the round-trip to exercise.
    fn bundle() -> EvidenceBundle {
        let mut adjudicators = Adjudicators::new();
        adjudicators.route(Enforcement::Enforcing).escalate(
            Tier::T1,
            ReasonCode::RuleTierAtLeast,
            "rule `core-unsafe` requires T1",
            EvidenceRef::Unattributed,
        );
        adjudicators.route(Enforcement::Advisory).escalate(
            Tier::T2,
            ReasonCode::CapabilityViolated,
            "`mutants-in-diff-killed` failed: 3 mutants survived",
            EvidenceRef::Unattributed,
        );
        let (enforced, advisory) = adjudicators.finish();

        EvidenceBundle {
            schema_version: SchemaVersion::BUNDLE,
            core: BundleCore::new(
                "vc_test".into(),
                "getkono/kono".into(),
                Some(412),
                "9f3c".into(),
                "master".into(),
                "1a77".into(),
                vec![RiskFlagId::new("unsafe")],
                BTreeMap::from([(CapabilityId::new("tests-pass"), ResolutionState::Adopt)]),
                "blake3:7c1e".into(),
                &enforced,
                &advisory,
            ),
            generator: Generator {
                name: "vibe-check".into(),
                version: "0.1.0".into(),
                git_sha: None,
                registry_digest: "blake3:0f22".into(),
            },
            adjudication: enforced.into_adjudication(),
            advisory_escalations: advisory.into_escalations(),
            confidence: Confidence::tally([
                (ResolutionState::Adopt, Enforcement::Enforcing),
                (ResolutionState::Run, Enforcement::Enforcing),
                (ResolutionState::Unverified, Enforcement::Advisory),
            ]),
            extensions: serde_json::Map::new(),
        }
    }

    #[test]
    fn round_trips_through_json() {
        let b = bundle();
        let json = serde_json::to_string(&b).expect("serialize");
        let back: EvidenceBundle = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, b);
    }

    #[test]
    fn the_core_field_names_are_the_frozen_contract() {
        // This test exists to make renaming a core field a deliberate act with a
        // failing test attached, rather than something a refactor does silently.
        // Every name here is read by the escape-rate loop and by downstream
        // tools across the whole history of a repository.
        let json = serde_json::to_value(bundle()).expect("serialize");
        let core = json.get("core").expect("core present");
        for field in [
            "bundle_id",
            "repo",
            "pr",
            "head_sha",
            "base_ref",
            "merge_base_sha",
            "tier",
            "verdict",
            "advisory_tier",
            "flag_ids",
            "capability_states",
            "verdict_digest",
        ] {
            assert!(
                core.get(field).is_some(),
                "core.{field} must never be renamed"
            );
        }
    }

    #[test]
    fn the_advisory_tier_is_written_in_the_documented_wire_form() {
        // The escape-rate loop parses this JSON, not this crate's types, so the
        // form is the contract. There is no digest in this workspace yet — see
        // #45, which writes the first bundle and must add `advisory_tier` to the
        // digest inclusion list — so the wire form is what can be proved here.
        let json = serde_json::to_value(bundle()).expect("serialize");
        let core = json.get("core").expect("core present");

        assert_eq!(core.get("tier"), Some(&serde_json::json!("t1")));
        assert_eq!(core.get("advisory_tier"), Some(&serde_json::json!("t2")));
        assert_eq!(
            core.get("verdict"),
            Some(&serde_json::json!("interface-review"))
        );
    }

    #[test]
    fn advisory_tier_is_mandatory_on_the_wire() {
        // Presence on a bundle we just serialized is not the property that
        // matters; the property is that a bundle *without* it is not readable.
        // `Tier` has no `Default` and must not gain one — a defaulted tier is
        // `T0`-shaped, which is the fail-open the lattice exists to prevent — so
        // a missing `advisory_tier` must be a parse failure and not a silent
        // `t0`. This test is what stops someone adding `#[serde(default)]` here
        // to make an unrelated fixture compile.
        let mut json = serde_json::to_value(bundle()).expect("serialize");
        json.get_mut("core")
            .and_then(serde_json::Value::as_object_mut)
            .expect("core is an object")
            .remove("advisory_tier");

        assert!(
            serde_json::from_value::<EvidenceBundle>(json).is_err(),
            "a bundle whose core omits `advisory_tier` must fail to parse, not \
             default to the most permissive tier"
        );
    }

    #[test]
    fn a_bundle_can_say_nothing_that_failed_counted() {
        // Without `advisory_tier`, `core.tier == t0` would mean either
        // "everything passed" or "everything that failed was advisory", and no
        // reader could tell which. That ambiguity would be permanent: `core` is
        // the one part of the bundle that cannot be changed later.
        let mut adjudicators = Adjudicators::new();
        adjudicators.route(Enforcement::Advisory).escalate(
            Tier::T2,
            ReasonCode::CapabilityViolated,
            "`mutants-in-diff-killed` failed: 3 mutants survived",
            EvidenceRef::Unattributed,
        );
        let (enforced, advisory) = adjudicators.finish();
        let core = BundleCore::new(
            "vc_test".into(),
            "getkono/kono".into(),
            None,
            "9f3c".into(),
            "master".into(),
            "1a77".into(),
            Vec::new(),
            BTreeMap::new(),
            "blake3:7c1e".into(),
            &enforced,
            &advisory,
        );

        assert_eq!(core.tier, Tier::T0);
        assert_eq!(core.verdict, Verdict::Auto);
        assert_eq!(core.advisory_tier, Tier::T2);
    }

    #[test]
    fn the_counterfactual_tier_is_the_join_of_the_two_fields() {
        // The reading the field documentation now spells out. `advisory_tier`
        // alone is *not* "what would have been enforced had every requirement
        // been enforcing": the advisory ledger never sees the enforcing lane, so
        // in this bundle it is `t2` while the enforcing lane independently
        // reached `t1`. A consumer reading `advisory_tier` as the counterfactual
        // undercounts every run whose enforcing lane was the louder of the two.
        let core = bundle().core;
        assert_eq!(core.tier, Tier::T1);
        assert_eq!(core.advisory_tier, Tier::T2);
        assert_eq!(core.tier.join(core.advisory_tier), Tier::T2);
        assert!(core.tier.join(core.advisory_tier) >= core.tier);
    }

    #[test]
    fn unknown_top_level_fields_survive_a_round_trip() {
        let mut json = serde_json::to_value(bundle()).expect("serialize");
        json.as_object_mut()
            .expect("object")
            .insert("perf".into(), serde_json::json!({"benchmarks": []}));
        let parsed: EvidenceBundle = serde_json::from_value(json).expect("deserialize");
        let out = serde_json::to_value(&parsed).expect("serialize");
        assert_eq!(
            out.get("perf"),
            Some(&serde_json::json!({"benchmarks": []})),
            "a section this build predates must not be dropped on rewrite"
        );
    }

    #[test]
    fn confidence_sentence_counts_requirements() {
        let c = Confidence::tally([
            (ResolutionState::Adopt, Enforcement::Enforcing),
            (ResolutionState::Adopt, Enforcement::Enforcing),
            (ResolutionState::Run, Enforcement::Enforcing),
            (ResolutionState::Unverified, Enforcement::Advisory),
        ]);
        assert_eq!(
            c.sentence(),
            "4 capabilities · 1 advisory · 2 adopted · 1 run · 1 unverified",
            "the advisory count leads, so a reader cannot miss how much of this \
             verdict was not being enforced"
        );
    }

    #[test]
    fn confidence_sentence_omits_empty_categories() {
        let c = Confidence::tally([(ResolutionState::Run, Enforcement::Enforcing)]);
        assert_eq!(c.sentence(), "1 capability · 1 run");
    }

    #[test]
    fn the_advisory_ledger_rides_outside_the_frozen_core() {
        // `advisory_escalations` is additive and lives outside `core`, so it
        // costs nothing permanent. Its absence must read as "none" rather than
        // as a parse failure, or every bundle written before this field existed
        // becomes unreadable.
        let json = serde_json::to_value(bundle()).expect("serialize");
        assert!(
            json.get("core")
                .and_then(|c| c.get("advisory_escalations"))
                .is_none()
        );
        assert_eq!(
            json.get("advisory_escalations")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len),
            Some(1)
        );

        let mut older = json.clone();
        older
            .as_object_mut()
            .expect("object")
            .remove("advisory_escalations");
        let parsed: EvidenceBundle = serde_json::from_value(older).expect("deserialize");
        assert!(parsed.advisory_escalations.is_empty());
    }

    #[test]
    fn a_confidence_object_written_before_a_count_existed_still_parses() {
        // `EvidenceBundle.confidence`'s `#[serde(default)]` only fires when the
        // key is *absent*. A `confidence` object that is present but predates a
        // count would otherwise fail on the missing field and take the entire
        // bundle down with it — which is exactly the additive-only contract
        // `bundle.rs` opens by stating.
        let mut json = serde_json::to_value(bundle()).expect("serialize");
        json.get_mut("confidence")
            .and_then(serde_json::Value::as_object_mut)
            .expect("confidence is an object")
            .remove("advisory");

        let parsed: EvidenceBundle = serde_json::from_value(json)
            .expect("a confidence object missing a count must still parse");
        assert_eq!(parsed.confidence.advisory, 0);
        assert_eq!(
            parsed.confidence.requirements, 3,
            "the counts that were present must survive"
        );
    }

    #[test]
    fn a_bundle_from_the_future_is_not_readable() {
        let mut b = bundle();
        b.schema_version = SchemaVersion(99);
        assert!(!b.readable_by(SchemaVersion::BUNDLE));
    }
}
