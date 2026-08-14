//! How a required capability turned out.
//!
//! # Two axes, not one
//!
//! [`CapabilityResolution`] says **how** the question was answered — adopted,
//! run, skipped, or unverified. [`Judgement`] says **what** the answer was.
//! Collapsing them into one enum looks tidier and immediately breaks: "adopted,
//! and it failed" and "adopted, but the test binary would not compile" have
//! nowhere to live, and the second one is exactly the case the test-negation
//! probe is built on.
//!
//! # Fail-closed lives here
//!
//! [`CapabilityResolution::account`] is the single consumer of a resolution, and
//! it is where the two rules that make the whole system safe are applied without
//! exception:
//!
//! - an unverified capability escalates to [`Tier::TOP`]
//! - a human-authored waiver escalates to [`Tier::T1`]
//!
//! Because there is one consumer, there is one place to audit.

use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::adjudicate::Adjudicator;
use crate::evidence::Evidence;
use crate::ids::{CapabilityId, ParserId, RequirementId};
use crate::reason::{EvidenceRef, PolicyRef, ReasonCode};
use crate::tier::Tier;

/// What the evidence says.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Judgement {
    /// The capability's question is answered "yes".
    Satisfied,
    /// The question is answered "no".
    Violated {
        /// What went wrong, in a sentence.
        detail: String,
    },
    /// The evidence does not answer the question either way.
    ///
    /// A benchmark that did not converge, a coverage run with no data, a
    /// negation test whose added tests would not compile against the base
    /// commit. Treated as unverified by [`CapabilityResolution::account`], so an
    /// inconclusive result never reads as a pass.
    Inconclusive {
        /// Why no conclusion could be drawn.
        reason: String,
    },
}

impl Judgement {
    /// Whether this judgement can support the capability being satisfied.
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        matches!(self, Self::Satisfied)
    }
}

/// Why a capability was not evaluated, in a way that is acceptable.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum SkipReason {
    /// The engine determined the capability does not apply.
    ///
    /// No `unsafe` in the changed hunks, so Miri has nothing to check. Free: no
    /// escalation, because nobody made a judgement call — the change simply does
    /// not raise the question.
    Derived {
        /// What was observed, for the record.
        detail: String,
    },
    /// A policy waiver declares the capability not applicable.
    ///
    /// Costs [`Tier::T1`]. A human wrote this down, and a change that relies on
    /// a human's waiver is precisely the change that should not merge
    /// unattended. Long-lived waivers becoming permanently mildly annoying is
    /// the intended behaviour; the escape-rate loop is how they get retired.
    Declared {
        /// The policy entry that granted it.
        policy_ref: PolicyRef,
        /// Why it was granted.
        reason: String,
        /// Who owns it.
        owner: String,
        /// When it lapses.
        ///
        /// Compared against the head commit's committer date, never the wall
        /// clock, so that re-running an old pull request gives the same verdict.
        expires: Date,
    },
}

impl SkipReason {
    /// Whether this skip was a human decision rather than an engine deduction.
    #[must_use]
    pub fn is_declared(&self) -> bool {
        matches!(self, Self::Declared { .. })
    }
}

/// Why a capability could not be answered.
///
/// Every variant escalates. This type is the destination for every failure in
/// the resolution pipeline: downstream crates provide `From` conversions into
/// it from their own error types, and — crucially — into *nothing else*. A parse
/// failure has exactly one thing it can become.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum UnverifiedReason {
    /// An artifact was found but could not be parsed.
    Unparseable {
        /// The parser that was tried.
        parser: ParserId,
        /// What went wrong.
        detail: String,
    },
    /// A check reported success but produced no machine-readable artifact.
    ///
    /// This is the case the whole adoption design exists to refuse. A green
    /// check named `tests` may have run a subset, excluded a feature, or skipped
    /// a target; its name is not evidence.
    NoArtifact {
        /// What was found instead.
        detail: String,
    },
    /// Evidence that should have been produced never arrived.
    MissingEvidence,
    /// An artifact could not be tied to this head commit.
    StaleArtifact {
        /// The commit the artifact claims.
        produced_from: String,
        /// The commit we needed.
        expected: String,
    },
    /// Evidence came from a workflow this pull request modifies.
    ///
    /// Attacker-controlled by construction: a change that edits the workflow
    /// producing its own evidence can make that evidence say anything.
    GatesModified {
        /// Which gate paths the change touches.
        paths: Vec<String>,
    },
    /// Answering would exceed the configured cost budget.
    ///
    /// Unverified rather than skipped: running out of compute must never be
    /// silently safe.
    BudgetExceeded {
        /// The capability's declared cost class.
        cost: String,
        /// The configured ceiling.
        max: String,
    },
    /// The evidence did not answer the question.
    Inconclusive {
        /// Why not.
        reason: String,
    },
    /// The tool could not be run.
    ExecutionFailed {
        /// What went wrong.
        detail: String,
    },
    /// No plan could be produced for this capability.
    PlanFailed {
        /// What went wrong.
        detail: String,
    },
    /// Policy names a capability this build does not implement.
    UnknownCapability {
        /// The name policy used.
        id: String,
    },
    /// Policy names a parser this build does not implement.
    UnknownParser {
        /// The name policy used.
        id: String,
    },
    /// Adoption needs a forge and there is none, e.g. running locally.
    NoForge,
    /// The artifact declares a schema newer than this build supports.
    SchemaTooNew {
        /// What it declared.
        found: u32,
        /// What we support.
        supported: u32,
    },
    /// The artifact exceeded a size limit.
    Oversized {
        /// What the limit was and what was seen.
        detail: String,
    },
}

impl UnverifiedReason {
    /// The reason code this escalates with.
    #[must_use]
    pub fn reason_code(&self) -> ReasonCode {
        match self {
            Self::UnknownCapability { .. } => ReasonCode::UnknownCapability,
            Self::UnknownParser { .. } => ReasonCode::UnknownParser,
            Self::BudgetExceeded { .. } => ReasonCode::BudgetExceeded,
            Self::GatesModified { .. } => ReasonCode::GatesModified,
            Self::StaleArtifact { .. } => ReasonCode::AdoptionStale,
            _ => ReasonCode::CapabilityUnverified,
        }
    }

    /// A sentence a maintainer can act on.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::Unparseable { parser, detail } => {
                format!("artifact could not be parsed by `{parser}`: {detail}")
            }
            Self::NoArtifact { detail } => format!(
                "no machine-readable artifact: {detail}. \
                 A passing check is not evidence; it must upload a parseable result."
            ),
            Self::MissingEvidence => {
                "expected evidence was never produced; the job may have been skipped or failed"
                    .into()
            }
            Self::StaleArtifact {
                produced_from,
                expected,
            } => format!(
                "artifact was produced from {produced_from} but the head is {expected}; \
                 re-run the producing workflow for this commit"
            ),
            Self::GatesModified { paths } => format!(
                "evidence comes from a workflow this change modifies ({}); \
                 adopted results cannot be trusted here",
                paths.join(", ")
            ),
            Self::BudgetExceeded { cost, max } => {
                format!("cost class `{cost}` exceeds the configured maximum `{max}`")
            }
            Self::Inconclusive { reason } => format!("evidence was inconclusive: {reason}"),
            Self::ExecutionFailed { detail } => format!("could not run the tool: {detail}"),
            Self::PlanFailed { detail } => format!("could not plan the capability: {detail}"),
            Self::UnknownCapability { id } => format!(
                "policy requires capability `{id}`, which this build does not implement; \
                 upgrade vibe-check or declare it in the policy"
            ),
            Self::UnknownParser { id } => {
                format!("policy names parser `{id}`, which this build does not implement")
            }
            Self::NoForge => {
                "adoption needs access to the forge; running locally without a token".into()
            }
            Self::SchemaTooNew { found, supported } => format!(
                "evidence declares schema v{found} but this build supports up to v{supported}; \
                 upgrade vibe-check"
            ),
            Self::Oversized { detail } => format!("artifact exceeded a size limit: {detail}"),
        }
    }
}

/// Which of the four states a capability landed in.
///
/// Split out from [`CapabilityResolution`] so counts and the bundle's state
/// table can be built without cloning the evidence.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResolutionState {
    /// An existing artifact answered it.
    Adopt,
    /// vibe-check ran something to answer it.
    Run,
    /// Declared not applicable.
    Skip,
    /// Expected, and unavailable.
    Unverified,
}

impl ResolutionState {
    /// Order by how much is actually known, least first.
    ///
    /// Used to aggregate a requirement that spans several crates:
    /// least-confident-wins. Note that `Adopt` sorting below `Run` affects only
    /// how the state is *labelled* — escalation is driven by the state itself,
    /// so this debatable ordering never changes a verdict.
    #[must_use]
    pub fn confidence_rank(self) -> u8 {
        match self {
            Self::Unverified => 0,
            Self::Skip => 1,
            Self::Adopt => 2,
            Self::Run => 3,
        }
    }
}

/// How a required capability was resolved.
///
/// Four states, closed deliberately: this is the specification's own hard rule,
/// and everything downstream branches on it exhaustively.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum CapabilityResolution {
    /// An existing artifact answered the question.
    Adopted {
        /// The normalized evidence.
        evidence: Box<Evidence>,
        /// What it says.
        judgement: Judgement,
    },
    /// vibe-check ran a tool to answer the question.
    Ran {
        /// The normalized evidence.
        evidence: Box<Evidence>,
        /// What it says.
        judgement: Judgement,
    },
    /// The question does not apply.
    Skipped {
        /// Why not.
        reason: SkipReason,
    },
    /// The question could not be answered.
    Unverified {
        /// Why not.
        reason: UnverifiedReason,
    },
}

impl CapabilityResolution {
    /// Which of the four states this is.
    #[must_use]
    pub fn state(&self) -> ResolutionState {
        match self {
            Self::Adopted { .. } => ResolutionState::Adopt,
            Self::Ran { .. } => ResolutionState::Run,
            Self::Skipped { .. } => ResolutionState::Skip,
            Self::Unverified { .. } => ResolutionState::Unverified,
        }
    }

    /// Apply this resolution to the verdict.
    ///
    /// **The only consumer of a resolution**, and therefore the only place the
    /// fail-closed rules need to be correct:
    ///
    /// | resolution | effect |
    /// |---|---|
    /// | satisfied, from measured evidence | nothing |
    /// | violated | escalate to [`Tier::TOP`] |
    /// | inconclusive | escalate to [`Tier::TOP`] — an inconclusive result is not a pass |
    /// | satisfied, but only *declared* | escalate to [`Tier::TOP`] — an assertion is not a measurement |
    /// | engine-derived skip | nothing |
    /// | policy-declared waiver | escalate to [`Tier::T1`] |
    /// | unverified | escalate to [`Tier::TOP`] |
    ///
    /// Note there is no path through this function in which an unanswered
    /// question leaves the tier alone.
    pub fn account(&self, requirement: &RequirementId, adjudicator: &mut Adjudicator) {
        let evidence_ref = EvidenceRef::Requirement(requirement.clone());
        match self {
            Self::Adopted {
                evidence,
                judgement,
            }
            | Self::Ran {
                evidence,
                judgement,
            } => {
                // A declaration masquerading as measured evidence would be the
                // cheapest possible way to fake a pass. It cannot satisfy.
                if !evidence.provenance().is_measured() {
                    adjudicator.escalate(
                        Tier::TOP,
                        ReasonCode::CapabilityUnverified,
                        format!(
                            "`{}` is backed only by a declaration, not a measurement",
                            evidence.capability()
                        ),
                        evidence_ref,
                    );
                    return;
                }
                match judgement {
                    Judgement::Satisfied => {}
                    Judgement::Violated { detail } => adjudicator.escalate(
                        Tier::TOP,
                        ReasonCode::CapabilityViolated,
                        format!("`{}` failed: {detail}", evidence.capability()),
                        evidence_ref,
                    ),
                    Judgement::Inconclusive { reason } => adjudicator.escalate(
                        Tier::TOP,
                        ReasonCode::CapabilityUnverified,
                        format!("`{}` was inconclusive: {reason}", evidence.capability()),
                        evidence_ref,
                    ),
                }
            }
            Self::Skipped { reason } => match reason {
                SkipReason::Derived { .. } => {}
                SkipReason::Declared {
                    policy_ref,
                    reason: why,
                    owner,
                    expires,
                } => adjudicator.escalate(
                    Tier::T1,
                    ReasonCode::DeclaredSkip,
                    format!("waived by {policy_ref} ({why}); owner {owner}, expires {expires}"),
                    evidence_ref,
                ),
            },
            Self::Unverified { reason } => adjudicator.escalate(
                Tier::TOP,
                reason.reason_code(),
                reason.detail(),
                evidence_ref,
            ),
        }
    }

    /// The capability this answers, when evidence is present.
    #[must_use]
    pub fn capability(&self) -> Option<&CapabilityId> {
        match self {
            Self::Adopted { evidence, .. } | Self::Ran { evidence, .. } => {
                Some(evidence.capability())
            }
            Self::Skipped { .. } | Self::Unverified { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::{EvidenceFacts, ParsedEvidence, Provenance};
    use crate::tier::Verdict;
    use jiff::Timestamp;

    fn requirement() -> RequirementId {
        RequirementId::new("req_tests-pass_all")
    }

    fn evidence_with(provenance: Provenance) -> Box<Evidence> {
        Box::new(Evidence::from_parsed(
            ParsedEvidence::new(
                CapabilityId::new("tests-pass"),
                ParserId::new("junit@1"),
                EvidenceFacts::default(),
            ),
            provenance,
        ))
    }

    fn measured() -> Box<Evidence> {
        evidence_with(Provenance::Executed {
            plan_digest: "blake3:abcd".into(),
            exit_code: 0,
            started_at: Timestamp::UNIX_EPOCH,
            duration_ms: 1,
            toolchain: "1.97.1".into(),
        })
    }

    fn verdict_of(resolution: &CapabilityResolution) -> Verdict {
        let mut adj = Adjudicator::new();
        resolution.account(&requirement(), &mut adj);
        adj.finish().verdict
    }

    #[test]
    fn satisfied_measured_evidence_leaves_the_verdict_alone() {
        assert_eq!(
            verdict_of(&CapabilityResolution::Ran {
                evidence: measured(),
                judgement: Judgement::Satisfied,
            }),
            Verdict::Auto
        );
    }

    #[test]
    fn every_unverified_reason_forces_human_review() {
        // Exhaustive on purpose: a new variant that forgot to escalate would be
        // a silent pass, which is the failure mode this whole design is about.
        let reasons = [
            UnverifiedReason::Unparseable {
                parser: ParserId::new("junit@1"),
                detail: "not xml".into(),
            },
            UnverifiedReason::NoArtifact {
                detail: "check `tests` succeeded but uploaded nothing".into(),
            },
            UnverifiedReason::MissingEvidence,
            UnverifiedReason::StaleArtifact {
                produced_from: "aaaa".into(),
                expected: "bbbb".into(),
            },
            UnverifiedReason::GatesModified {
                paths: vec![".github/workflows/ci.yml".into()],
            },
            UnverifiedReason::BudgetExceeded {
                cost: "extreme".into(),
                max: "high".into(),
            },
            UnverifiedReason::Inconclusive {
                reason: "no data".into(),
            },
            UnverifiedReason::ExecutionFailed {
                detail: "miri not installed".into(),
            },
            UnverifiedReason::PlanFailed {
                detail: "no config".into(),
            },
            UnverifiedReason::UnknownCapability {
                id: "loom-clean".into(),
            },
            UnverifiedReason::UnknownParser {
                id: "loom-json@1".into(),
            },
            UnverifiedReason::NoForge,
            UnverifiedReason::SchemaTooNew {
                found: 9,
                supported: 1,
            },
            UnverifiedReason::Oversized {
                detail: "72 MiB > 64 MiB".into(),
            },
        ];
        for reason in reasons {
            let detail = reason.detail();
            assert_eq!(
                verdict_of(&CapabilityResolution::Unverified {
                    reason: reason.clone()
                }),
                Verdict::Human,
                "{reason:?} must escalate"
            );
            assert!(!detail.is_empty(), "{reason:?} must explain itself");
        }
    }

    #[test]
    fn a_green_check_with_no_artifact_says_so_in_the_message() {
        // The message is the product here: a maintainer seeing this needs to
        // understand that their passing job was not enough and why.
        let reason = UnverifiedReason::NoArtifact {
            detail: "check `tests` succeeded but uploaded nothing".into(),
        };
        let detail = reason.detail();
        assert!(detail.contains("not evidence"), "{detail}");
        assert!(detail.contains("parseable"), "{detail}");
    }

    #[test]
    fn an_inconclusive_answer_is_not_a_pass() {
        assert_eq!(
            verdict_of(&CapabilityResolution::Ran {
                evidence: measured(),
                judgement: Judgement::Inconclusive {
                    reason: "added tests do not compile against base".into()
                },
            }),
            Verdict::Human
        );
    }

    #[test]
    fn a_declaration_cannot_satisfy_a_capability() {
        // Even claiming Satisfied: the provenance is checked before the
        // judgement, so writing "this is fine" in policy is not a way through.
        assert_eq!(
            verdict_of(&CapabilityResolution::Adopted {
                evidence: evidence_with(Provenance::Declared {
                    by: "policy#skip:x".into(),
                    reason: "trust me".into(),
                }),
                judgement: Judgement::Satisfied,
            }),
            Verdict::Human
        );
    }

    #[test]
    fn derived_skips_are_free_and_declared_waivers_are_not() {
        let derived = CapabilityResolution::Skipped {
            reason: SkipReason::Derived {
                detail: "no unsafe in changed hunks".into(),
            },
        };
        assert_eq!(verdict_of(&derived), Verdict::Auto);

        let declared = CapabilityResolution::Skipped {
            reason: SkipReason::Declared {
                policy_ref: PolicyRef {
                    path: ".vibe-check/policy.toml".into(),
                    kind: "skip".into(),
                    id: "macros-no-miri".into(),
                    blob_sha: None,
                },
                reason: "proc-macro crate forbids unsafe".into(),
                owner: "@kono/platform".into(),
                expires: Date::constant(2027, 1, 1),
            },
        };
        // A human waiver is reviewable, not free.
        assert_eq!(verdict_of(&declared), Verdict::InterfaceReview);
    }

    #[test]
    fn least_confident_wins_when_aggregating_across_crates() {
        let mut states = [
            ResolutionState::Run,
            ResolutionState::Unverified,
            ResolutionState::Adopt,
        ];
        states.sort_by_key(|s| s.confidence_rank());
        assert_eq!(states[0], ResolutionState::Unverified);
    }
}
