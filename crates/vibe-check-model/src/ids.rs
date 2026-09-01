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

use crate::scope::{RECORD_SEPARATOR, RequirementScope};

/// Declare an interned-string identifier newtype.
///
/// # Two arms, and why the difference matters
///
/// The ordinary arm — `id_newtype! { Foo }` — emits infallible constructors:
/// `new`, `From<&str>` and `From<String>`. For an identifier that is *quoted*
/// from a document somebody else wrote, that is right: the value already exists,
/// this build's job is to carry it, and refusing to represent it is how you fail
/// open on a flag you could not parse.
///
/// The `@derived_only` arm emits everything except those three. It exists for
/// [`RequirementId`], whose value is not quoted from anywhere — it is *computed*
/// from a capability and a scope, and two computations that should differ must
/// not be able to agree. An infallible `new` next to that is not a convenience;
/// it is the bypass, and a bypass that is one keystroke shorter than the
/// derivation is the one that gets used.
///
/// The arms share a body rather than duplicating one: the ordinary arm invokes
/// the `@derived_only` arm and then adds the three constructors and a
/// pass-through `Deserialize`. `Deserialize` is added by the outer arm rather
/// than derived in the shared body for the same reason `LeafId` writes its own:
/// `#[serde(transparent)]` is a construction path no macro arm can suppress, so
/// a type that wants its wire form checked has to own that impl.
macro_rules! id_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        id_newtype! { @derived_only $(#[$meta])* $name }

        impl $name {
            /// Construct from anything string-like.
            #[must_use]
            pub fn new(id: impl AsRef<str>) -> Self {
                Self(SmolStr::new(id.as_ref()))
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

        // What `#[derive(Deserialize)]` with `#[serde(transparent)]` would
        // emit, written out because the shared body cannot derive it
        // conditionally. The wire form is a bare string either way.
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                Ok(Self(SmolStr::deserialize(deserializer)?))
            }
        }
    };

    (@derived_only $(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(SmolStr);

        impl $name {
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
    @derived_only
    /// Identifies a requirement — a *(capability × scope)* pair after the
    /// monorepo union.
    ///
    /// Requirements, not capabilities, are the unit of resolution. A bare
    /// capability identifier loses scope, and scope is what says Miri should run
    /// over `kono-core` alone rather than the whole workspace.
    ///
    /// # Form
    ///
    /// `req_<capability>_<16 hex>`, where the hex is the leading 128 bits of
    /// `blake3(capability ⧺ \u{1e} ⧺ scope.canonical_bytes())`. The capability
    /// stays readable because this string is a CI matrix entry and a `--id`
    /// argument, and somebody debugging a fan-out has to be able to see which
    /// question a leaf is answering.
    ///
    /// The readable half is *lossy* — see [`derive`](Self::derive) — and the
    /// digest is not: two capabilities that render to the same readable half
    /// still get different digests, because the digest is taken over the
    /// capability as written.
    ///
    /// # Construction
    ///
    /// [`derive`](Self::derive) computes one; [`from_wire`](Self::from_wire)
    /// re-reads one that was already computed. There is deliberately no `new`,
    /// no `From<&str>` and no `From<String>` — this is the one identifier in
    /// this module the workspace *mints* rather than quotes, and a hand-written
    /// value that merely looks plausible is the collision this type exists to
    /// prevent. The macro's `@derived_only` arm is what withholds them.
    ///
    /// # Why a collision is a fail-open
    ///
    /// [`Resolutions`](crate::resolution::Resolutions) is a map keyed by this
    /// type. Two requirements sharing an identifier means one resolution
    /// displaces the other, and a displaced *failing* resolution reads, from the
    /// outside, as a question that was answered. `Resolutions::insert` returns
    /// what it displaced so the caller can see it happen; this type's job is to
    /// make sure it does not happen.
    RequirementId
}

/// The prefix every requirement identifier carries.
const REQUIREMENT_PREFIX: &str = "req_";

/// How many hex characters of digest a requirement identifier carries.
///
/// 16 hex characters is 64 bits. Requirement identifiers are compared for
/// equality within a single run's plan — tens to low thousands of entries — so
/// the birthday bound is comfortable, and the string has to stay short enough to
/// read in a CI matrix entry.
const REQUIREMENT_DIGEST_HEX: usize = 16;

/// Whether `character` may appear in the readable half of a requirement
/// identifier.
///
/// `_` is deliberately excluded even though [`LeafId`] allows it: it is the
/// field separator, and excluding it from the field means the identifier
/// contains exactly two underscores and the digest boundary is unambiguous.
fn is_requirement_capability_char(character: char) -> bool {
    character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '.' | '-')
}

/// Why a recorded requirement identifier was rejected.
///
/// Shape only. None of these say the identifier was *correctly derived* — that
/// cannot be rechecked without the scope, which the wire form does not carry.
/// See [`RequirementId::from_wire`].
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RequirementIdError {
    /// Did not start with `req_`.
    #[error("a requirement id must start with `{REQUIREMENT_PREFIX}`, got {got:?}")]
    MissingPrefix {
        /// The value as it arrived.
        got: String,
    },

    /// Carried no `_` after the prefix, so there was no digest to find.
    #[error(
        "a requirement id must be `req_<capability>_<{REQUIREMENT_DIGEST_HEX} hex>`;          {got:?} has no digest separator"
    )]
    MissingDigest {
        /// The value as it arrived.
        got: String,
    },

    /// The trailing field was not exactly 16 lowercase hex characters.
    #[error(
        "a requirement id's digest must be {REQUIREMENT_DIGEST_HEX} lowercase hex          characters, got {got:?}"
    )]
    MalformedDigest {
        /// The trailing field as it arrived.
        got: String,
    },

    /// The readable half held something `derive` never emits.
    #[error(
        "a requirement id's capability may contain only lowercase letters, digits,          `.` and `-`; found {found:?} at byte {at} of {got:?}"
    )]
    DisallowedCharacter {
        /// The readable half as it arrived.
        got: String,
        /// Byte offset of the offending character within the readable half.
        at: usize,
        /// The offending character.
        found: char,
    },
}

impl RequirementId {
    /// Derive the identifier for a *(capability × scope)* pair.
    ///
    /// The same pair derives to the same identifier in every process and every
    /// run, and — because [`RequirementScope`] is a pair of sets — in every
    /// order the planner happened to union its inputs in.
    ///
    /// The readable half lowercases ASCII and maps everything outside
    /// `[a-z0-9.-]` to `-`, so the result is always safe to interpolate into a
    /// job matrix and a command line. That mapping is lossy and deliberately
    /// so: the digest is taken over the capability *as written*, so two
    /// capabilities that render alike still get different identifiers. Losing
    /// the distinction in the readable half costs legibility; losing it in the
    /// digest would cost a verdict.
    ///
    /// # Stability
    ///
    /// This function's output is a wire value. It appears in
    /// [`EvidenceRef::Requirement`](crate::reason::EvidenceRef::Requirement)
    /// inside every bundle, so changing the encoding invalidates every
    /// historical escalation reference. `requirement_ids_are_derived.rs` pins a
    /// golden identifier for exactly that reason.
    #[must_use]
    pub fn derive(capability: &CapabilityId, scope: &RequirementScope) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(capability.as_str().as_bytes());
        hasher.update(&[RECORD_SEPARATOR]);
        hasher.update(&scope.canonical_bytes());
        let digest = hasher.finalize().to_hex();

        let readable: String = capability
            .as_str()
            .chars()
            .map(|character| {
                let lowered = character.to_ascii_lowercase();
                if is_requirement_capability_char(lowered) {
                    lowered
                } else {
                    '-'
                }
            })
            .collect();

        let mut id = String::with_capacity(
            REQUIREMENT_PREFIX.len() + readable.len() + 1 + REQUIREMENT_DIGEST_HEX,
        );
        id.push_str(REQUIREMENT_PREFIX);
        id.push_str(&readable);
        id.push('_');
        id.push_str(&digest.as_str()[..REQUIREMENT_DIGEST_HEX]);
        Self(SmolStr::new(id))
    }

    /// Re-read an identifier that was already derived — a bundle field, a job
    /// matrix entry, or a `--id` argument.
    ///
    /// Deliberately not named `new`: it asserts nothing about derivation.
    ///
    /// # What is and is not checked
    ///
    /// The *shape* is checked: `req_`, a readable half in `[a-z0-9.-]*`, `_`,
    /// and exactly 16 lowercase hex characters. Whether that digest is the one
    /// [`derive`](Self::derive) would have produced cannot be checked here,
    /// because the scope it was taken over is not in the string — the digest is
    /// non-invertible on purpose, which is what keeps a bundle from leaking a
    /// private repository's crate names.
    ///
    /// So this is a *filter*, not a proof. It rejects `req_tests-pass_all`,
    /// which is what a person writes when they are inventing an identifier
    /// rather than reading one back, and that is the mistake worth catching.
    ///
    /// # Errors
    ///
    /// The [`RequirementIdError`] naming the first rule that failed.
    pub fn from_wire(id: impl AsRef<str>) -> Result<Self, RequirementIdError> {
        let id = id.as_ref();
        let Some(rest) = id.strip_prefix(REQUIREMENT_PREFIX) else {
            return Err(RequirementIdError::MissingPrefix { got: id.to_owned() });
        };
        let Some((capability, digest)) = rest.rsplit_once('_') else {
            return Err(RequirementIdError::MissingDigest { got: id.to_owned() });
        };
        if digest.len() != REQUIREMENT_DIGEST_HEX
            || !digest
                .chars()
                .all(|character| character.is_ascii_digit() || matches!(character, 'a'..='f'))
        {
            return Err(RequirementIdError::MalformedDigest {
                got: digest.to_owned(),
            });
        }
        for (at, found) in capability.char_indices() {
            if !is_requirement_capability_char(found) {
                return Err(RequirementIdError::DisallowedCharacter {
                    got: capability.to_owned(),
                    at,
                    found,
                });
            }
        }
        Ok(Self(SmolStr::new(id)))
    }
}

impl TryFrom<&str> for RequirementId {
    type Error = RequirementIdError;

    fn try_from(id: &str) -> Result<Self, Self::Error> {
        Self::from_wire(id)
    }
}

impl TryFrom<String> for RequirementId {
    type Error = RequirementIdError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::from_wire(id)
    }
}

// Hand-written rather than derived, so the shape check runs on the wire.
//
// This is the half of "`derive` is the only derivation path" that a macro arm
// cannot deliver: `#[derive(Deserialize)]` with `#[serde(transparent)]` accepts
// any string at all, and a bundle or a plan document is exactly where a
// hand-invented identifier would arrive from. Routing deserialization through
// `from_wire` closes the shape hole. It does not — and cannot — close the
// derivation hole; see `from_wire`.
impl<'de> Deserialize<'de> for RequirementId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = SmolStr::deserialize(deserializer)?;
        Self::from_wire(raw).map_err(serde::de::Error::custom)
    }
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
/// `/`, `\`, or `:`, which an artifact name rejects outright; no whitespace,
/// which splits an argument; nothing a shell would expand; and no uppercase,
/// because an artifact is eventually unpacked onto a filesystem and the macOS
/// and Windows runners are case-insensitive — so two ids differing only in case
/// would collide there and not on Linux, which is the worst way for a
/// uniqueness bug to present.
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
