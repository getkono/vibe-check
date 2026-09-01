//! Canonical JSON and the two digests computed over it.
//!
//! Every determinism lint in `clippy.toml` exists to protect a value. This
//! module computes it. Until it existed,
//! [`BundleCore::verdict_digest`](crate::bundle::BundleCore::verdict_digest) was a
//! documented field with no producer, and the "same diff plus same policy yields
//! the same verdict" guarantee rested entirely on lints that stop you writing
//! the bug rather than on anything that detects it.
//!
//! # Two digests, and they are not the same shape
//!
//! [`verdict_digest`] answers *did this evaluation decide the same thing?*
//! [`bundle_id`] answers *is this the same document?* Those are different
//! questions and they take opposite defaults.
//!
//! **`verdict_digest` is an inclusion list.** It covers exactly the paths named
//! in [`VERDICT_DIGEST_PATHS`] and nothing else. This is the direction that
//! survives the bundle growing: `EvidenceBundle` has no section holding
//! resolutions or evidence today and will gain one, and under an exclusion list
//! *adding any section silently changes every historical verdict digest*. The
//! replay corpus would then fail on a change with nothing to do with
//! adjudication — which is the precise failure the corpus exists to catch, so
//! the false positive is expensive twice: once for the wasted investigation, and
//! again for the credibility the signal loses. A new section does not enter
//! `verdict_digest` until someone adds a line to [`VERDICT_DIGEST_PATHS`].
//!
//! **`bundle_id` is an exclusion list.** It is a content address of the
//! artifact, not of the decision, and nobody replays against it, so covering
//! everything by default is right: a field nobody thought about should change
//! the identity of the document that carries it. [`BUNDLE_ID_EXCLUDED_PATHS`]
//! holds the few paths that must not participate.
//!
//! # What is *not* on the inclusion list yet
//!
//! Roughly half the fields that belong in a verdict digest do not exist. There
//! is no evidence section on [`EvidenceBundle`]: `Provenance`, `CaseOutcome`,
//! and `EvidenceFacts` live on [`Evidence`](crate::evidence::Evidence), which
//! the bundle does not carry, and an
//! [`Escalation`](crate::adjudicate::Escalation) holds only an
//! [`EvidenceRef`](crate::reason::EvidenceRef) — a *reference*, not the thing.
//!
//! Those rows are recorded in [`DEFERRED_VERDICT_DIGEST_PATHS`] rather than in
//! the live list, and the distinction is load-bearing rather than tidy. A path
//! in the live list that matches nothing is, at run time, indistinguishable from
//! a path whose field was renamed: both project nothing, both silently shrink
//! what the digest covers, and neither says a word. Keeping the live list
//! resolvable is what lets
//! [`every_live_inclusion_path_resolves`](self#tests) exist — a test that reads
//! every path in [`VERDICT_DIGEST_PATHS`] against a fully populated bundle and
//! fails if one of them finds nothing. That test is the actual guard against a
//! rename quietly narrowing the digest, and it cannot be written at all if the
//! list is allowed to carry paths for fields nobody has built.
//!
//! # RFC 8785, and why not `serde_json::to_string`
//!
//! Serializing a `BTreeMap` is not JCS. Two things differ:
//!
//! - **Key order is UTF-16 code-unit order**, not UTF-8 byte order. These agree
//!   across the whole BMP and disagree above it: a supplementary character
//!   encodes in UTF-16 as a surrogate pair starting in `U+D800..=U+DBFF`, so it
//!   sorts *below* `U+E000..=U+FFFF`, while by code point — which is what Rust's
//!   `str` ordering gives you — it sorts above. Two implementations that
//!   disagree here produce different digests for the same document, which is a
//!   replay failure that looks like a verdict change.
//! - **Numbers use ECMAScript `Number::toString`**, over IEEE-754 doubles.
//!
//! # Floats are refused, not formatted
//!
//! [`canonicalize`] returns an error for any non-integer number, and for any
//! integer outside the range ECMAScript represents exactly
//! (`±(2^53 − 1)`). Every typed number in the model is integral and
//! [`Metric`](crate::evidence::Metric) already explains why, so nothing
//! legitimate is rejected. What this refuses is the untyped case: a parser can
//! put a float into [`EvidenceFacts::extra`](crate::evidence::EvidenceFacts) and
//! a newer build can put one into [`EvidenceBundle::extensions`], and both of
//! those reach [`bundle_id`].
//!
//! Refusing is the conservative choice. ECMAScript's number formatting is
//! shortest-round-trip formatting — the Ryū/Grisu family of algorithms — and an
//! implementation that is *almost* right is worse than none: it produces digests
//! that agree with other implementations on every value anybody tests and
//! disagree on some value in production, which is a corpus failure nobody can
//! reproduce. A canonicalizer that says "I cannot canonicalize this" is a build
//! error at the point of the mistake. When a float genuinely needs to be
//! digested, the fix is to implement the formatting deliberately, with the
//! conformance vectors, in its own change.

use std::fmt;
use std::str::FromStr;

use serde_json::{Map, Value};
use smol_str::SmolStr;

use crate::bundle::EvidenceBundle;

/// One entry on a digest's path list, with the reason it is there.
///
/// The list is data in one place rather than `#[serde(skip)]` attributes
/// scattered across the types. Scattered attributes cannot be read as a set, so
/// nobody can answer "what does the verdict digest cover?" without reading every
/// type in the crate — and nobody can test the answer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct DigestPath {
    /// Slash-separated path from the document root. The segment `[]` matches
    /// every element of an array; every other segment is an object key.
    ///
    /// A path whose parent is absent matches nothing and contributes nothing,
    /// which is correct for an optional field (`core/pr`) and is a silent
    /// narrowing for a renamed one — see the module documentation for the test
    /// that separates the two.
    pub path: &'static str,
    /// Why this path is on this list. Not decoration: an entry whose reason
    /// cannot be written is an entry somebody added because a test failed.
    pub reason: &'static str,
}

/// The paths [`verdict_digest`] covers. Adding a bundle section does not extend
/// this list; only editing this list does.
pub const VERDICT_DIGEST_PATHS: &[DigestPath] = &[
    DigestPath {
        path: "schema_version",
        reason: "the inclusion list below is versioned by the bundle schema \
                 major, so a digest computed under one major must not compare \
                 equal to a digest computed under another",
    },
    // `core`, field by field rather than wholesale, so that a field added to
    // the frozen core — which is not supposed to happen, but the list must not
    // depend on that — cannot enter the digest without this list saying so.
    DigestPath {
        path: "core/repo",
        reason: "identity of what was evaluated; the same diff in a different \
                 repository is not the same evaluation",
    },
    DigestPath {
        path: "core/pr",
        reason: "identity; absent for local runs, which is itself a distinction \
                 worth digesting",
    },
    DigestPath {
        path: "core/head_sha",
        reason: "the commit whose changes were classified — the input",
    },
    DigestPath {
        path: "core/base_ref",
        reason: "the base branch selects which policy applied",
    },
    DigestPath {
        path: "core/merge_base_sha",
        reason: "the other end of the diff; a different merge base is a \
                 different set of changes",
    },
    DigestPath {
        path: "core/tier",
        reason: "the verdict itself — the one value the replay corpus exists \
                 to hold still",
    },
    DigestPath {
        path: "core/verdict",
        reason: "derived from the tier, and digested anyway: a bundle whose \
                 verdict disagrees with its tier must not digest equal to one \
                 where they agree",
    },
    DigestPath {
        path: "core/advisory_tier",
        reason: "half of the measurement that distinguishes `nothing failed` \
                 from `nothing that failed counted` — see BundleCore, which \
                 already declares this field to be on the inclusion list",
    },
    DigestPath {
        path: "core/flag_ids",
        reason: "what the classifier found; sorted at construction, so the \
                 order is not incidental",
    },
    DigestPath {
        path: "core/capability_states",
        reason: "the least-confident answering method for each capability. A \
                 lossy collapse over scopes, so it is not the basis for the \
                 tier — the escalations are — but two evaluations that \
                 answered the same questions by different methods are not the \
                 same evaluation",
    },
    // `core/bundle_id` and `core/verdict_digest` are deliberately absent.
    // `verdict_digest` cannot cover itself, and `bundle_id` is derived from
    // this digest, so including either is a fixpoint, not a check.
    //
    // The escalation ledger, minus its free text. `from`/`to` are what makes
    // the ledger replayable to the tier it reports.
    DigestPath {
        path: "adjudication/tier",
        reason: "the enforced ledger's own tier; must agree with `core/tier` \
                 and must digest differently when it does not",
    },
    DigestPath {
        path: "adjudication/verdict",
        reason: "as `adjudication/tier`: derived, duplicated on the wire, and \
                 digested so that a disagreement between the two copies is \
                 visible rather than invisible",
    },
    DigestPath {
        path: "adjudication/escalations/[]/from",
        reason: "the ledger replays to the tier it reports only if the \
                 sequence is intact",
    },
    DigestPath {
        path: "adjudication/escalations/[]/to",
        reason: "the other end of the same step; `from`/`to` together are \
                 what make the ledger replay to the tier it reports",
    },
    DigestPath {
        path: "adjudication/escalations/[]/reason",
        reason: "the stable groupable code — why the verdict is what it is",
    },
    DigestPath {
        path: "adjudication/escalations/[]/evidence",
        reason: "what the escalation points at; an escalation attributed to a \
                 different requirement is a different decision",
    },
    // `adjudication/escalations/[]/detail` is deliberately absent: it is free
    // text for a human, and `ExecutionFailed { detail }` can carry a temporary
    // directory path, which differs between two runs that decided identically.
    DigestPath {
        path: "advisory_escalations/[]/from",
        reason: "the advisory ledger is digested on the same terms as the \
                 enforced one; `core/advisory_tier` is a summary of it and a \
                 summary is not the basis",
    },
    DigestPath {
        path: "advisory_escalations/[]/to",
        reason: "the other end of the same step, on the same terms as the \
                 enforced ledger",
    },
    DigestPath {
        path: "advisory_escalations/[]/reason",
        reason: "the stable groupable code for an outcome that was reported \
                 and not enforced; the escape-rate loop groups on it",
    },
    DigestPath {
        path: "advisory_escalations/[]/evidence",
        reason: "what the advisory escalation points at; re-attributing one \
                 is a different record even at an unchanged tier",
    },
    // `advisory_escalations/[]/detail` is absent for the same reason its
    // enforced counterpart is.
    DigestPath {
        path: "confidence",
        reason: "how much of the picture we had. Digested whole rather than \
                 field by field, because `Confidence` is `#[non_exhaustive]` \
                 and additive by contract: a count added later — `capabilities` \
                 was — enters the digest with the release that adds it rather \
                 than waiting for someone to remember this list. This is the \
                 one place the enumerate-everything rule is relaxed, and it is \
                 safe because the whole struct is counts. It must be covered: \
                 a misspelled `confidence` key deserializes to all-zeros and \
                 lands in `extensions`, rendering as `0 capabilities`, which \
                 reads as `nothing was required`, and that must not digest \
                 equal to a run that really required nothing",
    },
    // `generator/version` and `generator/git_sha` are absent: cutting a release
    // must not invalidate the replay corpus.
    //
    // `generator/registry_digest` is absent from *this* list and present in
    // `bundle_id`: registering an analyzer changes the rules in force, which is
    // a fact about the artifact, but it is not a change to the adjudication
    // that this bundle recorded.
    //
    // `extensions` is absent: it is an untyped bag written by a build this one
    // predates, so its contents cannot be reasoned about, and a float in it
    // cannot be canonicalized at all.
];

/// Paths that belong in [`verdict_digest`] and name fields the bundle does not
/// carry yet. **Not applied.**
///
/// Kept as data rather than as prose so the reasoning survives to whoever adds
/// the evidence section, and kept out of [`VERDICT_DIGEST_PATHS`] so that every
/// live entry stays resolvable and a rename cannot narrow the digest in silence.
/// Moving an entry here into the live list is the deliberate act the inclusion
/// list exists to require.
pub const DEFERRED_VERDICT_DIGEST_PATHS: &[DigestPath] = &[
    DigestPath {
        path: "evidence/[]/provenance/produced_from_commit",
        reason: "the anti-gaming spine: evidence adopted from a different \
                 commit must digest differently from evidence produced for \
                 this one",
    },
    DigestPath {
        path: "evidence/[]/provenance/sha256",
        reason: "identity of the artifact that was parsed",
    },
    DigestPath {
        path: "evidence/[]/provenance/plan_digest",
        reason: "proves the tool ran with retries disabled and a fixed thread \
                 count, rather than merely that it ran",
    },
    DigestPath {
        path: "evidence/[]/provenance/exit_code",
        reason: "what the tool actually reported",
    },
    DigestPath {
        path: "evidence/[]/provenance/toolchain",
        reason: "the same source under a different toolchain is a different \
                 measurement",
    },
    DigestPath {
        path: "evidence/[]/facts",
        reason: "the measurement itself, minus `facts/extra`, which is an \
                 untyped bag and can hold a float",
    },
];

/// Paths [`bundle_id`] must not cover.
///
/// Short by design: `bundle_id` is a content address, so anything not named
/// here participates, including sections this build has never heard of.
pub const BUNDLE_ID_EXCLUDED_PATHS: &[DigestPath] = &[
    DigestPath {
        path: "core/bundle_id",
        reason: "a content address cannot contain itself; the field is filled \
                 in from this function's result",
    },
    // Display timestamps land here, not on an inclusion list: `clock.rs` is the
    // one module allowed to read the wall clock, and every field it produces
    // must be excluded here or two identical evaluations get different bundle
    // identifiers. There is no such field on the bundle today.
];

/// The largest integer ECMAScript represents exactly, and therefore the largest
/// one RFC 8785 can serialize without ambiguity.
const MAX_SAFE_INTEGER: i128 = 9_007_199_254_740_991;

/// A content digest, rendered as `blake3:<64 lowercase hex>`.
///
/// A newtype rather than a `String` so that a digest cannot be confused with any
/// other string at a call site. The frozen wire fields
/// ([`BundleCore::bundle_id`](crate::bundle::BundleCore::bundle_id),
/// [`BundleCore::verdict_digest`](crate::bundle::BundleCore::verdict_digest),
/// [`Generator::registry_digest`](crate::bundle::Generator::registry_digest))
/// stay `String`, because their names and shapes are the permanent contract and
/// this type is not.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize)]
#[serde(transparent)]
pub struct Digest(SmolStr);

impl Digest {
    /// The algorithm prefix every digest carries.
    ///
    /// Present so that changing the algorithm produces values that visibly do
    /// not compare equal to the old ones, rather than bare hex that silently
    /// does not match.
    pub const PREFIX: &'static str = "blake3:";

    /// The number of hexadecimal characters in the digest body.
    const HEX_LEN: usize = 64;

    /// Borrow the rendered form, prefix included.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<Digest> for String {
    fn from(d: Digest) -> Self {
        d.0.into()
    }
}

/// Why a string is not a [`Digest`].
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DigestParseError {
    /// No `blake3:` prefix.
    #[error("digest `{0}` does not start with `{prefix}`", prefix = Digest::PREFIX)]
    Prefix(String),
    /// The body is not 64 lowercase hexadecimal characters.
    #[error(
        "digest `{0}` body is not {n} lowercase hexadecimal characters",
        n = Digest::HEX_LEN
    )]
    Body(String),
}

impl FromStr for Digest {
    type Err = DigestParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some(body) = s.strip_prefix(Self::PREFIX) else {
            return Err(DigestParseError::Prefix(s.to_owned()));
        };
        if body.len() != Self::HEX_LEN
            || !body
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(DigestParseError::Body(s.to_owned()));
        }
        Ok(Digest(SmolStr::new(s)))
    }
}

/// Why a value could not be canonicalized.
///
/// Every variant names the path at which the problem was found, because a
/// document that cannot be canonicalized is a document somebody has to fix, and
/// "there is a float in here somewhere" is not an actionable report.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CanonicalError {
    /// A non-integer number. See the module documentation for why these are
    /// refused rather than formatted.
    #[error(
        "cannot canonicalize the non-integer number at `{path}`: RFC 8785 \
         requires ECMAScript shortest-round-trip formatting, which this build \
         deliberately does not implement. Every typed number in the model is \
         integral; if this came from an untyped bag (`facts.extra`, \
         `extensions`), the producer must write an integer or a string"
    )]
    Float {
        /// Where in the document, as a slash-separated path from the root.
        path: String,
    },
    /// An integer outside `±(2^53 − 1)`.
    #[error(
        "the integer at `{path}` is outside ±(2^53 − 1) and has no exact \
         ECMAScript representation, so two conforming canonicalizers may \
         disagree on how to write it"
    )]
    IntegerNotExact {
        /// Where in the document, as a slash-separated path from the root.
        path: String,
    },
}

/// Why a digest could not be computed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DigestError {
    /// The value would not serialize to JSON at all.
    #[error("could not serialize the bundle to JSON before digesting it: {0}")]
    Serialize(#[from] serde_json::Error),
    /// The serialized value is not canonicalizable.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
}

/// Serialize `value` as RFC 8785 canonical JSON.
///
/// # Errors
///
/// [`CanonicalError`] if the document contains a number this build refuses to
/// write — see the module documentation.
pub fn canonicalize(value: &Value) -> Result<String, CanonicalError> {
    let mut out = String::new();
    let mut path = String::new();
    write_canonical(&mut out, value, &mut path)?;
    Ok(out)
}

/// blake3 over the canonical form of `value`.
///
/// # Errors
///
/// [`CanonicalError`], propagated from [`canonicalize`].
pub fn digest_of(value: &Value) -> Result<Digest, CanonicalError> {
    let canonical = canonicalize(value)?;
    Ok(digest_of_canonical_bytes(canonical.as_bytes()))
}

/// blake3 over bytes that are already canonical.
fn digest_of_canonical_bytes(bytes: &[u8]) -> Digest {
    let hash = blake3::hash(bytes);
    let mut s = String::with_capacity(Digest::PREFIX.len() + Digest::HEX_LEN);
    s.push_str(Digest::PREFIX);
    s.push_str(&hash.to_hex());
    Digest(SmolStr::new(s))
}

/// The digest of the verdict-bearing subtree named by [`VERDICT_DIGEST_PATHS`].
///
/// This is what the replay corpus compares. Two bundles with the same value here
/// decided the same thing about the same input; everything else about them may
/// differ.
///
/// # Errors
///
/// [`DigestError`] if the bundle will not serialize. The included paths are all
/// typed and integral, so [`CanonicalError`] is not reachable through this
/// function today — it is still surfaced rather than swallowed, because the
/// inclusion list is meant to grow.
pub fn verdict_digest(bundle: &EvidenceBundle) -> Result<Digest, DigestError> {
    let whole = serde_json::to_value(bundle)?;
    let projected = project(&whole, VERDICT_DIGEST_PATHS);
    Ok(digest_of(&projected)?)
}

/// The content address of the whole document, minus
/// [`BUNDLE_ID_EXCLUDED_PATHS`].
///
/// Derived from content rather than from a timestamp or a random source, so
/// regenerating the same evaluation yields the same identifier.
///
/// # Errors
///
/// [`DigestError`] if the bundle will not serialize, or if any part of it —
/// including `extensions`, which is untyped — cannot be canonicalized.
pub fn bundle_id(bundle: &EvidenceBundle) -> Result<Digest, DigestError> {
    let mut whole = serde_json::to_value(bundle)?;
    for excluded in BUNDLE_ID_EXCLUDED_PATHS {
        prune(&mut whole, &parse_path(excluded.path));
    }
    Ok(digest_of(&whole)?)
}

// --- path projection --------------------------------------------------------

/// One step of a [`DigestPath`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Segment<'a> {
    /// An object key.
    Key(&'a str),
    /// Every element of an array.
    Each,
}

fn parse_path(path: &str) -> Vec<Segment<'_>> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s == "[]" {
                Segment::Each
            } else {
                Segment::Key(s)
            }
        })
        .collect()
}

/// Build a value containing only the listed paths of `src`.
///
/// A path that matches nothing contributes nothing. That is right for an
/// optional field and wrong for a renamed one, which is why the live list is
/// tested for resolvability rather than trusted.
fn project(src: &Value, paths: &[DigestPath]) -> Value {
    let mut out = Value::Null;
    for p in paths {
        graft(src, &mut out, &parse_path(p.path));
    }
    out
}

fn graft(src: &Value, dst: &mut Value, segments: &[Segment<'_>]) {
    let Some((head, tail)) = segments.split_first() else {
        *dst = src.clone();
        return;
    };
    match *head {
        Segment::Key(key) => {
            let Some(child) = src.get(key) else { return };
            if !dst.is_object() {
                *dst = Value::Object(Map::new());
            }
            let Some(map) = dst.as_object_mut() else {
                return;
            };
            let slot = map.entry(key.to_owned()).or_insert(Value::Null);
            graft(child, slot, tail);
        }
        Segment::Each => {
            let Some(items) = src.as_array() else { return };
            if !dst.is_array() {
                *dst = Value::Array(Vec::new());
            }
            let Some(list) = dst.as_array_mut() else {
                return;
            };
            list.resize(items.len(), Value::Null);
            for (item, slot) in items.iter().zip(list.iter_mut()) {
                graft(item, slot, tail);
            }
        }
    }
}

/// Remove one path from `value`, wherever `[]` takes it.
fn prune(value: &mut Value, segments: &[Segment<'_>]) {
    let Some((head, tail)) = segments.split_first() else {
        return;
    };
    match *head {
        Segment::Key(key) => {
            if tail.is_empty() {
                if let Some(map) = value.as_object_mut() {
                    map.remove(key);
                }
            } else if let Some(child) = value.get_mut(key) {
                prune(child, tail);
            }
        }
        Segment::Each => {
            if let Some(items) = value.as_array_mut() {
                for item in items {
                    prune(item, tail);
                }
            }
        }
    }
}

// --- RFC 8785 ---------------------------------------------------------------

fn write_canonical(
    out: &mut String,
    value: &Value,
    path: &mut String,
) -> Result<(), CanonicalError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => write_number(out, n, path)?,
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let len = path.len();
                path.push('/');
                // Indices are written straight in; this is a diagnostic path,
                // not a `DigestPath`.
                path.push_str(&i.to_string());
                write_canonical(out, item, path)?;
                path.truncate(len);
            }
            out.push(']');
        }
        Value::Object(map) => {
            // The whole reason this module exists rather than calling
            // `serde_json::to_string`: JCS orders keys by UTF-16 code unit, and
            // Rust's `str` ordering is by UTF-8 byte, which is code-point order.
            // They agree across the BMP and disagree above it.
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                let len = path.len();
                path.push('/');
                path.push_str(key);
                let Some(child) = map.get(key) else { continue };
                write_canonical(out, child, path)?;
                path.truncate(len);
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_number(
    out: &mut String,
    n: &serde_json::Number,
    path: &str,
) -> Result<(), CanonicalError> {
    let as_int: i128 = if let Some(u) = n.as_u64() {
        i128::from(u)
    } else if let Some(i) = n.as_i64() {
        i128::from(i)
    } else {
        return Err(CanonicalError::Float {
            path: root_or(path),
        });
    };
    if as_int.abs() > MAX_SAFE_INTEGER {
        return Err(CanonicalError::IntegerNotExact {
            path: root_or(path),
        });
    }
    // Within the safe range, ECMAScript's `Number::toString` and Rust's integer
    // `Display` agree exactly: no exponent, no sign on zero, no trailing zeros.
    out.push_str(&as_int.to_string());
    Ok(())
}

fn root_or(path: &str) -> String {
    if path.is_empty() {
        "/".to_owned()
    } else {
        path.to_owned()
    }
}

/// Write a JSON string literal the way ECMAScript's `JSON.stringify` does.
///
/// Written out rather than delegated to `serde_json` so the JCS claim does not
/// depend on another crate's escaping staying what it is today. Non-ASCII is
/// emitted as UTF-8, not escaped; only `"`, `\`, and the C0 controls are
/// escaped, the six with short forms by their short forms and the rest as
/// lowercase `\u00xx`.
fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{09}' => out.push_str("\\t"),
            '\u{0a}' => out.push_str("\\n"),
            '\u{0c}' => out.push_str("\\f"),
            '\u{0d}' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u");
                let mut buf = [0u8; 4];
                let hex = "0123456789abcdef".as_bytes();
                let v = c as u32;
                buf[0] = hex[((v >> 12) & 0xf) as usize];
                buf[1] = hex[((v >> 8) & 0xf) as usize];
                buf[2] = hex[((v >> 4) & 0xf) as usize];
                buf[3] = hex[(v & 0xf) as usize];
                for b in buf {
                    out.push(char::from(b));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// --- what the list is checked against ---------------------------------------

/// Whether `path` resolves to at least one value in `value`.
///
/// Used by the tests that keep [`VERDICT_DIGEST_PATHS`] honest. Public because
/// the replay corpus, when it exists, needs to make the same assertion against
/// a bundle it did not construct.
#[must_use]
pub fn path_resolves(value: &Value, path: &str) -> bool {
    fn walk(v: &Value, segments: &[Segment<'_>]) -> bool {
        let Some((head, tail)) = segments.split_first() else {
            return true;
        };
        match *head {
            Segment::Key(key) => v.get(key).is_some_and(|child| walk(child, tail)),
            Segment::Each => v
                .as_array()
                .is_some_and(|items| !items.is_empty() && items.iter().all(|i| walk(i, tail))),
        }
    }
    walk(value, &parse_path(path))
}

/// Assert-friendly view of what the two lists cover, for tests and for the
/// `explain` rendering that will need it.
#[must_use]
pub fn verdict_digest_covers(path: &str) -> Option<&'static str> {
    VERDICT_DIGEST_PATHS
        .iter()
        .find(|p| p.path == path)
        .map(|p| p.reason)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;
    use serde_json::json;

    use super::*;
    use crate::adjudicate::{Adjudicators, Enforcement};
    use crate::bundle::{BundleCore, Confidence, Generator};
    use crate::ids::{CapabilityId, RiskFlagId};
    use crate::location::{LineRange, Location};
    use crate::reason::{EvidenceRef, ReasonCode};
    use crate::resolution::ResolutionState;
    use crate::schema::SchemaVersion;
    use crate::tier::Tier;

    /// A bundle in which every path on [`VERDICT_DIGEST_PATHS`] is populated,
    /// including the optional ones, so that a path finding nothing is a
    /// rename and not a `None`.
    ///
    /// `EvidenceRef` is restricted to the unit and struct-shaped variants
    /// here. The enum is internally tagged today, which cannot serialize a
    /// newtype variant over a plain string, so `Requirement`, `Capability`,
    /// `Crate`, `Rule`, and `Path` do not currently round-trip. That is #78's
    /// to fix, and it is also why no golden digest is committed with this
    /// change: a digest taken over the pre-#78 wire form would be invalidated
    /// by a fix that has nothing to do with adjudication.
    fn bundle() -> EvidenceBundle {
        let mut adjudicators = Adjudicators::new();
        adjudicators.route(Enforcement::Enforcing).escalate(
            Tier::T1,
            ReasonCode::RuleTierAtLeast,
            "rule `core-unsafe` requires T1",
            EvidenceRef::Flag {
                flag: RiskFlagId::new("unsafe"),
                locations: vec![
                    Location::file("crates/core/src/ring.rs")
                        .at_lines(LineRange::single(88))
                        .at_blob("0f22")
                        .in_item("kono_core::ring::Ring::push_unchecked"),
                ],
            },
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
                git_sha: Some("cafe".into()),
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

    // --- RFC 8785 ----------------------------------------------------------

    #[test]
    fn jcs_orders_keys_by_utf16_code_unit_not_by_code_point() {
        // The case that earns the "JCS" claim, and the only one where UTF-8
        // byte order — which is what `BTreeMap<String, _>` and
        // `serde_json::to_string` give you — is *wrong*.
        //
        // U+E000 is in the BMP: one UTF-16 code unit, 0xE000.
        // U+10000 is supplementary: the surrogate pair 0xD800 0xDC00.
        //
        // By code point (and therefore by UTF-8 byte): E000 < 10000.
        // By UTF-16 code unit:                         D800 < E000, so the
        // supplementary key sorts FIRST.
        let value = json!({ "\u{e000}": 1, "\u{10000}": 2 });

        assert_eq!(
            canonicalize(&value).expect("canonicalize"),
            "{\"\u{10000}\":2,\"\u{e000}\":1}",
            "JCS sorts by UTF-16 code unit, so the supplementary character \
             leads a private-use BMP character"
        );

        // And the proof that this is not what the obvious implementation does.
        let naive = serde_json::to_string(&value).expect("serialize");
        assert_ne!(
            naive,
            canonicalize(&value).expect("canonicalize"),
            "if these ever agree, this test has stopped testing anything — \
             pick a different pair of keys"
        );
    }

    #[test]
    fn jcs_orders_bmp_keys_the_ordinary_way() {
        let value = json!({ "b": 1, "A": 2, "a": 3, "\u{00e9}": 4 });
        assert_eq!(
            canonicalize(&value).expect("canonicalize"),
            "{\"A\":2,\"a\":3,\"b\":1,\"\u{00e9}\":4}"
        );
    }

    #[test]
    fn jcs_emits_no_whitespace_and_preserves_array_order() {
        let value: Value =
            serde_json::from_str("  { \"z\" : [ 3 , 1 , 2 ] ,\n \"a\" : null }  ").expect("parse");
        assert_eq!(
            canonicalize(&value).expect("canonicalize"),
            "{\"a\":null,\"z\":[3,1,2]}"
        );
    }

    #[test]
    fn jcs_escapes_the_way_json_stringify_does() {
        let value = json!({ "k": "a\"b\\c\u{08}\u{09}\u{0a}\u{0c}\u{0d}\u{01}é/" });
        assert_eq!(
            canonicalize(&value).expect("canonicalize"),
            "{\"k\":\"a\\\"b\\\\c\\b\\t\\n\\f\\r\\u0001é/\"}",
            "short forms for the six, lowercase \\u00xx for the rest, and \
             neither `/` nor non-ASCII is escaped"
        );
    }

    #[test]
    fn a_float_is_refused_with_its_path() {
        let err = canonicalize(&json!({ "a": { "b": [0, 1.5] } })).expect_err("must refuse");
        assert_eq!(
            err,
            CanonicalError::Float {
                path: "/a/b/1".to_owned()
            }
        );
    }

    #[test]
    fn an_integer_beyond_the_ecmascript_safe_range_is_refused() {
        let err =
            canonicalize(&json!({ "n": 9_007_199_254_740_992_u64 })).expect_err("must refuse");
        assert!(matches!(err, CanonicalError::IntegerNotExact { .. }));
        assert!(canonicalize(&json!({ "n": 9_007_199_254_740_991_u64 })).is_ok());
    }

    #[test]
    fn a_float_in_extensions_fails_bundle_id_rather_than_digesting_silently() {
        // `extensions` is untyped and reaches `bundle_id`. The failure has to
        // be loud: a canonicalizer that quietly formatted this would produce
        // an identifier another implementation disagrees with.
        let mut b = bundle();
        b.extensions
            .insert("perf".into(), json!({ "p99_ms": 12.5 }));
        assert!(matches!(
            bundle_id(&b),
            Err(DigestError::Canonical(CanonicalError::Float { .. }))
        ));
        assert!(
            verdict_digest(&b).is_ok(),
            "`extensions` is off the verdict inclusion list, so it cannot \
             break a verdict digest either"
        );
    }

    // --- the inclusion list ------------------------------------------------

    #[test]
    fn every_live_inclusion_path_resolves() {
        // The guard the module documentation describes. A path that matches
        // nothing narrows the digest in silence, and at run time a renamed
        // field is indistinguishable from an absent optional one — so the list
        // is checked against a bundle in which every optional field is
        // populated.
        let value = serde_json::to_value(bundle()).expect("serialize");
        for p in VERDICT_DIGEST_PATHS {
            assert!(
                path_resolves(&value, p.path),
                "`{}` is on the verdict inclusion list and resolves to nothing \
                 in a fully populated bundle. Either the field was renamed — in \
                 which case the digest just stopped covering it — or the entry \
                 belongs in DEFERRED_VERDICT_DIGEST_PATHS.",
                p.path
            );
        }
    }

    #[test]
    fn no_deferred_path_resolves_yet() {
        // The other half: an entry graduates out of the deferred list when the
        // field exists, and this test is what notices that it does.
        let value = serde_json::to_value(bundle()).expect("serialize");
        for p in DEFERRED_VERDICT_DIGEST_PATHS {
            assert!(
                !path_resolves(&value, p.path),
                "`{}` now exists on the bundle. Move it from \
                 DEFERRED_VERDICT_DIGEST_PATHS into VERDICT_DIGEST_PATHS, \
                 which is the deliberate act the inclusion list exists to \
                 require.",
                p.path
            );
        }
    }

    #[test]
    fn every_path_entry_carries_a_reason_and_appears_once() {
        // The reason is the entry's justification and the only thing that
        // makes the list auditable; an entry nobody could explain is an entry
        // somebody added to make a test go green. A duplicate path is worse
        // than useless: it grafts the same subtree twice and reads as two
        // independent decisions.
        let mut seen = std::collections::BTreeSet::new();
        for p in VERDICT_DIGEST_PATHS
            .iter()
            .chain(DEFERRED_VERDICT_DIGEST_PATHS)
        {
            assert!(!p.path.is_empty());
            assert!(
                p.reason.len() > 20,
                "`{}` has no real reason attached",
                p.path
            );
            assert!(
                seen.insert(p.path),
                "`{}` appears twice across the live and deferred lists",
                p.path
            );
        }
        let mut excluded = std::collections::BTreeSet::new();
        for p in BUNDLE_ID_EXCLUDED_PATHS {
            assert!(p.reason.len() > 20, "`{}` has no reason", p.path);
            assert!(excluded.insert(p.path), "`{}` appears twice", p.path);
        }
    }

    #[test]
    fn the_two_self_referential_core_fields_are_off_the_verdict_list() {
        assert!(verdict_digest_covers("core/verdict_digest").is_none());
        assert!(verdict_digest_covers("core/bundle_id").is_none());
        assert!(verdict_digest_covers("core/tier").is_some());
    }

    // --- what each digest is sensitive to ----------------------------------

    #[test]
    fn the_verdict_digest_is_stable_across_runs() {
        let a = verdict_digest(&bundle()).expect("digest");
        let b = verdict_digest(&bundle()).expect("digest");
        assert_eq!(a, b);
        assert!(a.as_str().starts_with(Digest::PREFIX));
        assert_eq!(a.as_str().len(), Digest::PREFIX.len() + 64);
    }

    #[test]
    fn changing_an_included_field_changes_the_verdict_digest() {
        let base = verdict_digest(&bundle()).expect("digest");

        let mut b = bundle();
        b.core.tier = Tier::TOP;
        assert_ne!(verdict_digest(&b).expect("digest"), base, "core/tier");

        let mut b = bundle();
        b.confidence.unverified += 1;
        assert_ne!(verdict_digest(&b).expect("digest"), base, "confidence");

        let mut b = bundle();
        b.adjudication.escalations[0].reason = ReasonCode::CapabilityUnverified;
        assert_ne!(
            verdict_digest(&b).expect("digest"),
            base,
            "escalations[].reason"
        );

        let mut b = bundle();
        b.adjudication.escalations[0].evidence = EvidenceRef::Unattributed;
        assert_ne!(
            verdict_digest(&b).expect("digest"),
            base,
            "escalations[].evidence"
        );

        let mut b = bundle();
        b.advisory_escalations[0].from = Tier::T2;
        assert_ne!(
            verdict_digest(&b).expect("digest"),
            base,
            "advisory_escalations[].to"
        );

        let mut b = bundle();
        b.core
            .capability_states
            .insert(CapabilityId::new("mutants"), ResolutionState::Unverified);
        assert_ne!(
            verdict_digest(&b).expect("digest"),
            base,
            "core/capability_states"
        );
    }

    #[test]
    fn changing_an_excluded_field_does_not_change_the_verdict_digest() {
        let base = verdict_digest(&bundle()).expect("digest");

        let mut b = bundle();
        b.generator.version = "9.9.9".into();
        b.generator.git_sha = Some("deadbeef".into());
        assert_eq!(
            verdict_digest(&b).expect("digest"),
            base,
            "cutting a release must not invalidate the replay corpus"
        );

        let mut b = bundle();
        b.generator.registry_digest = "blake3:ffff".into();
        assert_eq!(
            verdict_digest(&b).expect("digest"),
            base,
            "registering an analyzer is not a change to this adjudication"
        );

        let mut b = bundle();
        b.adjudication.escalations[0].detail = "/tmp/.tmpXYZ/target/nextest".into();
        assert_eq!(
            verdict_digest(&b).expect("digest"),
            base,
            "free text can carry a tempdir path that differs between two runs \
             that decided identically"
        );

        let mut b = bundle();
        b.core.verdict_digest = "blake3:whatever".into();
        b.core.bundle_id = "vc_other".into();
        assert_eq!(
            verdict_digest(&b).expect("digest"),
            base,
            "a digest cannot cover itself"
        );

        let mut b = bundle();
        b.extensions
            .insert("perf".into(), json!({ "benchmarks": [] }));
        assert_eq!(verdict_digest(&b).expect("digest"), base, "extensions");
    }

    #[test]
    fn the_bundle_id_covers_what_the_verdict_digest_deliberately_drops() {
        let base = bundle_id(&bundle()).expect("digest");

        let mut b = bundle();
        b.generator.version = "9.9.9".into();
        assert_ne!(bundle_id(&b).expect("digest"), base, "generator/version");

        let mut b = bundle();
        b.generator.registry_digest = "blake3:ffff".into();
        assert_ne!(
            bundle_id(&b).expect("digest"),
            base,
            "generator/registry_digest"
        );

        let mut b = bundle();
        b.extensions
            .insert("perf".into(), json!({ "benchmarks": [] }));
        assert_ne!(bundle_id(&b).expect("digest"), base, "extensions");

        let mut b = bundle();
        b.adjudication.escalations[0].detail = "something else".into();
        assert_ne!(bundle_id(&b).expect("digest"), base, "escalations[].detail");
    }

    #[test]
    fn the_bundle_id_ignores_only_itself() {
        let base = bundle_id(&bundle()).expect("digest");
        let mut b = bundle();
        b.core.bundle_id = "anything at all".into();
        assert_eq!(
            bundle_id(&b).expect("digest"),
            base,
            "a content address cannot contain itself"
        );

        let mut b = bundle();
        b.core.verdict_digest = "blake3:ffff".into();
        assert_ne!(
            bundle_id(&b).expect("digest"),
            base,
            "the verdict digest is content, and content is what bundle_id \
             addresses"
        );
    }

    #[test]
    fn the_two_digests_are_not_the_same_value() {
        let b = bundle();
        assert_ne!(
            verdict_digest(&b).expect("digest").as_str(),
            bundle_id(&b).expect("digest").as_str()
        );
    }

    // --- Digest ------------------------------------------------------------

    #[test]
    fn a_digest_round_trips_through_its_rendered_form() {
        let d = verdict_digest(&bundle()).expect("digest");
        let parsed: Digest = d.as_str().parse().expect("parse");
        assert_eq!(parsed, d);
        assert_eq!(String::from(d.clone()), d.to_string());
    }

    #[test]
    fn a_digest_that_is_not_one_is_refused() {
        assert!("7c1e".parse::<Digest>().is_err());
        assert!("blake3:7c1e".parse::<Digest>().is_err());
        assert!(
            format!("blake3:{}", "A".repeat(64))
                .parse::<Digest>()
                .is_err(),
            "uppercase hex is a different string and must not compare equal to \
             the lowercase form by accident"
        );
        assert!(
            format!("blake3:{}", "0".repeat(64))
                .parse::<Digest>()
                .is_ok()
        );
    }

    // --- properties --------------------------------------------------------

    /// Write an object out as JSON text with the keys in the order given,
    /// rather than in whatever order a map would have imposed.
    fn render<'a>(entries: impl Iterator<Item = (&'a str, i64)>) -> String {
        let body: Vec<String> = entries.map(|(k, v)| format!("  \"{k}\" : {v}")).collect();
        format!("{{\n{}\n}}", body.join(",\n"))
    }

    /// A JSON value with no floats, so the generator does not spend its time
    /// producing documents the canonicalizer is meant to refuse.
    fn arb_json() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            (-1000i64..1000).prop_map(|n| json!(n)),
            "[\\PC]{0,8}".prop_map(Value::String),
        ];
        leaf.prop_recursive(4, 32, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
                prop::collection::vec(("[\\PC]{1,6}", inner), 0..4)
                    .prop_map(|entries| { Value::Object(entries.into_iter().collect()) }),
            ]
        })
    }

    proptest! {
        #[test]
        fn canonicalizing_is_idempotent_and_reparses_to_the_same_value(v in arb_json()) {
            let once = canonicalize(&v).expect("canonicalize");
            let reparsed: Value = serde_json::from_str(&once).expect("canonical JSON parses");
            prop_assert_eq!(canonicalize(&reparsed).expect("canonicalize"), once);
        }

        #[test]
        fn the_digest_does_not_depend_on_how_the_text_was_written(v in arb_json()) {
            // Whitespace and key order in the *source text* are exactly what
            // canonicalization is for: a bundle re-serialized by a different
            // writer must digest the same.
            let compact = serde_json::to_string(&v).expect("serialize");
            let pretty = serde_json::to_string_pretty(&v).expect("serialize");
            let a: Value = serde_json::from_str(&compact).expect("parse");
            let b: Value = serde_json::from_str(&pretty).expect("parse");
            prop_assert_eq!(digest_of(&a).expect("digest"), digest_of(&b).expect("digest"));
        }

        #[test]
        fn the_key_order_of_the_source_text_does_not_reach_the_digest(
            entries in prop::collection::btree_map("[a-z]{1,4}", -50i64..50, 1..8)
        ) {
            // What the escape ledger depends on: `git notes merge -s
            // cat_sort_uniq` de-duplicates *lines*, so two writers that agree
            // on the facts must emit byte-identical text. Here the same object
            // is written out with its keys in ascending and descending order
            // and must digest the same.
            //
            // `serde_json::Map` is a `BTreeMap` in this build, so a parse
            // already normalizes key order and this property is partly held by
            // construction. It is asserted anyway because that is a build
            // configuration, not a contract — enabling `preserve_order` would
            // retract it silently, and this test is what would notice.
            let ascending = render(entries.iter().map(|(k, v)| (k.as_str(), *v)));
            let descending = render(entries.iter().rev().map(|(k, v)| (k.as_str(), *v)));
            prop_assume!(entries.len() < 2 || ascending != descending);

            let a: Value = serde_json::from_str(&ascending).expect("parse");
            let b: Value = serde_json::from_str(&descending).expect("parse");
            prop_assert_eq!(digest_of(&a).expect("digest"), digest_of(&b).expect("digest"));
        }

        #[test]
        fn projection_never_invents_a_path(v in arb_json()) {
            // Whatever the projection produces, every path it produced was in
            // the source. This is the property that makes the inclusion list a
            // restriction rather than a transformation.
            let projected = project(&v, VERDICT_DIGEST_PATHS);
            for p in VERDICT_DIGEST_PATHS {
                if path_resolves(&projected, p.path) {
                    prop_assert!(path_resolves(&v, p.path), "{}", p.path);
                }
            }
        }
    }
}
