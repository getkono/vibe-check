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

/// The greatest length a leaf identifier may have, in bytes.
///
/// Bytes and characters coincide for every identifier that can be accepted,
/// because the accepted alphabet is ASCII.
const LEAF_ID_MAX_LEN: usize = 64;

/// Whether `character` may appear after the first one.
fn is_leaf_id_char(character: char) -> bool {
    character.is_ascii_lowercase()
        || character.is_ascii_digit()
        || matches!(character, '.' | '_' | '-')
}

/// Why a leaf identifier was rejected.
///
/// Each variant names the rule that failed rather than restating the pattern.
/// The value handed to [`LeafId::new_checked`] may have come from a plan
/// document, a job matrix, or a `--id` flag someone typed, and "which character,
/// at which offset" is the difference between a message that gets fixed and one
/// that gets shrugged at.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LeafIdError {
    /// The identifier was empty.
    #[error("a leaf id must not be empty")]
    Empty,

    /// Longer than 64 bytes.
    #[error("a leaf id must be at most {LEAF_ID_MAX_LEN} bytes, got {len}")]
    TooLong {
        /// How long it actually was, in bytes.
        len: usize,
    },

    /// Did not begin with a lowercase ASCII letter or a digit.
    #[error("a leaf id must start with a lowercase letter or a digit, found {found:?}")]
    BadLeadingCharacter {
        /// The offending first character.
        found: char,
    },

    /// Contained something outside `[a-z0-9._-]`.
    #[error(
        "a leaf id may contain only lowercase letters, digits, `.`, `_`, and `-`; \
         found {found:?} at byte {at}"
    )]
    DisallowedCharacter {
        /// Byte offset of the offending character.
        at: usize,
        /// The offending character.
        found: char,
    },
}

/// Identifies one leaf of a plan: a single capability against a single scope,
/// dispatched as one unit of work.
///
/// Every identifier matches `^[a-z0-9][a-z0-9._-]{0,63}$`, and
/// [`new_checked`](LeafId::new_checked) is the only way to obtain one.
///
/// # Why this one is checked when the rest of this module is not
///
/// The identifiers above are interned strings precisely so that an unknown value
/// stays representable: you cannot fail closed on a risk flag you refused to
/// parse, and that vocabulary arrives from a policy document somebody else wrote
/// against a build that may be newer than this one.
///
/// A leaf id is the opposite kind of value. It is a token *we* mint, from our own
/// plan, and there is no future leaf-id shape our own scheduler emits and this
/// build must tolerate. Nothing is lost by refusing a malformed one, and a great
/// deal is lost by accepting it, because the id is not only read — it is
/// *interpolated*. It crosses four process boundaries on the way to becoming
/// evidence:
///
/// ```text
/// plan JSON → $GITHUB_OUTPUT → ${{ matrix.id }} → --id → artifact name → adjudicate
/// ```
///
/// Each hop is a chance to get the encoding wrong, and two of them are shell and
/// workflow-expression contexts. The alphabet here is the intersection of what
/// survives all of them and what GitHub accepts as an artifact-name suffix: no
/// uppercase (artifact names are matched case-insensitively, so two ids
/// differing only in case would collide), no `/` or `\` or `:` (rejected
/// outright in an artifact name), no whitespace, and nothing a shell would
/// expand.
///
/// # Uniqueness
///
/// A leaf id must be unique within a run, because it is also the artifact-name
/// suffix and uploading two artifacts under one name is an error rather than a
/// merge. That is a property of a *set* of ids and no constructor can enforce
/// it — this type makes each id well-formed and comparable, so the planner that
/// mints them can enforce uniqueness with [`Eq`] and [`Ord`] rather than with
/// string handling.
///
/// # Construction
///
/// There is deliberately no `new`, no `From<&str>`, and no `From<String>`. This
/// type is written out by hand instead of through `id_newtype!` for that single
/// reason: the macro's infallible constructors would make
/// `LeafId::from("../../etc/passwd")` compile, and the constructor being the
/// only way in is the entire point.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct LeafId(SmolStr);

impl LeafId {
    /// Construct from anything string-like, rejecting anything GitHub will not
    /// accept as an artifact-name suffix.
    ///
    /// # Errors
    /// Returns the [`LeafIdError`] naming the first rule that failed: empty,
    /// too long, a bad leading character, or a disallowed character and where it
    /// was found.
    pub fn new_checked(id: impl AsRef<str>) -> Result<Self, LeafIdError> {
        let id = id.as_ref();
        let mut characters = id.char_indices();
        let Some((_, first)) = characters.next() else {
            return Err(LeafIdError::Empty);
        };
        if id.len() > LEAF_ID_MAX_LEN {
            return Err(LeafIdError::TooLong { len: id.len() });
        }
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            return Err(LeafIdError::BadLeadingCharacter { found: first });
        }
        for (at, found) in characters {
            if !is_leaf_id_char(found) {
                return Err(LeafIdError::DisallowedCharacter { at, found });
            }
        }
        Ok(Self(SmolStr::new(id)))
    }

    /// Borrow the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for LeafId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

// Matches the format `id_newtype!` produces, so a tracing line or an assertion
// failure reads the same whichever kind of identifier is involved.
impl fmt::Debug for LeafId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LeafId({:?})", self.0.as_str())
    }
}

impl AsRef<str> for LeafId {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl TryFrom<&str> for LeafId {
    type Error = LeafIdError;

    fn try_from(id: &str) -> Result<Self, Self::Error> {
        Self::new_checked(id)
    }
}

impl TryFrom<String> for LeafId {
    type Error = LeafIdError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new_checked(id)
    }
}

// Hand-written rather than derived, so the check runs on the wire.
//
// `Serialize` is `#[serde(transparent)]` and emits a bare string, as every other
// identifier here does. Deserialization reads that string and passes it through
// `new_checked`, which is what makes the plan-JSON hop actually validated rather
// than nominally typed: a `LeafId` that exists has been checked, no matter which
// of the four boundaries it arrived across.
impl<'de> Deserialize<'de> for LeafId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = SmolStr::deserialize(deserializer)?;
        Self::new_checked(raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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

    /// The pattern `LeafId` promises, written out so the property tests below
    /// have something independent to check `new_checked` against.
    fn matches_leaf_id_pattern(id: &str) -> bool {
        let mut characters = id.chars();
        let Some(first) = characters.next() else {
            return false;
        };
        id.len() <= LEAF_ID_MAX_LEN
            && (first.is_ascii_lowercase() || first.is_ascii_digit())
            && characters.all(is_leaf_id_char)
    }

    #[test]
    fn accepts_the_ids_the_planner_mints() {
        for id in [
            "a",
            "0",
            "miri-core-0",
            "tests.workspace",
            "api_diff",
            "z9",
            &"a".repeat(LEAF_ID_MAX_LEN),
        ] {
            let leaf = LeafId::new_checked(id).expect("a well-formed leaf id");
            assert_eq!(leaf.as_str(), id);
            assert_eq!(leaf.to_string(), id);
        }
    }

    #[test]
    fn rejects_what_would_not_survive_the_four_hops() {
        // Each of these is a real failure mode rather than a spelling nit:
        // uppercase collides with a lowercase id in a case-insensitive artifact
        // name, a leading `-` reads as a flag, `/` and `..` are the path-traversal
        // shape, whitespace splits an argument, and `$( )` is command
        // substitution in the shell hop.
        for bad in [
            "",
            "Miri-core-0",
            "-leading",
            ".leading",
            "_leading",
            "a/b",
            "../../etc/passwd",
            "..",
            "a b",
            "$(rm -rf /)",
            "a\\nb",
            "café",
            &"a".repeat(LEAF_ID_MAX_LEN + 1),
        ] {
            assert!(
                LeafId::new_checked(bad).is_err(),
                "{bad:?} must not be a leaf id"
            );
            assert!(!matches_leaf_id_pattern(bad));
        }
    }

    #[test]
    fn an_error_names_the_rule_that_failed() {
        // A caller who typed the id needs to know which character to change.
        assert_eq!(LeafId::new_checked(""), Err(LeafIdError::Empty));
        assert_eq!(
            LeafId::new_checked("-x"),
            Err(LeafIdError::BadLeadingCharacter { found: '-' })
        );
        assert_eq!(
            LeafId::new_checked("a/b"),
            Err(LeafIdError::DisallowedCharacter { at: 1, found: '/' })
        );
        assert_eq!(
            LeafId::new_checked("a".repeat(65)),
            Err(LeafIdError::TooLong { len: 65 })
        );
        assert!(
            LeafId::new_checked("a/b")
                .expect_err("rejected")
                .to_string()
                .contains("at byte 1")
        );
    }

    #[test]
    fn the_wire_form_is_a_bare_string_like_every_other_identifier() {
        let id = LeafId::new_checked("miri-core-0").expect("valid");
        assert_eq!(
            serde_json::to_string(&id).expect("serialize"),
            r#""miri-core-0""#
        );
        assert_eq!(format!("{id:?}"), r#"LeafId("miri-core-0")"#);
    }

    #[test]
    fn deserialization_runs_the_same_check_as_the_constructor() {
        // The point of the hand-written `Deserialize`. Plan JSON is one of the
        // four hops, and a nominally-typed `LeafId` that never ran the check
        // would make the type decorative exactly where it matters most.
        let error = serde_json::from_str::<LeafId>(r#""../../etc/passwd""#)
            .expect_err("must not deserialize");
        assert!(
            error.to_string().contains("must start with"),
            "the serde error must carry the rule that failed, got {error}"
        );
    }

    #[test]
    fn try_from_is_the_only_other_way_in() {
        assert_eq!(LeafId::try_from("ok-1").expect("valid").as_str(), "ok-1");
        assert_eq!(
            LeafId::try_from("ok-1".to_owned()).expect("valid").as_str(),
            "ok-1"
        );
        assert!(LeafId::try_from("NOT OK").is_err());
    }

    proptest! {
        /// `new_checked` accepts exactly the pattern the type documents —
        /// no more, and no less.
        #[test]
        fn new_checked_agrees_with_the_documented_pattern(raw in ".{0,80}") {
            prop_assert_eq!(
                LeafId::new_checked(&raw).is_ok(),
                matches_leaf_id_pattern(&raw)
            );
        }

        /// Anything accepted survives the plan-JSON hop unchanged.
        #[test]
        fn every_accepted_id_round_trips_through_json(raw in "[a-z0-9][a-z0-9._-]{0,63}") {
            let id = LeafId::new_checked(&raw).expect("matches the pattern");
            let json = serde_json::to_string(&id).expect("serialize");
            let back: LeafId = serde_json::from_str(&json).expect("deserialize");
            prop_assert_eq!(back, id);
        }

        /// And anything rejected is rejected on the wire too, rather than
        /// slipping in because it arrived as JSON instead of through the
        /// constructor.
        #[test]
        fn a_rejected_id_does_not_deserialize(raw in ".{0,80}") {
            prop_assume!(LeafId::new_checked(&raw).is_err());
            let json = serde_json::to_string(&raw).expect("serialize a plain string");
            prop_assert!(serde_json::from_str::<LeafId>(&json).is_err());
        }
    }
}
