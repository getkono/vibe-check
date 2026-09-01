//! What a requirement is *about*: the crates and paths it covers.
//!
//! Requirements, not capabilities, are the unit of resolution, and a
//! requirement is a *(capability × scope)* pair. This module owns the scope
//! half and — more importantly — owns the encoding that
//! [`RequirementId::derive`](crate::ids::RequirementId::derive) digests.
//!
//! # Why the encoding is the security-relevant part
//!
//! [`Resolutions`](crate::resolution::Resolutions) is keyed by
//! [`RequirementId`](crate::ids::RequirementId). Two scopes that encode to the
//! same bytes get the same identifier, and one resolution then displaces the
//! other in that map. If the displaced one was the failing one, the run reports
//! a pass it never measured. So this file's obligation is not "produce a
//! reasonable string" — it is **injectivity**: distinct scopes must produce
//! distinct bytes, with no exceptions and no reliance on the caller.
//!
//! Injectivity is bought with three decisions, each of which is load-bearing:
//!
//! 1. **Two separators, not one.** `\u{1f}` (unit separator) between the
//!    members of one set, `\u{1e}` (record separator) between the two sets —
//!    the convention `ProcessPlan::digest_input` already established, so the
//!    workspace has one convention rather than two. One separator would make
//!    `crates = {a, b}, paths = {}` and `crates = {a}, paths = {b}` encode
//!    identically, which is exactly the collision this type exists to prevent.
//! 2. **The separators cannot occur in the data.** A separator convention is
//!    injective only if the data is separator-free, so [`RequirementScope::new`]
//!    rejects every ASCII control character in a crate identifier or a path.
//!    That is not defensive tidying; without it the convention proves nothing.
//! 3. **Empty members are rejected.** Otherwise `{}` and `{""}` both encode to
//!    the empty string, and the distinction between "no crates" and "one crate
//!    with a blank name" is lost.
//!
//! `ProcessPlan::digest_input` is cited above as the source of the *convention*,
//! not as a model of correctness: it joins `env.inherit` with `,` and
//! interpolates several fields with `{:?}`, so it is not itself injective. This
//! module is held to the stronger standard because a collision here silently
//! answers a question nobody asked.
//!
//! # Why paths are rejected rather than normalized
//!
//! `foo/../bar` could be rewritten to `bar`, and that rewrite would be wrong
//! often enough to matter: `..` traverses a symlink differently from how a
//! lexical normalizer does, so the rewritten path may not name the file the
//! author meant. A scope that quietly means something other than what it says
//! is a scope nobody can audit, and a scope nobody can audit is one that can be
//! widened by a typo. Rejecting costs a policy author one clear error message;
//! normalizing costs the reader their ability to check.

use std::collections::BTreeSet;
use std::fmt;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

use crate::ids::CrateId;

/// Separates the members of one set in the canonical encoding.
pub(crate) const UNIT_SEPARATOR: u8 = 0x1f;

/// Separates one set from the next in the canonical encoding.
pub(crate) const RECORD_SEPARATOR: u8 = 0x1e;

/// Why a scope was rejected.
///
/// Each variant names the rule that failed and echoes the offending value,
/// because the value usually came from a policy document somebody wrote by
/// hand and "which entry, and why" is the difference between an error that gets
/// fixed and one that gets worked around.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScopeError {
    /// A crate identifier was empty.
    #[error("a scope's crate identifier must not be empty")]
    EmptyCrate,

    /// A path was empty.
    #[error("a scope's path must not be empty")]
    EmptyPath,

    /// A crate identifier or a path contained an ASCII control character.
    ///
    /// Rejected because the canonical encoding separates members with `\u{1f}`
    /// and sets with `\u{1e}`; a member containing either could impersonate a
    /// boundary, and the encoding would no longer be injective.
    #[error(
        "a scope member must not contain control characters, \
         found {found:?} at byte {at} of {text:?}"
    )]
    ControlCharacter {
        /// The offending member, echoed whole.
        text: String,
        /// Byte offset of the offending character.
        at: usize,
        /// The offending character.
        found: char,
    },

    /// A path began with `./`.
    #[error("a scope path must not begin with `./`, found {path:?}; write it as {suggestion:?}")]
    LeadingCurrentDirectory {
        /// The offending path.
        path: Utf8PathBuf,
        /// The same path with the prefix removed.
        suggestion: String,
    },

    /// A path ended with `/`.
    #[error("a scope path must not end with `/`, found {path:?}")]
    TrailingSlash {
        /// The offending path.
        path: Utf8PathBuf,
    },

    /// A path was not a repository-relative path in normal form: it was
    /// absolute, or contained `.`, `..`, or an empty component.
    #[error(
        "a scope path must be repository-relative and already normalized — \
         no `.`, no `..`, no `//`, no leading `/` — found {path:?}"
    )]
    NotNormalized {
        /// The offending path.
        path: Utf8PathBuf,
    },
}

/// A requirement's scope after the monorepo union.
///
/// Sorted and deduplicated by construction, because the identifier derived from
/// it must not depend on the order the planner happened to union things in. Two
/// planners that discovered the same crates in different orders are describing
/// the same requirement, and they must reach the same
/// [`RequirementId`](crate::ids::RequirementId) or the same work is scheduled
/// twice under two names.
///
/// # Construction
///
/// [`new`](Self::new) is the only way in for a scope that narrows anything, and
/// it is fallible. There is no `From` and no `Default`: the canonical
/// encoding's injectivity depends on invariants only a checking constructor can
/// establish (see the module documentation), and a *derived* `Default` would
/// hand out the widest scope there is under the name that reads as harmless.
/// [`everything`](Self::everything) is that value, spelled so a reader sees it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RequirementScope {
    /// Crates and pseudo-crates. Sorted and deduplicated by construction.
    crates: BTreeSet<CrateId>,
    /// Repository-relative paths, already in normal form. Sorted and
    /// deduplicated by construction.
    paths: BTreeSet<Utf8PathBuf>,
}

impl RequirementScope {
    /// Build a scope, rejecting anything the canonical encoding could not
    /// represent unambiguously.
    ///
    /// # Errors
    ///
    /// Returns the [`ScopeError`] naming the first rule that failed. Crates are
    /// checked before paths, and within each the iteration order of the
    /// argument decides which failure is reported first — the caller is
    /// expected to fix all of them, so which one is named first is a matter of
    /// message quality rather than of behaviour.
    pub fn new<C, P>(crates: C, paths: P) -> Result<Self, ScopeError>
    where
        C: IntoIterator<Item = CrateId>,
        P: IntoIterator,
        P::Item: Into<Utf8PathBuf>,
    {
        let mut checked_crates = BTreeSet::new();
        for id in crates {
            if id.as_str().is_empty() {
                return Err(ScopeError::EmptyCrate);
            }
            reject_control_characters(id.as_str())?;
            checked_crates.insert(id);
        }

        let mut checked_paths = BTreeSet::new();
        for path in paths {
            checked_paths.insert(check_path(path.into())?);
        }

        Ok(Self {
            crates: checked_crates,
            paths: checked_paths,
        })
    }

    /// An empty scope: the whole repository, narrowed by nothing.
    ///
    /// Spelled out rather than reached through `new([], [])` so that the
    /// "narrowed by nothing" case is legible at the call site, and so that the
    /// caller does not have to name two empty iterators' element types.
    #[must_use]
    pub fn everything() -> Self {
        Self {
            crates: BTreeSet::new(),
            paths: BTreeSet::new(),
        }
    }

    /// The crates in this scope, ascending.
    pub fn crates(&self) -> impl ExactSizeIterator<Item = &CrateId> {
        self.crates.iter()
    }

    /// The paths in this scope, ascending in `Utf8Path` order.
    ///
    /// Which is *not* the order [`canonical_bytes`](Self::canonical_bytes)
    /// encodes them in, and deliberately so — see that method. Nothing that
    /// feeds a digest may read this.
    pub fn paths(&self) -> impl ExactSizeIterator<Item = &Utf8Path> {
        self.paths.iter().map(Utf8PathBuf::as_path)
    }

    /// Whether this scope narrows anything at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.crates.is_empty() && self.paths.is_empty()
    }

    /// The injective encoding a requirement identifier is derived from.
    ///
    /// `crates` joined by `\u{1f}`, then `\u{1e}`, then `paths` joined by
    /// `\u{1f}`. Because [`new`](Self::new) has already excluded both
    /// separators and the empty string from every member, the last `\u{1e}` in
    /// the output is always the set boundary and every `\u{1f}` is always a
    /// member boundary — so the encoding can be read back and is therefore
    /// injective.
    ///
    /// # Why the members are re-sorted here
    ///
    /// By their UTF-8 bytes, not by the order they sit in the sets. For
    /// [`CrateId`] the two agree, but `Utf8PathBuf`'s [`Ord`] is *component-wise*
    /// — `f/a` sorts before `f-`, because it compares `f` against `f-` and only
    /// then `a` against nothing. That order is perfectly deterministic, and it
    /// belongs to camino rather than to us. A camino release that refined it
    /// would move every derived identifier in the workspace, silently and with
    /// a green build, and every historical escalation reference with it. So the
    /// digest reads a rule this file states, and the collections keep whichever
    /// order suits them.
    ///
    /// `pub(crate)` on purpose. This is an input to a digest, not a
    /// serialization format: publishing it would invite a second consumer, and
    /// a second consumer is what turns "we can change this" into "changing this
    /// breaks someone".
    pub(crate) fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        join_into(&mut out, self.crates.iter().map(CrateId::as_str));
        out.push(RECORD_SEPARATOR);
        join_into(&mut out, self.paths.iter().map(|path| path.as_str()));
        out
    }
}

impl fmt::Display for RequirementScope {
    /// A human-readable rendering for diagnostics — *not* the digest input.
    ///
    /// Deliberately lossy where the encoding is not: it uses a comma, which a
    /// crate name could contain. Anything that needs to tell two scopes apart
    /// must use the identifier, not this.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("<whole repository>");
        }
        let mut first = true;
        for id in &self.crates {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            write!(f, "{id}")?;
        }
        for path in &self.paths {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            write!(f, "{path}")?;
        }
        Ok(())
    }
}

/// Append `members` to `out`, sorted by their bytes and separated by
/// [`UNIT_SEPARATOR`].
///
/// The sort is here rather than inherited from the caller's collection so that
/// the digest input depends on a rule this file states — see
/// [`RequirementScope::canonical_bytes`].
fn join_into<'a>(out: &mut Vec<u8>, members: impl Iterator<Item = &'a str>) {
    let mut members: Vec<&str> = members.collect();
    members.sort_unstable_by_key(|member| member.as_bytes());
    for (index, member) in members.into_iter().enumerate() {
        if index > 0 {
            out.push(UNIT_SEPARATOR);
        }
        out.extend_from_slice(member.as_bytes());
    }
}

/// Reject any ASCII control character, naming where it was found.
///
/// # Errors
///
/// [`ScopeError::ControlCharacter`] on the first one.
fn reject_control_characters(text: &str) -> Result<(), ScopeError> {
    for (at, found) in text.char_indices() {
        if found.is_control() {
            return Err(ScopeError::ControlCharacter {
                text: text.to_owned(),
                at,
                found,
            });
        }
    }
    Ok(())
}

/// Check one path, returning it unchanged when it is already in normal form.
///
/// Nothing is rewritten. The value that comes back is the value that went in,
/// or an error — see the module documentation for why normalizing would be the
/// wrong favour to do a policy author.
///
/// # Errors
///
/// The [`ScopeError`] naming the first rule that failed.
fn check_path(path: Utf8PathBuf) -> Result<Utf8PathBuf, ScopeError> {
    let raw = path.as_str();
    if raw.is_empty() {
        return Err(ScopeError::EmptyPath);
    }
    reject_control_characters(raw)?;
    if let Some(rest) = raw.strip_prefix("./") {
        return Err(ScopeError::LeadingCurrentDirectory {
            suggestion: rest.to_owned(),
            path,
        });
    }
    if raw.ends_with('/') {
        return Err(ScopeError::TrailingSlash { path });
    }

    // Re-spelling the path from its own components catches everything left in
    // one comparison: a `.` or `..` component, a `//` that `components()`
    // collapses, a leading `/`, and a Windows-style prefix. If the components
    // do not spell the input back exactly, the input was not in normal form —
    // whatever the reason.
    let mut normalized = Utf8PathBuf::new();
    for component in Utf8Path::new(raw).components() {
        match component {
            Utf8Component::Normal(part) => normalized.push(part),
            _ => return Err(ScopeError::NotNormalized { path }),
        }
    }
    if normalized.as_str() != raw {
        return Err(ScopeError::NotNormalized { path });
    }

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Build a scope from string slices, for readable fixtures.
    fn scope(crates: &[&str], paths: &[&str]) -> Result<RequirementScope, ScopeError> {
        RequirementScope::new(
            crates.iter().map(CrateId::new).collect::<Vec<_>>(),
            paths.to_vec(),
        )
    }

    #[test]
    fn accepts_the_scopes_a_policy_writes() {
        let scope = scope(
            &["kono-core", "@workspace"],
            &["crates/kono-core/src/lib.rs", "ci"],
        )
        .expect("a well-formed scope");
        assert_eq!(
            scope.crates().map(CrateId::as_str).collect::<Vec<_>>(),
            ["@workspace", "kono-core"],
            "sorted by construction, not by the order they were unioned in"
        );
        assert_eq!(
            scope.paths().map(Utf8Path::as_str).collect::<Vec<_>>(),
            ["ci", "crates/kono-core/src/lib.rs"]
        );
        assert!(!scope.is_empty());
    }

    #[test]
    fn the_empty_scope_is_the_whole_repository() {
        let everything = RequirementScope::everything();
        assert!(everything.is_empty());
        assert_eq!(everything.to_string(), "<whole repository>");
        assert_eq!(
            everything.canonical_bytes(),
            vec![RECORD_SEPARATOR],
            "one boundary and nothing either side of it"
        );
    }

    #[test]
    fn a_traversing_path_is_an_error_not_a_rewrite() {
        // The claim in the issue, asserted directly: `foo/../bar` must not be
        // silently accepted, and must not silently become `bar`.
        let error = scope(&[], &["foo/../bar"]).expect_err("must be rejected");
        assert_eq!(
            error,
            ScopeError::NotNormalized {
                path: "foo/../bar".into()
            }
        );
    }

    #[test]
    fn each_rejected_shape_names_its_own_rule() {
        assert_eq!(
            scope(&[], &[""]).expect_err("empty path"),
            ScopeError::EmptyPath
        );
        assert_eq!(
            scope(&[], &["./src"]).expect_err("leading ./"),
            ScopeError::LeadingCurrentDirectory {
                path: "./src".into(),
                suggestion: "src".to_owned(),
            }
        );
        assert_eq!(
            scope(&[], &["src/"]).expect_err("trailing slash"),
            ScopeError::TrailingSlash {
                path: "src/".into()
            }
        );
        for bad in [".", "..", "/etc/passwd", "a//b", "a/./b", "a/.."] {
            assert!(
                scope(&[], &[bad]).is_err(),
                "{bad:?} must not be a scope path"
            );
        }
        assert_eq!(
            scope(&[""], &[]).expect_err("empty crate"),
            ScopeError::EmptyCrate
        );
    }

    #[test]
    fn a_separator_cannot_be_smuggled_in_as_data() {
        // Without this the two-separator convention proves nothing: a crate
        // named "a\u{1f}b" would encode exactly as the two-crate set {a, b}.
        assert_eq!(
            scope(&["a\u{1f}b"], &[]).expect_err("control character"),
            ScopeError::ControlCharacter {
                text: "a\u{1f}b".to_owned(),
                at: 1,
                found: '\u{1f}',
            }
        );
        assert!(scope(&[], &["a\u{1e}b"]).is_err());
        assert!(scope(&[], &["a\nb"]).is_err());
    }

    #[test]
    fn the_encoding_distinguishes_the_two_sets() {
        // One separator would make these identical, and identical bytes are
        // one identifier for two requirements.
        let as_crates = scope(&["a", "b"], &[]).expect("valid");
        let split = scope(&["a"], &["b"]).expect("valid");
        assert_ne!(as_crates.canonical_bytes(), split.canonical_bytes());
    }

    #[test]
    fn insertion_order_and_duplicates_do_not_reach_the_encoding() {
        let one = scope(&["b", "a", "b"], &["y", "x"]).expect("valid");
        let other = scope(&["a", "b"], &["x", "y", "x"]).expect("valid");
        assert_eq!(one, other);
        assert_eq!(one.canonical_bytes(), other.canonical_bytes());
    }

    proptest! {
        /// The encoding can be read back, which is what injective means.
        #[test]
        fn the_encoding_is_readable_back(
            crates in prop::collection::btree_set("[a-z@][a-z0-9._@-]{0,8}", 0..5),
            paths in prop::collection::btree_set("[a-z][a-z0-9._-]{0,6}(/[a-z][a-z0-9._-]{0,6}){0,3}", 0..5),
        ) {
            let scope = RequirementScope::new(
                crates.iter().map(CrateId::new).collect::<Vec<_>>(),
                paths.iter().cloned().collect::<Vec<_>>(),
            ).expect("the generators only produce accepted members");

            let bytes = scope.canonical_bytes();
            let text = String::from_utf8(bytes).expect("members are UTF-8");
            let (crate_part, path_part) = text
                .rsplit_once(char::from(RECORD_SEPARATOR))
                .expect("exactly one set boundary");

            let split = |part: &str| -> Vec<String> {
                if part.is_empty() {
                    Vec::new()
                } else {
                    part.split(char::from(UNIT_SEPARATOR))
                        .map(ToOwned::to_owned)
                        .collect()
                }
            };
            // Byte order, which is what `canonical_bytes` imposes — and for
            // paths that is deliberately *not* the `BTreeSet<Utf8PathBuf>`
            // order, so this expectation is built from the raw strings.
            let mut expected_crates: Vec<String> = crates.into_iter().collect();
            expected_crates.sort_unstable();
            let mut expected_paths: Vec<String> = paths.into_iter().collect();
            expected_paths.sort_unstable();

            prop_assert_eq!(split(crate_part), expected_crates);
            prop_assert_eq!(split(path_part), expected_paths);
        }
    }
}
