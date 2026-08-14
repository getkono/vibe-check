//! Normalized evidence, and where it came from.
//!
//! # Shape
//!
//! Heterogeneous artifacts — nextest JUnit, libtest JSON, `llvm-cov`,
//! `cargo-mutants` outcomes, SARIF, criterion — all normalize into
//! [`EvidenceFacts`], which is a **small closed core plus an open bag**.
//!
//! A single `enum EvidenceKind { Tests(..), Coverage(..), … }` would be a closed
//! set, so every new parser shape would be a breaking change. A bare
//! `serde_json::Value` would push all structure into capabilities and leave the
//! renderer with nothing generic to display. In practice adjudication and
//! rendering only ever need four shapes, so those four are typed and everything
//! else goes in `extra`.
//!
//! [`LocatedFinding`] in particular is what lets the renderer emit pull-request
//! annotations for *any* future parser without a renderer change.
//!
//! # Where evidence comes from
//!
//! An [`Evidence`] can only be built from a [`ParsedEvidence`], and a
//! [`ParsedEvidence`] is what a parser returns on success. There is deliberately
//! no `Evidence::new`, no `Default`, and no conversion from a check-run
//! conclusion — see [`Evidence::from_parsed`].

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::ids::{AdoptionSourceId, CapabilityId, MetricKey, ParserId};
use crate::location::{LineRange, Location};
use crate::schema::SchemaVersion;

/// How a single case turned out.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum CaseStatus {
    /// The case passed.
    Pass,
    /// The case ran and failed.
    Fail,
    /// The case was not run.
    Skip,
    /// The case could not run — it did not compile, or the harness died.
    ///
    /// Distinct from [`Fail`](Self::Fail), and the distinction is load-bearing:
    /// test-negation needs "the added test failed against the base commit"
    /// (proof) to be different from "the added test would not compile against
    /// the base commit" (inconclusive).
    Error,
    /// The case exceeded its time budget.
    Timeout,
}

impl CaseStatus {
    /// Whether this status represents a case that ran to a real conclusion.
    #[must_use]
    pub fn is_conclusive(self) -> bool {
        matches!(self, Self::Pass | Self::Fail)
    }
}

/// One test, mutant, or build configuration.
///
/// Covers nextest and libtest cases, `cargo-mutants` outcomes, and
/// feature-powerset build results — they are the same shape, and treating them
/// as one shape is why a new tool of this kind needs no new bundle section.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct CaseOutcome {
    /// Stable identifier, e.g. `kono_core::ring::tests::no_torn_read`.
    pub id: String,
    /// How it turned out.
    pub status: CaseStatus,
    /// Wall-clock duration in milliseconds, when the tool reported one.
    ///
    /// Display only — excluded from verdict digests, because it varies run to
    /// run and would make replay comparisons fail for no reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Where the case lives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    /// How many times it was retried before this result.
    ///
    /// Non-zero retries mean the result is not evidence of determinism, which
    /// matters for the flake probe.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retries: u32,
    /// Failure message, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// How serious a finding is.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Severity {
    /// Informational.
    Note,
    /// Should be looked at.
    Warning,
    /// Must be fixed.
    Error,
}

/// A finding with a source location.
///
/// Covers SARIF diagnostics, clippy lints, `cargo-deny` advisories, surviving
/// mutants, and public-API differences. One shape, so the renderer can turn any
/// of them into a pull-request annotation without knowing which tool produced
/// it — including tools that do not exist yet.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct LocatedFinding {
    /// Where it is.
    pub location: Location,
    /// How serious.
    pub severity: Severity,
    /// The rule or check that produced it, e.g. `clippy::needless_borrow`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// What it says.
    pub message: String,
}

impl LocatedFinding {
    /// Whether this finding falls inside any of the given changed ranges.
    ///
    /// Diff-scoped capabilities — surviving mutants, newly uncovered lines —
    /// only count findings the pull request is actually responsible for.
    /// A finding with no line information is treated as in scope when its file
    /// changed at all, because we cannot prove otherwise and guessing in the
    /// permissive direction is how things get missed.
    #[must_use]
    pub fn is_in_scope(&self, changed: &[LineRange]) -> bool {
        match self.location.lines {
            None => true,
            Some(lines) => changed.iter().any(|c| c.overlaps(&lines)),
        }
    }
}

/// A single measurement.
///
/// Stored as an integer with an explicit unit rather than a float. JSON floats
/// do not round-trip reliably, and a digest computed over a float is a
/// reproducibility landmine — the replay test would fail on the last decimal
/// place for reasons nobody could act on.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Metric {
    /// The value, scaled by `unit`.
    pub value: i64,
    /// The unit, e.g. `ns`, `ppm`, `count`, `permille`.
    pub unit: String,
}

impl Metric {
    /// A count of things.
    #[must_use]
    pub fn count(value: i64) -> Self {
        Self {
            value,
            unit: "count".into(),
        }
    }

    /// A proportion in parts per million, avoiding floats.
    #[must_use]
    pub fn ppm(value: i64) -> Self {
        Self {
            value,
            unit: "ppm".into(),
        }
    }
}

/// The normalized content of an artifact.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct EvidenceFacts {
    /// Tests, mutants, powerset builds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cases: Vec<CaseOutcome>,
    /// Diagnostics with locations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<LocatedFinding>,
    /// Scalar measurements, ordered so serialization is stable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<MetricKey, Metric>,
    /// Capability-private data, described by the capability's own declaration.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl EvidenceFacts {
    /// Cases that did not pass, in order.
    pub fn failures(&self) -> impl Iterator<Item = &CaseOutcome> {
        self.cases.iter().filter(|c| c.status == CaseStatus::Fail)
    }

    /// Whether any case failed to run at all, as opposed to running and failing.
    #[must_use]
    pub fn has_inconclusive_cases(&self) -> bool {
        self.cases
            .iter()
            .any(|c| matches!(c.status, CaseStatus::Error | CaseStatus::Timeout))
    }
}

/// A pointer to the artifact this evidence was parsed from.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RawRef {
    /// Path within the uploaded artifact.
    pub path: Utf8PathBuf,
    /// The format, as the producer declared it.
    pub format: String,
    /// SHA-256 of the bytes we actually parsed.
    ///
    /// Not of the bytes the producer claims to have written — of what we read.
    pub sha256: String,
}

/// Who says so, and how do we know.
///
/// This is the anti-gaming spine. Everything that makes an adopted artifact
/// trustworthy — which commit produced it, which workflow run, what its bytes
/// hashed to — lives here, and the adoption path refuses to proceed without it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Provenance {
    /// Consumed from an artifact some other job already produced.
    Adopted {
        /// Where it was found.
        source: AdoptionSourceId,
        /// Which commit the producing run was for.
        ///
        /// Checked against the pull request head. A force-push invalidates
        /// every adoption, which is the point: evidence for a commit that no
        /// longer exists is not evidence for the commit that does.
        produced_from_commit: String,
        /// When the producing run finished.
        produced_at: Timestamp,
        /// The workflow run, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workflow_run: Option<u64>,
        /// SHA-256 of the downloaded artifact.
        sha256: String,
    },
    /// Produced by vibe-check running a tool.
    Executed {
        /// Digest of the execution plan: program, arguments, environment
        /// allowlist, and determinism settings. Proves what was actually run,
        /// including whether retries were disabled.
        plan_digest: String,
        /// Process exit code.
        exit_code: i32,
        /// When it started.
        started_at: Timestamp,
        /// How long it took, in milliseconds. Display only.
        duration_ms: u64,
        /// The toolchain that ran it.
        toolchain: String,
    },
    /// Asserted by configuration rather than measured.
    ///
    /// A declaration is never enough to satisfy a capability — see
    /// [`crate::resolution::CapabilityResolution`]. It exists so that a
    /// deliberate waiver is recorded in the bundle with its reason, rather than
    /// being invisible.
    Declared {
        /// Who declared it, e.g. a policy reference.
        by: String,
        /// Why.
        reason: String,
    },
}

impl Provenance {
    /// Whether this provenance can support a capability being *satisfied*.
    ///
    /// A declaration cannot: somebody writing "this is fine" in a configuration
    /// file is not evidence that it is fine.
    #[must_use]
    pub fn is_measured(&self) -> bool {
        !matches!(self, Self::Declared { .. })
    }
}

/// The successful result of parsing an artifact.
///
/// The only thing an [`Evidence`] can be built from. A parser returns this on
/// success and a [`crate::resolution::UnverifiedReason`] on failure; there is no
/// third option, which is what makes "unparseable means unverified" a shape of
/// the code rather than a rule to remember.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParsedEvidence {
    capability: CapabilityId,
    parser: ParserId,
    facts: EvidenceFacts,
    raw: Option<RawRef>,
}

impl ParsedEvidence {
    /// Record a successful parse. Called by parsers, and by nothing else.
    #[must_use]
    pub fn new(capability: CapabilityId, parser: ParserId, facts: EvidenceFacts) -> Self {
        Self {
            capability,
            parser,
            facts,
            raw: None,
        }
    }

    /// Attach a pointer to the artifact this came from.
    #[must_use]
    pub fn with_raw(mut self, raw: RawRef) -> Self {
        self.raw = Some(raw);
        self
    }
}

/// Normalized evidence about one capability.
///
/// # Construction
///
/// [`from_parsed`](Self::from_parsed) is the only way to *produce* evidence.
/// There is no `new`, no `Default`, and — importantly — no conversion from a
/// check-run conclusion. A green check named `tests` may have run a subset,
/// excluded a feature, or skipped a target entirely; adopting it because of its
/// name would manufacture confidence out of a string. There is no
/// `From<CheckRun> for Evidence` anywhere in the workspace, and adding one would
/// be the single most damaging change someone could make to this codebase.
///
/// # The honest caveat
///
/// This type derives [`Deserialize`], so evidence can also be *reconstructed*
/// from a recorded bundle. That is deliberate and is not a hole: it is the
/// replay path, rehydrating evidence that already went through a parser when it
/// was first produced. The guarantee is about the production path — nothing in
/// the adoption or execution flow can reach an `Evidence` except through a
/// successful parse.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Evidence {
    schema: SchemaVersion,
    capability: CapabilityId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parser: Option<ParserId>,
    provenance: Provenance,
    #[serde(default)]
    facts: EvidenceFacts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    raw: Option<RawRef>,
    /// Fields written by a newer build than this one.
    ///
    /// Preserved rather than dropped, so that an older tool reading and
    /// rewriting a bundle does not silently destroy data. This is the opposite
    /// of how policy documents treat unknown keys, and the asymmetry is
    /// deliberate — see [`crate::schema`].
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    unknown: serde_json::Map<String, serde_json::Value>,
}

impl Evidence {
    /// Build evidence from a successful parse. **The only producer.**
    #[must_use]
    pub fn from_parsed(parsed: ParsedEvidence, provenance: Provenance) -> Self {
        Self {
            schema: SchemaVersion::EVIDENCE,
            capability: parsed.capability,
            parser: Some(parsed.parser),
            provenance,
            facts: parsed.facts,
            raw: parsed.raw,
            unknown: serde_json::Map::new(),
        }
    }

    /// The capability this answers.
    #[must_use]
    pub fn capability(&self) -> &CapabilityId {
        &self.capability
    }

    /// The parser that produced it, if it came from an artifact.
    #[must_use]
    pub fn parser(&self) -> Option<&ParserId> {
        self.parser.as_ref()
    }

    /// Where it came from.
    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// The normalized content.
    #[must_use]
    pub fn facts(&self) -> &EvidenceFacts {
        &self.facts
    }

    /// The artifact it was parsed from, if any.
    #[must_use]
    pub fn raw(&self) -> Option<&RawRef> {
        self.raw.as_ref()
    }

    /// The schema version it was written at.
    #[must_use]
    pub fn schema(&self) -> SchemaVersion {
        self.schema
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts_with_one_pass() -> EvidenceFacts {
        EvidenceFacts {
            cases: vec![CaseOutcome {
                id: "core::tests::works".into(),
                status: CaseStatus::Pass,
                duration_ms: Some(3),
                location: None,
                retries: 0,
                message: None,
            }],
            ..EvidenceFacts::default()
        }
    }

    fn executed() -> Provenance {
        Provenance::Executed {
            plan_digest: "blake3:abcd".into(),
            exit_code: 0,
            started_at: Timestamp::UNIX_EPOCH,
            duration_ms: 12,
            toolchain: "1.97.1-x86_64-unknown-linux-gnu".into(),
        }
    }

    #[test]
    fn evidence_carries_the_parser_that_produced_it() {
        let parsed = ParsedEvidence::new(
            CapabilityId::new("tests-pass"),
            ParserId::new("junit@1"),
            facts_with_one_pass(),
        );
        let ev = Evidence::from_parsed(parsed, executed());
        assert_eq!(ev.capability().as_str(), "tests-pass");
        assert_eq!(ev.parser().map(ParserId::as_str), Some("junit@1"));
        assert_eq!(ev.schema(), SchemaVersion::EVIDENCE);
    }

    #[test]
    fn a_declaration_is_not_a_measurement() {
        // Somebody writing "this is fine" in a config file must not be able to
        // satisfy a capability. The resolution layer relies on this predicate.
        assert!(
            !Provenance::Declared {
                by: "policy#skip:macros-no-miri".into(),
                reason: "proc-macro crate forbids unsafe".into(),
            }
            .is_measured()
        );
        assert!(executed().is_measured());
    }

    #[test]
    fn error_and_timeout_are_not_conclusive() {
        // test-negation depends on this: a test that fails to compile against
        // the base commit proves nothing, while one that compiles and fails
        // proves what we wanted.
        assert!(CaseStatus::Pass.is_conclusive());
        assert!(CaseStatus::Fail.is_conclusive());
        assert!(!CaseStatus::Error.is_conclusive());
        assert!(!CaseStatus::Timeout.is_conclusive());
    }

    #[test]
    fn findings_scope_to_the_changed_lines() {
        let changed = [LineRange { start: 10, end: 20 }];
        let inside = LocatedFinding {
            location: Location::file("a.rs").at_lines(LineRange::single(15)),
            severity: Severity::Warning,
            rule: None,
            message: "m".into(),
        };
        let outside = LocatedFinding {
            location: Location::file("a.rs").at_lines(LineRange::single(30)),
            severity: Severity::Warning,
            rule: None,
            message: "m".into(),
        };
        let unlocated = LocatedFinding {
            location: Location::file("a.rs"),
            severity: Severity::Warning,
            rule: None,
            message: "m".into(),
        };
        assert!(inside.is_in_scope(&changed));
        assert!(!outside.is_in_scope(&changed));
        // Cannot prove it is out of scope, so it stays in. Under-reporting is
        // the dangerous direction.
        assert!(unlocated.is_in_scope(&changed));
    }

    #[test]
    fn unknown_fields_survive_a_round_trip() {
        // An older build must not destroy data written by a newer one. This is
        // what keeps the escape-rate loop's historical record intact across
        // upgrades.
        let json = serde_json::json!({
            "schema": 1,
            "capability": "tests-pass",
            "parser": "junit@1",
            "provenance": {
                "kind": "executed",
                "plan_digest": "blake3:abcd",
                "exit_code": 0,
                "started_at": "1970-01-01T00:00:00Z",
                "duration_ms": 12,
                "toolchain": "x"
            },
            "facts": {},
            "a_field_from_the_future": {"nested": [1, 2, 3]}
        });
        let ev: Evidence = serde_json::from_value(json).expect("deserialize");
        let out = serde_json::to_value(&ev).expect("serialize");
        assert_eq!(
            out.get("a_field_from_the_future"),
            Some(&serde_json::json!({"nested": [1, 2, 3]})),
            "a field this build does not understand must survive re-serialization"
        );
    }

    #[test]
    fn empty_collections_stay_out_of_the_wire_form() {
        let ev = Evidence::from_parsed(
            ParsedEvidence::new(
                CapabilityId::new("tests-pass"),
                ParserId::new("junit@1"),
                EvidenceFacts::default(),
            ),
            executed(),
        );
        let out = serde_json::to_value(&ev).expect("serialize");
        let facts = out.get("facts").expect("facts present");
        assert_eq!(facts, &serde_json::json!({}));
    }
}
