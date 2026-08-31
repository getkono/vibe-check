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

use crate::adjudicate::{AdvisoryAdjudication, EnforcedAdjudication};
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
    /// The tier the advisory ledger reached. Never contributes to `tier`.
    ///
    /// `tier` is what was enforced; this is what *would* have been enforced had
    /// every requirement been enforcing. Their disagreement is the measurement
    /// the escape-rate loop needs, and the only thing distinguishing "nothing
    /// failed" from "nothing that failed counted".
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
/// Rendered as the confidence sentence: *"T2 on 8 capabilities, 2 adopted,
/// 5 run, 1 unverified."* Counting is over **requirements**, not capabilities:
/// a capability can be adopted for one crate and run for another, so there is no
/// single state to report for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
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
}

impl Confidence {
    /// Tally a set of resolution states.
    #[must_use]
    pub fn tally(states: impl IntoIterator<Item = ResolutionState>) -> Self {
        let mut c = Self::default();
        for state in states {
            c.requirements += 1;
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

    /// A bundle in the state the whole feature exists to make representable:
    /// nothing enforced failed, and something advisory did.
    fn bundle() -> EvidenceBundle {
        let mut adjudicators = Adjudicators::new();
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
            confidence: Confidence::tally([
                ResolutionState::Adopt,
                ResolutionState::Run,
                ResolutionState::Unverified,
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

        assert_eq!(core.get("tier"), Some(&serde_json::json!("t0")));
        assert_eq!(core.get("advisory_tier"), Some(&serde_json::json!("t2")));
        assert_eq!(core.get("verdict"), Some(&serde_json::json!("auto")));
    }

    #[test]
    fn a_bundle_can_say_nothing_that_failed_counted() {
        // Without `advisory_tier`, `core.tier == t0` would mean either
        // "everything passed" or "everything that failed was advisory", and no
        // reader could tell which. That ambiguity would be permanent: `core` is
        // the one part of the bundle that cannot be changed later.
        let core = bundle().core;
        assert_eq!(core.tier, Tier::T0);
        assert_eq!(core.verdict, Verdict::Auto);
        assert_eq!(core.advisory_tier, Tier::T2);
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
            ResolutionState::Adopt,
            ResolutionState::Adopt,
            ResolutionState::Run,
            ResolutionState::Unverified,
        ]);
        assert_eq!(
            c.sentence(),
            "4 capabilities · 2 adopted · 1 run · 1 unverified"
        );
    }

    #[test]
    fn confidence_sentence_omits_empty_categories() {
        let c = Confidence::tally([ResolutionState::Run]);
        assert_eq!(c.sentence(), "1 capability · 1 run");
    }

    #[test]
    fn a_bundle_from_the_future_is_not_readable() {
        let mut b = bundle();
        b.schema_version = SchemaVersion(99);
        assert!(!b.readable_by(SchemaVersion::BUNDLE));
    }
}
