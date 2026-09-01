//! The evidence bundle: one JSON document, three renderings.
//!
//! The bundle is the product. The Actions step summary, `vibe-check.json`, and
//! human stdout are all views of this one value, so they cannot disagree about
//! what happened. #12 renders all three from one function; the pull-request
//! comment is a fourth backend of that same function, not a fourth rendering.
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
    /// The answering *method* each required capability ended on, one entry per
    /// capability.
    ///
    /// Keyed by a bare [`CapabilityId`] while the unit of resolution is a
    /// *requirement* — a (capability × scope) pair — so a capability required
    /// for two crates resolves twice and appears here once. The entry is those
    /// resolutions collapsed by [`ResolutionState::collapse`]: **the least
    /// confident method wins**. `tests-pass` adopted for `kono-core` and
    /// unverified for `kono-net` reads `unverified`, never `adopt`, because an
    /// entry reading `adopt` while a scope went unanswered is a fail-open
    /// written into the one part of the bundle that can never be corrected.
    ///
    /// **It says nothing about whether anything passed.** A
    /// [`ResolutionState`] is how the question was answered, not what the
    /// answer was, so `Run` covers a run that reported a violation exactly as
    /// it covers a clean one — and `Run` is the *most* confident method, the
    /// one collapse discards. A capability that ran and failed for `kono-core`
    /// while adopting a satisfied artifact for `kono-net` reads `adopt` here.
    /// That is not the worst thing that happened to it; it is how the
    /// least-confidently-answered scope was answered. Every outcome, that
    /// violation included, is in the adjudication and its escalations, which is
    /// what `tier` and `verdict` are computed from. A consumer that buckets
    /// these four values into pass and fail reads a failing run as good news.
    ///
    /// The rule is documented on the field because the field is frozen. A
    /// consumer reading bundles going back as far as the repository does cannot
    /// be asked to guess which of two disagreeing scopes an entry stood for,
    /// and there is no later release in which the key could grow a scope to
    /// disambiguate it.
    ///
    /// Collapsing is lossy, and [`Confidence::partial`] is where the loss is
    /// counted — a count nothing can feed yet, for the reason recorded on that
    /// field. Until it can, an entry here does not disclose whether the
    /// capability's other scopes agreed with it.
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
/// Rendered as the confidence sentence: *"T2 on 8 requirements, 3 advisory,
/// 2 adopted, 5 run, 1 unverified."* Counting is over **requirements**, not
/// capabilities: a capability can be adopted for one crate and run for another,
/// so there is no single state to report for it. The sentence said
/// "capabilities" until the two counts had to appear in it together.
///
/// Two fields are the deliberate exceptions, and they are the only two:
/// [`partial`](Self::partial), which counts the capabilities that disagreed
/// across their scopes — precisely the thing a requirement count cannot see —
/// and [`capabilities`](Self::capabilities), which is there to say whether that
/// grouping happened at all. Because both units sit in one struct, and one
/// sentence, [`sentence`](Self::sentence) names the unit whenever it prints a
/// capability count.
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
    /// **Capabilities** — not requirements — whose per-requirement states
    /// disagreed.
    ///
    /// The one count in this struct over a different unit, because it measures
    /// something no per-requirement count can. Scope is inside a requirement,
    /// so a requirement has exactly one state and can never be "partial"; a
    /// *capability* spans scopes, and
    /// [`BundleCore::capability_states`](crate::bundle::BundleCore::capability_states)
    /// shows only the least confident of them. This is how many entries in that
    /// map hid a disagreement — the caveat a reader needs before treating a
    /// `t0`-looking entry as the whole story.
    ///
    /// Only [`tally_by_capability`](Self::tally_by_capability) can compute it.
    /// [`tally`](Self::tally) is handed states without the capability they
    /// belong to, so it cannot group them and leaves this at zero.
    ///
    /// # Nothing can feed that constructor yet
    ///
    /// A tally is built from what [`Resolutions`](crate::resolution::Resolutions)
    /// yields, and it yields no capability. Its key is a
    /// [`RequirementId`](crate::ids::RequirementId), a non-invertible digest of
    /// a capability and its scope, and
    /// [`CapabilityResolution::capability`](crate::resolution::CapabilityResolution::capability)
    /// is `None` for precisely `Skipped` and `Unverified` — the two states this
    /// count exists to expose. So the only iterator that fits a tally today is
    /// `Resolutions::states`, which fits `tally`, and every bundle written
    /// before that changes carries a zero here.
    ///
    /// Closing it means a resolution carrying the capability it answers: a
    /// fourth `Resolutions::insert` parameter, or a `resolved()` iterator
    /// yielding `(CapabilityId, ResolutionState, Enforcement)`. That is an
    /// accounting-input change and it belongs with the bundle writer (#45),
    /// which is the first thing that will need it. Recorded here rather than
    /// left to be discovered, because a count that silently stays zero looks
    /// exactly like a repository in which nothing ever disagreed.
    ///
    /// Read a zero against [`capabilities`](Self::capabilities), which
    /// distinguishes the two.
    pub partial: usize,
    /// Requirements whose outcome could not raise the enforced tier.
    ///
    /// Counted across all four states, not only failures: "3 requirements were
    /// advisory" is the number a reader needs in order to know how much of the
    /// verdict was not being enforced. Orthogonal to the other counts, so it
    /// does not sum with them.
    pub advisory: usize,
    /// How many distinct capabilities those requirements covered.
    ///
    /// Carried so that [`partial`](Self::partial) is legible at zero. Without
    /// it, `partial: 0` means either "no capability disagreed" or "nobody
    /// grouped by capability", and `#[serde(default)]` makes an absent field
    /// the same byte as both. With it: `capabilities == 0` says the grouping
    /// never happened, and `capabilities > 0` with `partial == 0` says it
    /// happened and every capability agreed with itself.
    ///
    /// Added now rather than beside the first writer, because the ambiguity
    /// becomes permanent the moment bundles start being written — a field
    /// added later cannot say anything about the bundles that predate it.
    /// [`Confidence`] is `#[non_exhaustive]` and additive by contract, so this
    /// costs one key on the wire and no compatibility.
    ///
    /// Zero from [`tally`](Self::tally), which is given no capability to count.
    pub capabilities: usize,
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
            c.count(state, enforcement);
        }
        c
    }

    /// Tally the same way, plus [`partial`](Self::partial), from requirements
    /// that carry the capability they resolve.
    ///
    /// A second constructor rather than a wider item type on
    /// [`tally`](Self::tally). Widening `tally` would force every caller to
    /// produce a capability identity in order to get counts that do not depend
    /// on one — including
    /// [`Resolutions::states`](crate::resolution::Resolutions::states), which
    /// cannot: a [`RequirementId`](crate::ids::RequirementId) is a digest of a
    /// capability and its scope, so the capability is not recoverable from it.
    /// A caller that has the identity to hand gets the extra count; one that
    /// does not still gets every other count.
    ///
    /// The per-requirement counts come from the same private `count` as
    /// `tally`, so the two constructors cannot drift into disagreeing about
    /// what `adopted` means.
    ///
    /// The same iteration is what a caller needs in order to build
    /// [`BundleCore::capability_states`](crate::bundle::BundleCore::capability_states)
    /// with [`ResolutionState::collapse_all`]: one capability, every state its
    /// requirements reached. Nothing constructs that map yet — see #45 — so
    /// this returns the count and not the map, rather than guessing at the
    /// shape the writer will want.
    ///
    /// # This constructor has no supplier today
    ///
    /// Nothing in the workspace can produce its argument for a complete set of
    /// requirements, so a bundle built from the API as it stands must call
    /// [`tally`](Self::tally) and carries `partial: 0`. The obstruction and
    /// what would clear it are recorded on [`partial`](Self::partial). It is
    /// shipped ahead of its supplier deliberately: the rule for collapsing a
    /// capability's scopes and the count of what that collapse hides are one
    /// decision, and splitting them across two milestones is how the map ends
    /// up frozen with the caveat still unwritten.
    #[must_use]
    pub fn tally_by_capability(
        resolved: impl IntoIterator<Item = (CapabilityId, ResolutionState, Enforcement)>,
    ) -> Self {
        let mut c = Self::default();
        // `(first state seen, has any later state differed)`. A `BTreeMap` and
        // not a hash map because this crate's determinism rules apply even to a
        // count: iteration order is not observed here today, and pinning it
        // costs nothing.
        let mut per_capability: BTreeMap<CapabilityId, (ResolutionState, bool)> = BTreeMap::new();
        for (capability, state, enforcement) in resolved {
            c.count(state, enforcement);
            per_capability
                .entry(capability)
                .and_modify(|(first, disagreed)| *disagreed |= *first != state)
                .or_insert((state, false));
        }
        c.capabilities = per_capability.len();
        c.partial = per_capability
            .values()
            .filter(|(_, disagreed)| *disagreed)
            .count();
        c
    }

    /// Count one resolved requirement into every per-requirement field.
    ///
    /// The single place a state becomes a number, so `tally` and
    /// `tally_by_capability` are the same tally with one extra question asked.
    fn count(&mut self, state: ResolutionState, enforcement: Enforcement) {
        self.requirements += 1;
        if enforcement == Enforcement::Advisory {
            self.advisory += 1;
        }
        match state {
            ResolutionState::Adopt => self.adopted += 1,
            ResolutionState::Run => self.ran += 1,
            ResolutionState::Skip => self.skipped += 1,
            ResolutionState::Unverified => self.unverified += 1,
        }
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
        // Trails the state counts and names its own unit, unlike every sibling
        // part. The counts above are over requirements, this one is over
        // capabilities, and the sentence opens with a requirement count printed
        // under the word "capabilities" — so a bare "1 partial" here would be
        // divided by the wrong denominator by exactly the reader it is for.
        if self.partial > 0 {
            let unit = if self.partial == 1 {
                "capability"
            } else {
                "capabilities"
            };
            parts.push(format!("{} {unit} partial across scopes", self.partial));
        }
        // `requirement`, not `capability`. The count is over requirements —
        // the struct doc has said so since it was written — and the two differ
        // in exactly the runs that matter: `partial > 0` implies some
        // capability holds two requirements, so the clause above fires only
        // where the leading noun would be wrong. "4 capabilities · 2
        // capabilities partial across scopes" is 2 of 2 rendered as 2 of 4.
        let noun = if self.requirements == 1 {
            "requirement"
        } else {
            "requirements"
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
            "4 requirements · 1 advisory · 2 adopted · 1 run · 1 unverified",
            "the advisory count leads, so a reader cannot miss how much of this \
             verdict was not being enforced"
        );
    }

    #[test]
    fn confidence_sentence_omits_empty_categories() {
        let c = Confidence::tally([(ResolutionState::Run, Enforcement::Enforcing)]);
        assert_eq!(c.sentence(), "1 requirement · 1 run");
    }

    /// The case `capability_states`' key forces, end to end.
    ///
    /// One capability, two scopes, two disagreeing resolutions: the map entry
    /// is the unverified one and `partial` says a scope disagreed. Neither half
    /// is enough alone — the entry alone loses that `kono-core` passed, and the
    /// count alone loses which way the disagreement went.
    #[test]
    fn a_capability_adopted_in_one_scope_and_unverified_in_another() {
        let tests_pass = CapabilityId::new("tests-pass");
        let resolved = [
            (
                tests_pass.clone(),
                ResolutionState::Adopt,
                Enforcement::Enforcing,
            ),
            (
                tests_pass.clone(),
                ResolutionState::Unverified,
                Enforcement::Enforcing,
            ),
        ];

        let states: BTreeMap<CapabilityId, ResolutionState> = BTreeMap::from([(
            tests_pass.clone(),
            ResolutionState::collapse_all(resolved.iter().map(|(_, state, _)| *state))
                .expect("two requirements resolved"),
        )]);
        assert_eq!(
            states.get(&tests_pass),
            Some(&ResolutionState::Unverified),
            "the frozen map must show the scope nothing answered, not the one \
             that passed"
        );

        let c = Confidence::tally_by_capability(resolved);
        assert_eq!(c.requirements, 2, "two requirements, still counted as two");
        assert_eq!(c.partial, 1, "one capability disagreed with itself");
        assert_eq!(c.adopted, 1);
        assert_eq!(c.unverified, 1);
    }

    #[test]
    fn agreeing_scopes_are_not_partial() {
        // A capability required twice and resolved the same way both times is
        // not a disagreement, and must not be reported as one — otherwise every
        // monorepo bundle carries a permanent caveat that means nothing.
        let c = Confidence::tally_by_capability([
            (
                CapabilityId::new("tests-pass"),
                ResolutionState::Run,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("tests-pass"),
                ResolutionState::Run,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("clippy-clean"),
                ResolutionState::Adopt,
                Enforcement::Advisory,
            ),
        ]);
        assert_eq!(c.partial, 0);
        assert_eq!(c.requirements, 3);
        assert_eq!(c.advisory, 1);
    }

    #[test]
    fn the_two_tallies_agree_on_every_requirement_count() {
        // `tally_by_capability` must be `tally` plus one question, not a second
        // opinion about what `adopted` means.
        let resolved = [
            (
                CapabilityId::new("tests-pass"),
                ResolutionState::Adopt,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("tests-pass"),
                ResolutionState::Unverified,
                Enforcement::Advisory,
            ),
            (
                CapabilityId::new("clippy-clean"),
                ResolutionState::Run,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("miri-clean"),
                ResolutionState::Skip,
                Enforcement::Enforcing,
            ),
        ];
        let by_capability = Confidence::tally_by_capability(resolved.clone());
        let flat = Confidence::tally(
            resolved
                .iter()
                .map(|(_, state, enforcement)| (*state, *enforcement)),
        );

        assert_eq!(by_capability.requirements, flat.requirements);
        assert_eq!(by_capability.adopted, flat.adopted);
        assert_eq!(by_capability.ran, flat.ran);
        assert_eq!(by_capability.skipped, flat.skipped);
        assert_eq!(by_capability.unverified, flat.unverified);
        assert_eq!(by_capability.advisory, flat.advisory);
        assert_eq!(
            (by_capability.partial, flat.partial),
            (1, 0),
            "only the constructor given capability identities can measure this"
        );
    }

    #[test]
    fn the_sentence_names_the_unit_it_counts_partial_in() {
        // The sentence opens with a *requirement* count under the word
        // "capabilities" and `partial` is a *capability* count, so this part
        // carries its own noun. A bare "1 partial" would be read as one
        // requirement in four.
        let c = Confidence::tally_by_capability([
            (
                CapabilityId::new("tests-pass"),
                ResolutionState::Adopt,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("tests-pass"),
                ResolutionState::Unverified,
                Enforcement::Enforcing,
            ),
        ]);
        assert_eq!(
            c.sentence(),
            "2 requirements · 1 adopted · 1 unverified · 1 capability partial across scopes",
            "the leading count is requirements and the trailing one is \
             capabilities, and each says which it is"
        );
    }

    #[test]
    fn the_two_denominators_are_never_the_same_number_when_both_appear() {
        // `partial > 0` implies some capability holds two requirements, so the
        // partial clause fires *only* where the leading count and the trailing
        // one differ. Four requirements over two capabilities, both disagreeing:
        // the trailing 2 is 2 of 2, and printing the leading 4 under the word
        // "capabilities" made it read as 2 of 4.
        let c = Confidence::tally_by_capability([
            (
                CapabilityId::new("tests-pass"),
                ResolutionState::Adopt,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("tests-pass"),
                ResolutionState::Unverified,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("clippy-clean"),
                ResolutionState::Run,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("clippy-clean"),
                ResolutionState::Skip,
                Enforcement::Enforcing,
            ),
        ]);
        assert_eq!((c.requirements, c.capabilities, c.partial), (4, 2, 2));
        assert_eq!(
            c.sentence(),
            "4 requirements · 1 adopted · 1 run · 1 skipped · 1 unverified · \
             2 capabilities partial across scopes"
        );
    }

    #[test]
    fn the_capability_count_says_whether_partial_was_measured_at_all() {
        // `partial: 0` is ambiguous on its own — no capability disagreed, or
        // nobody grouped by capability — and `#[serde(default)]` makes an
        // absent field the same byte as both. `capabilities` separates them,
        // which matters because no supplier for `tally_by_capability` exists
        // yet, so every bundle written today takes the second branch.
        let not_measured = Confidence::tally([
            (ResolutionState::Adopt, Enforcement::Enforcing),
            (ResolutionState::Unverified, Enforcement::Enforcing),
        ]);
        assert_eq!((not_measured.capabilities, not_measured.partial), (0, 0));

        let measured_and_agreed = Confidence::tally_by_capability([
            (
                CapabilityId::new("tests-pass"),
                ResolutionState::Run,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("clippy-clean"),
                ResolutionState::Run,
                Enforcement::Enforcing,
            ),
        ]);
        assert_eq!(
            (
                measured_and_agreed.capabilities,
                measured_and_agreed.partial
            ),
            (2, 0)
        );
        assert!(
            !measured_and_agreed.sentence().contains("partial"),
            "a measured zero is not a caveat worth printing"
        );
    }

    #[test]
    fn several_partial_capabilities_are_pluralized() {
        let c = Confidence::tally_by_capability([
            (
                CapabilityId::new("clippy-clean"),
                ResolutionState::Run,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("clippy-clean"),
                ResolutionState::Skip,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("tests-pass"),
                ResolutionState::Adopt,
                Enforcement::Enforcing,
            ),
            (
                CapabilityId::new("tests-pass"),
                ResolutionState::Unverified,
                Enforcement::Enforcing,
            ),
        ]);
        assert_eq!(c.partial, 2);
        assert!(
            c.sentence()
                .ends_with("2 capabilities partial across scopes"),
            "{}",
            c.sentence()
        );
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

        // The same for the newest count, which every bundle written before it
        // existed omits.
        let mut json = serde_json::to_value(bundle()).expect("serialize");
        json.get_mut("confidence")
            .and_then(serde_json::Value::as_object_mut)
            .expect("confidence is an object")
            .remove("capabilities");
        let parsed: EvidenceBundle =
            serde_json::from_value(json).expect("a confidence object missing a count must parse");
        assert_eq!(parsed.confidence.capabilities, 0);
    }

    #[test]
    fn a_bundle_from_the_future_is_not_readable() {
        let mut b = bundle();
        b.schema_version = SchemaVersion(99);
        assert!(!b.readable_by(SchemaVersion::BUNDLE));
    }
}
