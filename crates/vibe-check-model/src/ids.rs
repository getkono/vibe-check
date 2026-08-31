//! Domain identifiers.
//!
//! Every identifier here is a newtype over an interned string, **not** a closed
//! `enum`. That is a deliberate and load-bearing choice.
//!
//! An `enum RiskFlag { PublicApi, Unsafe, … }` looks tempting because the set is
//! enumerated in the specification. It fails for two reasons:
//!
//! 1. Every new flag becomes a breaking change to every `match` in the tree, so
//!    the thing we most want to be additive becomes the thing that is hardest to
//!    add.
//! 2. More seriously, it breaks fail-closed. A bundle written six months ago, or
//!    a policy written against a newer binary, may name a flag this build has
//!    never heard of. With an enum, that document does not *deserialize* — and
//!    you cannot escalate a verdict because of a flag you were unable to
//!    represent. Unknown has to be representable in order to be dangerous.
//!
//! See [`crate::Known`] for how an unknown identifier is prevented from being
//! observed without escalating.

use std::fmt;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Declare an interned-string identifier newtype.
macro_rules! id_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(SmolStr);

        impl $name {
            /// Construct from anything string-like.
            #[must_use]
            pub fn new(id: impl AsRef<str>) -> Self {
                Self(SmolStr::new(id.as_ref()))
            }

            /// Borrow the identifier as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.0.as_str())
            }
        }

        // `Debug` prints as `Name("value")` so tracing output and assertion
        // failures say which kind of identifier is involved, not just its text.
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({:?})"), self.0.as_str())
            }
        }

        impl From<&str> for $name {
            fn from(id: &str) -> Self {
                Self::new(id)
            }
        }

        impl From<String> for $name {
            fn from(id: String) -> Self {
                Self(SmolStr::new(id))
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.0.as_str()
            }
        }
    };
}

id_newtype! {
    /// Identifies a capability — a *question* about a change, never a tool.
    ///
    /// `tests-pass`, `api-diff-empty`, `mutants-in-diff-killed`. Naming the
    /// question rather than the answer is what lets the same capability be
    /// satisfied by adopting an existing CI artifact in one repository and by
    /// running a tool in another.
    CapabilityId
}

id_newtype! {
    /// Identifies a risk flag emitted by the classifier.
    ///
    /// `public-api`, `unsafe`, `atomics`, `new-dep`, `hot-path`.
    RiskFlagId
}

id_newtype! {
    /// Identifies an evidence parser, e.g. `junit@1`, `libtest-json@2`.
    ///
    /// The version suffix is part of the identifier: a parser whose output shape
    /// changes is a *different* parser, so that policy written against the old
    /// one keeps meaning what it said.
    ParserId
}

id_newtype! {
    /// Identifies a risk analyzer — the unit that emits flags.
    ///
    /// Distinct from [`RiskFlagId`] because several analyzers may legitimately
    /// emit the same flag from different evidence.
    AnalyzerId
}

id_newtype! {
    /// Identifies an adoption source: where an existing artifact was found.
    AdoptionSourceId
}

id_newtype! {
    /// Identifies a lane — a scheduling class with a time budget.
    ///
    /// `cheap` and `conditional` are the shipped defaults, but this is an open
    /// identifier rather than a two-variant enum so that a `nightly` or
    /// `merge-queue` lane is configuration rather than a refactor.
    LaneId
}

id_newtype! {
    /// Identifies a rule in the policy document.
    ///
    /// Every `[[rule]]` carries a mandatory unique `id` as its first key, so a
    /// diff hunk of the policy file always shows which rule changed. This type
    /// is what carries that identity into the bundle, so a verdict can point at
    /// the exact rule that caused it.
    RuleId
}

id_newtype! {
    /// Identifies a crate in the workspace, or a pseudo-crate for paths that
    /// belong to no crate (`@workspace`, `@ci`, `@policy`, `@other`).
    ///
    /// Pseudo-crates participate in policy scoping identically to real ones,
    /// which is what keeps "changes to CI configuration" from needing a special
    /// case in the engine.
    CrateId
}

id_newtype! {
    /// Identifies a metric within evidence, e.g. `coverage.lines.pct`.
    MetricKey
}

id_newtype! {
    /// Identifies a requirement — a *(capability × scope)* pair after the
    /// monorepo union.
    ///
    /// Requirements, not capabilities, are the unit of resolution. A bare
    /// capability identifier loses scope, and scope is what says Miri should run
    /// over `kono-core` alone rather than the whole workspace. Derived from a
    /// digest of the capability and its canonicalized scope, so the same
    /// requirement computed twice gets the same identifier.
    RequirementId
}

id_newtype! {
    /// Identifies a fact produced by an extractor and consumed by analyzers.
    FactKey
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let id = CapabilityId::new("tests-pass");
        let json = serde_json::to_string(&id).expect("serialize");
        // `serde(transparent)` means the wire form is a bare string, not an
        // object. Bundles are read by tools that are not this binary.
        assert_eq!(json, r#""tests-pass""#);
        let back: CapabilityId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, id);
    }

    #[test]
    fn an_unrecognized_identifier_still_parses() {
        // The whole point of interned strings over an enum: a flag this build
        // has never heard of must survive deserialization so that it can be
        // escalated on. See `Known`.
        let flag: RiskFlagId =
            serde_json::from_str(r#""some-flag-from-the-future""#).expect("deserialize");
        assert_eq!(flag.as_str(), "some-flag-from-the-future");
    }

    #[test]
    fn orders_lexicographically_for_stable_output() {
        let mut ids = [
            CapabilityId::new("tests-pass"),
            CapabilityId::new("api-diff-empty"),
            CapabilityId::new("mutants-in-diff-killed"),
        ];
        ids.sort();
        assert_eq!(
            ids.iter().map(CapabilityId::as_str).collect::<Vec<_>>(),
            ["api-diff-empty", "mutants-in-diff-killed", "tests-pass"]
        );
    }

    #[test]
    fn debug_names_the_identifier_kind() {
        assert_eq!(
            format!("{:?}", RiskFlagId::new("unsafe")),
            r#"RiskFlagId("unsafe")"#
        );
    }
}
