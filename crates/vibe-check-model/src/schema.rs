//! Document versioning.
//!
//! Three independent version streams run through vibe-check, and none of them is
//! tied to the binary's own semantic version — `release-plz` bumps that on every
//! feature, and a bundle schema bump every Tuesday would make the schema
//! meaningless:
//!
//! - **Policy documents.** Read from the merge base, so arbitrarily old versions
//!   must stay loadable *indefinitely*. Migrated forward through a chain of
//!   total functions.
//! - **Evidence bundles.** Written by new tools, read by old ones and vice
//!   versa, across the whole history the escape-rate loop looks at.
//! - **The capability registry.** Hashed into every bundle so historical
//!   verdicts can be attributed to the rules that were actually in force.
//!
//! # The asymmetry that matters
//!
//! Unknown fields in **policy** are a hard error. Unknown fields in **bundles**
//! are preserved.
//!
//! Policy is adversarial input: a silently ignored misspelling of a
//! security-relevant key is a way to weaken a gate that passes review because
//! the diff looks right. Bundles are archive output: silently dropping a field
//! an older reader does not understand corrupts the record.
//!
//! Getting these two backwards is a security bug, not a style inconsistency.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A major schema version.
///
/// Major only. Within a major version, changes must be additive — new optional
/// fields, new `#[non_exhaustive]` variants — never a removal, a rename, or a
/// retype. A test asserts the regenerated JSON Schema is a superset of the
/// committed one, so this is checked rather than promised.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(pub u32);

impl SchemaVersion {
    /// The evidence-envelope schema this build writes.
    pub const EVIDENCE: Self = Self(1);
    /// The bundle schema this build writes.
    pub const BUNDLE: Self = Self(1);
    /// The policy schema this build understands.
    pub const POLICY: Self = Self(1);

    /// Whether a document at this version can be read by a build that writes
    /// `current`.
    ///
    /// Older is fine — it migrates forward. Newer is not: a document from the
    /// future may rely on semantics this build does not implement, and guessing
    /// is how a gate gets silently skipped. The caller escalates.
    #[must_use]
    pub fn readable_by(self, current: Self) -> bool {
        self <= current
    }
}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_future_document_is_not_readable() {
        // Never best-effort. A policy declaring a schema we do not implement
        // escalates rather than being partially applied.
        assert!(!SchemaVersion(2).readable_by(SchemaVersion(1)));
        assert!(SchemaVersion(1).readable_by(SchemaVersion(1)));
        assert!(SchemaVersion(1).readable_by(SchemaVersion(2)));
    }

    #[test]
    fn serializes_as_a_bare_integer() {
        assert_eq!(
            serde_json::to_string(&SchemaVersion::BUNDLE).expect("serialize"),
            "1"
        );
    }
}
