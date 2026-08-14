//! Identifiers that may name something this build does not implement.
//!
//! # Why this module has no children
//!
//! As with [`crate::adjudicate::accumulator`], the guarantee here rests on
//! module-scoped privacy: `Inner` is private to this module, so no sibling code
//! can pattern-match around the escalation in [`Known::get`]. **Do not add
//! submodules.**
//!
//! # The problem this solves
//!
//! A policy document read from the merge base may be arbitrarily old, and the
//! binary reading it may be arbitrarily new — or the reverse. Either way, a
//! document can name a risk flag, capability, or parser that this build cannot
//! evaluate.
//!
//! Failing to parse the document is wrong: fail-closed means *escalate*, and you
//! cannot escalate over something you refused to represent. Silently dropping
//! the unknown entry is much worse — it converts "this build cannot check the
//! thing policy demanded" into "policy demanded nothing", which is a way to
//! disable a gate by typo.
//!
//! So the unknown case is representable, and the only way to reach the value
//! requires an [`Adjudicator`] which will be escalated as a side effect.

use smol_str::SmolStr;
use std::fmt;

use crate::adjudicate::Adjudicator;
use crate::reason::{EvidenceRef, ReasonCode};
use crate::tier::Tier;

/// What kind of thing an unknown identifier was supposed to name.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum UnknownKind {
    /// A risk flag no registered analyzer emits.
    RiskFlag,
    /// A capability this build does not implement.
    Capability,
    /// An evidence parser this build does not implement.
    Parser,
    /// A key the policy schema does not define.
    PolicyKey,
    /// A cost class outside the known set.
    CostClass,
    /// A scheduling lane outside the configured set.
    Lane,
}

impl UnknownKind {
    /// The reason code an unknown of this kind escalates with.
    #[must_use]
    pub fn reason(self) -> ReasonCode {
        match self {
            Self::RiskFlag => ReasonCode::UnknownRiskFlag,
            Self::Capability => ReasonCode::UnknownCapability,
            Self::Parser => ReasonCode::UnknownParser,
            Self::PolicyKey | Self::CostClass | Self::Lane => ReasonCode::UnknownPolicyKey,
        }
    }

    /// Human-readable noun, for error messages.
    #[must_use]
    pub fn noun(self) -> &'static str {
        match self {
            Self::RiskFlag => "risk flag",
            Self::Capability => "capability",
            Self::Parser => "parser",
            Self::PolicyKey => "policy key",
            Self::CostClass => "cost class",
            Self::Lane => "lane",
        }
    }
}

impl fmt::Display for UnknownKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.noun())
    }
}

/// Private. Keeping the variants unreachable outside this module is what makes
/// [`Known::get`] the only path to the value.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Inner<T> {
    Known(T),
    Unknown {
        id: SmolStr,
        kind: UnknownKind,
        origin: EvidenceRef,
    },
}

/// Either a resolved `T`, or an identifier this build does not recognize.
///
/// This is a *runtime resolution* type, not a wire type. It has no
/// `Deserialize`, because deciding whether an identifier is known requires a
/// registry that serde does not have. Bundles and policy documents store plain
/// identifier strings; they become `Known<T>` when resolved against a registry
/// via [`Known::resolve`].
///
/// `#[must_use]`: dropping one on the floor means an unknown identifier passed
/// through without escalating, which is the exact failure this type exists to
/// prevent.
#[must_use = "an unresolved identifier must be observed via `get`, which escalates when it is unknown"]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Known<T>(Inner<T>);

impl<T> Known<T> {
    /// Wrap a value that resolved successfully.
    pub fn resolved(value: T) -> Self {
        Self(Inner::Known(value))
    }

    /// Record an identifier that did not resolve.
    pub fn unresolved(id: impl Into<SmolStr>, kind: UnknownKind, origin: EvidenceRef) -> Self {
        Self(Inner::Unknown {
            id: id.into(),
            kind,
            origin,
        })
    }

    /// Resolve `id` through `lookup`, remembering the identifier either way.
    ///
    /// This is the constructor to reach for: it makes the unknown branch
    /// automatic rather than something each call site must remember to handle.
    pub fn resolve(
        id: impl Into<SmolStr>,
        kind: UnknownKind,
        origin: EvidenceRef,
        lookup: impl FnOnce(&str) -> Option<T>,
    ) -> Self {
        let id = id.into();
        match lookup(id.as_str()) {
            Some(value) => Self::resolved(value),
            None => Self::unresolved(id, kind, origin),
        }
    }

    /// Whether the identifier resolved.
    ///
    /// Useful for reporting; it deliberately does not hand back the value, so
    /// it cannot be used to route around [`get`](Self::get).
    #[must_use]
    pub fn is_known(&self) -> bool {
        matches!(self.0, Inner::Known(_))
    }

    /// The kind of thing that failed to resolve, if it failed.
    #[must_use]
    pub fn unknown_kind(&self) -> Option<UnknownKind> {
        match &self.0 {
            Inner::Known(_) => None,
            Inner::Unknown { kind, .. } => Some(*kind),
        }
    }

    /// Take the value, escalating to [`Tier::TOP`] when it did not resolve.
    ///
    /// **The only way to reach the inner value.** There is no `unwrap`, no
    /// `Deref`, no `into_option`, and no public variant to match on. Observing
    /// an unknown identifier without escalating is therefore not expressible.
    ///
    /// Returns `None` in the unknown case so the caller can skip work it can no
    /// longer do — the verdict has already been made safe by then.
    pub fn get(self, adjudicator: &mut Adjudicator) -> Option<T> {
        match self.0 {
            Inner::Known(value) => Some(value),
            Inner::Unknown { id, kind, origin } => {
                adjudicator.escalate(
                    Tier::TOP,
                    kind.reason(),
                    format!(
                        "policy references {} `{id}`, which this build does not implement; \
                         upgrade vibe-check or remove the reference. \
                         This change is escalated to human review until then.",
                        kind.noun()
                    ),
                    origin,
                );
                None
            }
        }
    }
}

impl<T: AsRef<str>> Known<T> {
    /// The identifier, whether or not it resolved.
    ///
    /// Non-value-leaking: renderers need to print the name of a capability they
    /// could not resolve, without being handed a value that does not exist.
    #[must_use]
    pub fn id(&self) -> &str {
        match &self.0 {
            Inner::Known(value) => value.as_ref(),
            Inner::Unknown { id, .. } => id.as_str(),
        }
    }
}

impl<T: AsRef<str>> fmt::Display for Known<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())?;
        if !self.is_known() {
            f.write_str(" (unknown)")?;
        }
        Ok(())
    }
}

impl<T: AsRef<str>> serde::Serialize for Known<T> {
    /// Serializes as the bare identifier. A bundle records what was named, not
    /// whether the build that wrote it happened to implement it — a later build
    /// may well resolve the same string successfully.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CapabilityId, RiskFlagId};

    fn registry_lookup(id: &str) -> Option<CapabilityId> {
        matches!(id, "tests-pass" | "api-diff-empty").then(|| CapabilityId::new(id))
    }

    #[test]
    fn a_known_identifier_resolves_without_escalating() {
        let mut adj = Adjudicator::new();
        let k = Known::resolve(
            "tests-pass",
            UnknownKind::Capability,
            EvidenceRef::Unattributed,
            registry_lookup,
        );
        assert!(k.is_known());
        assert_eq!(k.get(&mut adj), Some(CapabilityId::new("tests-pass")));
        assert_eq!(adj.tier(), Tier::T0);
        assert!(adj.ledger().is_empty());
    }

    #[test]
    fn an_unknown_identifier_escalates_to_the_top_when_observed() {
        let mut adj = Adjudicator::new();
        let k = Known::resolve(
            "loom-clean",
            UnknownKind::Capability,
            EvidenceRef::Unattributed,
            registry_lookup,
        );
        assert!(!k.is_known());
        assert_eq!(k.get(&mut adj), None);
        assert_eq!(adj.tier(), Tier::TOP);
        assert_eq!(adj.ledger().len(), 1);
        assert_eq!(adj.ledger()[0].reason, ReasonCode::UnknownCapability);
    }

    #[test]
    fn the_error_message_names_the_identifier_and_says_what_to_do() {
        // Per the repository conventions, errors must carry actionable context.
        // "unknown capability" alone leaves the reader to guess which one.
        let mut adj = Adjudicator::new();
        let _ = Known::<CapabilityId>::unresolved(
            "loom-clean",
            UnknownKind::Capability,
            EvidenceRef::Unattributed,
        )
        .get(&mut adj);
        let detail = &adj.ledger()[0].detail;
        assert!(
            detail.contains("loom-clean"),
            "names the identifier: {detail}"
        );
        assert!(detail.contains("capability"), "names the kind: {detail}");
        assert!(detail.contains("upgrade"), "says what to do: {detail}");
    }

    #[test]
    fn unknown_kinds_map_to_their_own_reason_codes() {
        // The escape-rate loop groups by reason code, so "unknown flag" and
        // "unknown capability" must not collapse into one bucket.
        assert_eq!(UnknownKind::RiskFlag.reason(), ReasonCode::UnknownRiskFlag);
        assert_eq!(
            UnknownKind::Capability.reason(),
            ReasonCode::UnknownCapability
        );
        assert_eq!(UnknownKind::Parser.reason(), ReasonCode::UnknownParser);
    }

    #[test]
    fn serializes_as_the_bare_identifier_in_both_states() {
        let known = Known::resolved(RiskFlagId::new("unsafe"));
        let unknown = Known::<RiskFlagId>::unresolved(
            "quantum-entanglement",
            UnknownKind::RiskFlag,
            EvidenceRef::Unattributed,
        );
        assert_eq!(
            serde_json::to_string(&known).expect("serialize"),
            r#""unsafe""#
        );
        // A bundle records what policy named. A later build may implement it.
        assert_eq!(
            serde_json::to_string(&unknown).expect("serialize"),
            r#""quantum-entanglement""#
        );
    }

    #[test]
    fn renders_the_unknown_state_visibly() {
        let unknown = Known::<RiskFlagId>::unresolved(
            "ffi",
            UnknownKind::RiskFlag,
            EvidenceRef::Unattributed,
        );
        assert_eq!(unknown.to_string(), "ffi (unknown)");
    }
}
