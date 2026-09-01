//! Why a verdict is what it is.
//!
//! Every escalation must name a [`ReasonCode`] and point at an [`EvidenceRef`].
//! That is not documentation-by-convention: [`crate::Adjudicator::escalate`]
//! takes both as required arguments, so raising scrutiny anonymously is not
//! something the API permits. A verdict that cannot explain itself is a verdict
//! nobody will trust, and the first thing an unexplained verdict produces is a
//! pull request to turn the tool off.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::ids::{CapabilityId, CrateId, RequirementId, RiskFlagId, RuleId};
use crate::location::Location;

/// A stable, groupable reason for an escalation.
///
/// These strings are a public interface. The escape-rate loop groups historical
/// verdicts by reason code to compute per-category rates, and downstream tools
/// branch on them, so renaming one silently invalidates history. Treat a change
/// here as a schema-major event.
///
/// Deliberately **not** `#[non_exhaustive]`. This enum is internal to our own
/// crates, and adding a variant *should* fail the build everywhere it is matched
/// until each site has been reconsidered. Fail-closed applies to us too.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasonCode {
    // --- policy said so ---------------------------------------------------
    /// A policy rule demanded at least this tier.
    RuleTierAtLeast,
    /// A changed path matched no rule at all.
    ///
    /// The fail-closed floor. There is deliberately no catch-all `paths = ["**"]`
    /// rule, because under union semantics it would pin the whole repository to
    /// the top tier forever; uncovered paths are handled by the engine instead.
    UnmatchedPath,
    /// A crate exists at head but no merge-base rule covers it.
    ///
    /// Adding a crate always requires human review the first time. This follows
    /// from evaluating the merge-base policy, which cannot have a rule scoped to
    /// a crate that did not exist yet. Correct, but surprising enough to deserve
    /// its own code and its own sentence in the comment.
    CrateUncoveredAtMergeBase,

    // --- evidence was missing or bad --------------------------------------
    /// A required capability could not be answered.
    CapabilityUnverified,
    /// A required capability was answered, and the answer was "no".
    CapabilityViolated,
    /// A capability was declared not-applicable by a policy waiver.
    ///
    /// Engine-derived skips are free; a human-authored waiver is not, because a
    /// change riding on a waiver is exactly the change that should not merge
    /// unattended.
    DeclaredSkip,
    /// A policy waiver has passed its expiry date.
    ExpiredSkip,
    /// Answering a required capability would exceed the configured cost budget.
    ///
    /// Resolves as unverified, never as a skip: running out of compute must
    /// never be silently safe.
    BudgetExceeded,

    // --- we could not classify --------------------------------------------
    /// One or more changed source files could not be parsed.
    ///
    /// Without this, a file that fails to parse yields an empty flag set and
    /// sails through — the most exploitable gap in a syntax-driven classifier.
    ClassificationDegraded,
    /// The workspace layout could not be determined, so paths cannot be
    /// attributed to crates and path-scoped policy cannot be applied.
    WorkspaceUnavailable,

    // --- anti-gaming ------------------------------------------------------
    /// The pull request modifies its own gates: policy, workflows, toolchain, or
    /// build scripts.
    GateIntegrity,
    /// Evidence was produced by a workflow this pull request modifies, and is
    /// therefore attacker-controlled.
    GatesModified,
    /// An adopted artifact could not be proven to come from this head commit.
    AdoptionStale,

    // --- the document and the binary disagree ------------------------------
    /// Policy references a risk flag this build cannot evaluate.
    UnknownRiskFlag,
    /// Policy references a capability this build does not implement.
    UnknownCapability,
    /// Policy references a parser this build does not implement.
    UnknownParser,
    /// Policy contains a key this build does not recognize.
    ///
    /// Unknown keys in policy are a hard error, unlike unknown keys in bundles,
    /// which are preserved. Policy is adversarial input: a silently ignored
    /// misspelling of a security-relevant key is a way to weaken a gate without
    /// it showing up in review.
    UnknownPolicyKey,
    /// The policy document declares a newer schema than this build understands.
    PolicyTooNew,
    /// The policy document requires a newer binary than this one.
    BinaryTooOld,

    // --- we could not run at all -------------------------------------------
    /// The merge base could not be resolved.
    ///
    /// Never falls back to head policy, which would let a pull request be judged
    /// by rules it wrote.
    MergeBaseUnavailable,
    /// vibe-check panicked. Emitted with a minimal bundle so that a crash is
    /// still a verdict, and specifically is still `human`.
    InternalPanic,
}

impl ReasonCode {
    /// The stable wire string for this code.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RuleTierAtLeast => "rule-tier-at-least",
            Self::UnmatchedPath => "unmatched-path",
            Self::CrateUncoveredAtMergeBase => "crate-uncovered-at-merge-base",
            Self::CapabilityUnverified => "capability-unverified",
            Self::CapabilityViolated => "capability-violated",
            Self::DeclaredSkip => "declared-skip",
            Self::ExpiredSkip => "expired-skip",
            Self::BudgetExceeded => "budget-exceeded",
            Self::ClassificationDegraded => "classification-degraded",
            Self::WorkspaceUnavailable => "workspace-unavailable",
            Self::GateIntegrity => "gate-integrity",
            Self::GatesModified => "gates-modified",
            Self::AdoptionStale => "adoption-stale",
            Self::UnknownRiskFlag => "unknown-risk-flag",
            Self::UnknownCapability => "unknown-capability",
            Self::UnknownParser => "unknown-parser",
            Self::UnknownPolicyKey => "unknown-policy-key",
            Self::PolicyTooNew => "policy-too-new",
            Self::BinaryTooOld => "binary-too-old",
            Self::MergeBaseUnavailable => "merge-base-unavailable",
            Self::InternalPanic => "internal-panic",
        }
    }
}

impl fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A pointer into the policy document.
///
/// Rendered as `path#kind:id@blob:hash` so a verdict can be traced to the exact
/// rule that produced it, in the exact revision of the file that was in force —
/// which is not necessarily the revision in the pull request.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct PolicyRef {
    /// Path to the policy document, repository-relative.
    pub path: camino::Utf8PathBuf,
    /// The kind of entry, e.g. `rule`, `skip`, `exempt`, `adopt`, `capability`.
    pub kind: String,
    /// The entry's mandatory unique identifier.
    pub id: String,
    /// Git blob hash of the policy document that was actually evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_sha: Option<String>,
}

impl fmt::Display for PolicyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}:{}", self.path, self.kind, self.id)?;
        if let Some(sha) = &self.blob_sha {
            write!(f, "@blob:{sha}")?;
        }
        Ok(())
    }
}

/// What an escalation points at.
///
/// `#[non_exhaustive]` because this is rendered into bundles that outlive the
/// build that wrote them, and a renderer meeting a variant it does not know must
/// degrade to showing the reason code rather than failing to display a verdict.
///
/// # Wire format
///
/// Adjacently tagged: `{"kind": "<variant>", "ref": <payload>}`, with `ref`
/// absent for [`EvidenceRef::Unattributed`], which has no payload. The `kind`
/// key is what the degrade-to-the-reason-code contract above reads, so it stays
/// at the top level and stays spelled the same as on the five sibling enums.
///
/// That placement is necessary for the contract and not sufficient for it: a
/// document naming a variant this build has never heard of still fails to
/// deserialize here, exactly as it did under internal tagging — serde answers
/// `unknown variant`, and a renderer never gets as far as reading the tag it
/// would degrade on. Making the enum unknown-tolerant is #29 and is not done
/// here. What this encoding fixes is that the *known* variants can be written
/// at all.
///
/// Adjacent rather than internal tagging, and not by taste. Internal tagging
/// requires every payload to be a map, and five variants here are newtypes over
/// [`crate::ids`] string identifiers, which serde refuses to tag internally —
/// they failed at run time, not at compile time. Internal tagging also put
/// `PolicyRef`'s own `kind` field in the same object as the tag, where it
/// overwrote it. Nesting the payload under `ref` fixes both at once.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "ref", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum EvidenceRef {
    /// An entry in the policy document.
    Policy(PolicyRef),
    /// A requirement that was resolved, or failed to resolve.
    Requirement(RequirementId),
    /// A capability, when no specific requirement applies.
    Capability(CapabilityId),
    /// A risk flag, with where it was found.
    Flag {
        /// The flag.
        flag: RiskFlagId,
        /// Where it was found. May be empty if the analyzer could not localize it.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        locations: Vec<Location>,
    },
    /// A crate or pseudo-crate.
    Crate(CrateId),
    /// A rule in the policy matrix.
    Rule(RuleId),
    /// A path in the repository.
    Path(camino::Utf8PathBuf),
    /// Nothing more specific — the reason code carries the whole story.
    ///
    /// Kept explicit rather than using `Option<EvidenceRef>` so that "there is
    /// genuinely nothing to point at" is a decision someone made, not a field
    /// somebody forgot to fill in.
    Unattributed,
}

impl fmt::Display for EvidenceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy(p) => write!(f, "{p}"),
            Self::Requirement(r) => write!(f, "requirement:{r}"),
            Self::Capability(c) => write!(f, "capability:{c}"),
            Self::Flag { flag, locations } => {
                write!(f, "flag:{flag}")?;
                if let Some(first) = locations.first() {
                    write!(f, " @ {first}")?;
                    let extra = locations.len() - 1;
                    if extra > 0 {
                        write!(f, " (+{extra} more)")?;
                    }
                }
                Ok(())
            }
            Self::Crate(c) => write!(f, "crate:{c}"),
            Self::Rule(r) => write!(f, "rule:{r}"),
            Self::Path(p) => write!(f, "path:{p}"),
            Self::Unattributed => f.write_str("-"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::location::LineRange;

    #[test]
    fn reason_codes_match_their_wire_form() {
        // `as_str` and the serde representation must agree: the escape-rate loop
        // groups by the serialized string, and the renderer prints `as_str`.
        // If they drift, historical grouping silently splits in two.
        for code in [
            ReasonCode::RuleTierAtLeast,
            ReasonCode::GateIntegrity,
            ReasonCode::CapabilityUnverified,
            ReasonCode::CrateUncoveredAtMergeBase,
            ReasonCode::InternalPanic,
        ] {
            let json = serde_json::to_string(&code).expect("serialize");
            assert_eq!(json, format!(r#""{}""#, code.as_str()));
        }
    }

    #[test]
    fn policy_ref_renders_traceably() {
        let r = PolicyRef {
            path: ".vibe-check/policy.toml".into(),
            kind: "rule".into(),
            id: "core-unsafe".into(),
            blob_sha: Some("e41d".into()),
        };
        assert_eq!(
            r.to_string(),
            ".vibe-check/policy.toml#rule:core-unsafe@blob:e41d"
        );
    }

    /// The wire `kind` every variant promises to serialize under.
    ///
    /// It is exhaustive over a `#[non_exhaustive]` enum, which only code inside
    /// this crate is allowed to be — an integration test in `tests/` is an
    /// external crate and would be forced to write a wildcard arm, which guards
    /// nothing. So it lives here.
    ///
    /// Its contribution is narrower than "adding a variant breaks the build":
    /// the `Display` impl above is already an exhaustive `match` over this same
    /// type, and it broke the build for a new variant long before this existed
    /// — both of the defects fixed in this module shipped past it. What is new
    /// is *what the author must write to get compiling again*. `Display` asks
    /// for a human rendering; this asks for the variant's wire name, which is
    /// the thing the tests below can then check against the bytes serde
    /// actually emits.
    ///
    /// What it cannot do is force the author to also add a sample to
    /// `every_variant` — that was measured, not assumed: an arm added without a
    /// sample leaves every test below green. The compile error is the prompt,
    /// and it lands the author in this module, next to the list they need to
    /// extend; nothing here makes it mandatory. Doing so needs a variant count
    /// the language will not give us on stable without a derive, and this crate
    /// does not take one for a test.
    fn wire_kind(evidence: &EvidenceRef) -> &'static str {
        match evidence {
            EvidenceRef::Policy(_) => "policy",
            EvidenceRef::Requirement(_) => "requirement",
            EvidenceRef::Capability(_) => "capability",
            EvidenceRef::Flag { .. } => "flag",
            EvidenceRef::Crate(_) => "crate",
            EvidenceRef::Rule(_) => "rule",
            EvidenceRef::Path(_) => "path",
            EvidenceRef::Unattributed => "unattributed",
        }
    }

    /// Every `kind` the wire format promises, in no particular order.
    ///
    /// Written out rather than derived, because being derived from the enum is
    /// exactly what would stop it noticing. It is a second, independent copy of
    /// the variant names, so renaming a variant reddens
    /// `every_variant_is_sampled` even after `wire_kind` has been updated to
    /// agree with the rename — the rename has to be typed out twice, in two
    /// places, which is the point.
    ///
    /// It says nothing about serde. Both sides of that comparison are string
    /// literals in this module, so a change to the *encoding* of a name — the
    /// `rename_all` rule, say — is invisible here; that is
    /// `every_variant_is_tagged_with_its_kind`'s job, and it is checked there
    /// against real bytes.
    const EVERY_WIRE_KIND: &[&str] = &[
        "policy",
        "requirement",
        "capability",
        "flag",
        "crate",
        "rule",
        "path",
        "unattributed",
    ];

    /// One sample of each variant, with a non-trivial payload where there is
    /// one, so a round trip has something to lose.
    fn every_variant() -> Vec<EvidenceRef> {
        vec![
            EvidenceRef::Policy(PolicyRef {
                path: ".vibe-check/policy.toml".into(),
                kind: "rule".into(),
                id: "core-unsafe".into(),
                blob_sha: Some("e41d".into()),
            }),
            EvidenceRef::Requirement(
                RequirementId::from_wire("req_tests-pass_0000000000000000")
                    .expect("a well-formed fixture identifier"),
            ),
            EvidenceRef::Capability(CapabilityId::new("loom-clean")),
            EvidenceRef::Flag {
                flag: RiskFlagId::new("unsafe"),
                locations: vec![Location::file("a.rs").at_lines(LineRange::single(88))],
            },
            EvidenceRef::Crate(CrateId::new("vibe-check-model")),
            EvidenceRef::Rule(RuleId::new("core-unsafe")),
            EvidenceRef::Path("crates/vibe-check-model/src/reason.rs".into()),
            EvidenceRef::Unattributed,
        ]
    }

    #[test]
    fn every_variant_is_sampled() {
        // Two things at once: that `every_variant` covers each name in
        // `EVERY_WIRE_KIND` exactly once, so the round trips below are not
        // silently testing seven of eight; and that no existing variant has
        // been renamed, since `EVERY_WIRE_KIND` is a hand-written copy of those
        // names that a rename does not carry along with it.
        //
        // "Every variant" here means every variant the const lists, not every
        // variant the enum has — the const is hand-maintained, and as
        // `wire_kind` says, a new variant that reaches neither list is caught
        // by nothing at run time. The compile error at `wire_kind` is what is
        // relied on for that case.
        let mut sampled: Vec<&str> = every_variant().iter().map(wire_kind).collect();
        sampled.sort_unstable();
        let mut promised = EVERY_WIRE_KIND.to_vec();
        promised.sort_unstable();
        assert_eq!(
            sampled, promised,
            "every wire `kind` needs exactly one sample in `every_variant`"
        );
    }

    #[test]
    fn every_variant_round_trips() {
        // The property that was false for five of eight variants: an
        // internally tagged enum cannot serialize a newtype variant over a
        // string, and it failed at run time rather than at compile time.
        for evidence in every_variant() {
            let json = serde_json::to_string(&evidence)
                .unwrap_or_else(|e| panic!("{evidence:?} does not serialize: {e}"));
            let back: EvidenceRef = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{evidence:?} does not deserialize from {json}: {e}"));
            assert_eq!(back, evidence, "round trip changed the value: {json}");
        }
    }

    #[test]
    fn every_variant_is_tagged_with_its_kind() {
        // `#[non_exhaustive]` promises a renderer meeting an unknown variant
        // can degrade to the reason code. That promise is only keepable if
        // `kind` is readable at the top level without knowing the variant, so
        // the tag's placement is part of the contract, not an encoding detail.
        // (Keepable, not kept: see the type's wire-format note — an unknown
        // variant does not deserialize at all today. This pins the half of it
        // that the encoding does deliver.)
        //
        // This is also the only test here that compares `wire_kind` against
        // bytes serde actually produced, which makes it the one that notices a
        // change to how names are encoded rather than to the names themselves.
        // Note that it would not notice one *today*: every variant is a single
        // word, so `kebab-case`, `snake_case` and `lowercase` all agree on all
        // eight. Verified by switching the attribute — the suite stays green.
        // The first multi-word variant is what arms it, and this is where that
        // failure will surface.
        for evidence in every_variant() {
            let value = serde_json::to_value(&evidence).expect("serialize");
            let object = value.as_object().expect("every variant is an object");
            assert_eq!(
                object.get("kind").and_then(serde_json::Value::as_str),
                Some(wire_kind(&evidence)),
                "wrong or missing tag for {evidence:?}"
            );
            // Adjacent tagging means exactly two keys, or one when there is no
            // payload. `PolicyRef` has a `kind` field of its own, and under the
            // internal tagging this replaced it overwrote the tag with it.
            let expected_keys = usize::from(!matches!(evidence, EvidenceRef::Unattributed)) + 1;
            assert_eq!(
                object.len(),
                expected_keys,
                "payload leaked into the tag object for {evidence:?}: {value}"
            );
        }
    }

    #[test]
    fn a_policy_ref_keeps_its_own_kind_field() {
        // The collision, stated directly. `PolicyRef::kind` and the tag key are
        // both `kind`; internally tagged they landed in one object, where
        // `serde_json::to_value` silently overwrote the tag `"policy"` with the
        // field's `"rule"` — a different, real variant — and
        // `serde_json::to_string` emitted a duplicate key that would not
        // deserialize. Adjacent tagging puts the payload one level down.
        let policy = PolicyRef {
            path: ".vibe-check/policy.toml".into(),
            kind: "rule".into(),
            id: "core-unsafe".into(),
            blob_sha: Some("e41d".into()),
        };
        let evidence = EvidenceRef::Policy(policy.clone());
        let value = serde_json::to_value(&evidence).expect("serialize");

        assert_eq!(value["kind"], "policy", "the tag survives the payload");
        assert_eq!(value["ref"]["kind"], "rule", "and the payload survives too");

        let json = serde_json::to_string(&evidence).expect("serialize");
        let back: EvidenceRef = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, EvidenceRef::Policy(policy));
    }

    #[test]
    fn flag_ref_summarizes_extra_locations() {
        let r = EvidenceRef::Flag {
            flag: RiskFlagId::new("unsafe"),
            locations: vec![
                Location::file("a.rs").at_lines(LineRange::single(88)),
                Location::file("b.rs").at_lines(LineRange::single(12)),
            ],
        };
        assert_eq!(r.to_string(), "flag:unsafe @ a.rs:88 (+1 more)");
    }
}
