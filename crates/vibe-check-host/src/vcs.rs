//! Version-control access.
//!
//! Most of this is satisfied by `karet-vcs`, whose `range_changes(base, head,
//! merge_base: true)` is exactly the pull-request diff and which forces rename
//! detection on regardless of the user's `diff.renames` setting. That last part
//! matters more than it looks: rename detection decides which crate a file
//! belongs to, so a developer's personal git configuration must not be able to
//! change a verdict.
//!
//! The operations `karet-vcs` does not cover — git notes, worktree creation —
//! are subprocess calls made with `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM`
//! pointed at nothing, for the same reason.

use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;

/// How a file changed between two revisions.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum ChangeKind {
    /// Newly added.
    Added,
    /// Contents changed.
    Modified,
    /// Removed.
    Deleted,
    /// Moved, possibly with edits.
    Renamed,
}

/// One changed file, with both sides of its contents.
///
/// Carrying the full before and after text rather than a formatted diff is what
/// lets the classifier parse both sides into syntax trees and compare them
/// structurally, instead of pattern-matching a textual patch.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileChange {
    /// Path at head.
    pub path: Utf8PathBuf,
    /// Path at base, when the file moved.
    pub old_path: Option<Utf8PathBuf>,
    /// What kind of change.
    pub kind: ChangeKind,
    /// Whether the contents are binary, in which case both sides are empty.
    pub is_binary: bool,
    /// Contents at the merge base.
    pub old: String,
    /// Contents at head.
    pub new: String,
}

/// Why a version-control operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VcsError {
    /// The two revisions share no common ancestor, or history is too shallow.
    ///
    /// Never falls back to head policy: a change judged by rules it authored is
    /// not judged at all. The caller escalates instead.
    #[error("no merge base between {base} and {head}: {detail}")]
    NoMergeBase {
        /// Base revision.
        base: String,
        /// Head revision.
        head: String,
        /// What went wrong — commonly a shallow clone.
        detail: String,
    },
    /// A revision could not be resolved.
    #[error("cannot resolve revision `{0}`")]
    BadRevision(String),
    /// A path was not valid UTF-8.
    ///
    /// Rejected rather than lossily converted: a lossy path silently changes
    /// which crate a change is attributed to, and therefore which policy applies.
    #[error("path is not valid UTF-8: {0}")]
    NonUtf8Path(String),
    /// Anything else.
    #[error("git operation failed: {0}")]
    Git(String),
}

/// Read access to the repository, plus the worktree operations the negation
/// probe needs.
#[async_trait]
pub trait Vcs: Send + Sync {
    /// Resolve a revision to a full hash.
    async fn resolve(&self, rev: &str) -> Result<String, VcsError>;

    /// The merge base of two revisions.
    ///
    /// Always computed, never taken from a webhook payload.
    async fn merge_base(&self, base_rev: &str, head_rev: &str) -> Result<String, VcsError>;

    /// Files that `head` changed since it diverged from `base`.
    async fn changed_files(
        &self,
        base_rev: &str,
        head_rev: &str,
    ) -> Result<Vec<FileChange>, VcsError>;

    /// Read a file's contents at a revision.
    ///
    /// How the policy document is read from the merge base without touching the
    /// working tree, which belongs to the pull request and must not be disturbed.
    async fn blob_at_rev(&self, rev: &str, path: &Utf8Path) -> Result<Option<Vec<u8>>, VcsError>;

    /// Hash of a directory tree at a revision.
    ///
    /// Comparing tree hashes is how gate-integrity detects that a pull request
    /// touched its own configuration: one comparison catches content edits, mode
    /// changes, symlink swaps, additions and renames together, where a file list
    /// would need each case handled separately.
    async fn tree_hash(&self, rev: &str, path: &Utf8Path) -> Result<Option<String>, VcsError>;

    /// The committer date of a commit.
    ///
    /// **The clock for every time-dependent decision.** Waiver expiry is
    /// compared against this, not against the wall clock, so re-evaluating an
    /// old pull request produces the verdict it had rather than a new one.
    async fn committer_date(&self, rev: &str) -> Result<Timestamp, VcsError>;

    /// Create a detached worktree at a revision.
    ///
    /// The negation probe applies added test files to the base commit and
    /// expects them to fail. That has to happen somewhere other than the
    /// checkout the rest of the run is using.
    async fn add_worktree(&self, rev: &str, at: &Utf8Path) -> Result<(), VcsError>;

    /// Remove a worktree created by [`add_worktree`](Self::add_worktree).
    async fn remove_worktree(&self, at: &Utf8Path) -> Result<(), VcsError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_merge_base_names_the_likely_cause() {
        // In CI this is nearly always a shallow clone, and saying so saves the
        // reader a debugging session over a one-line workflow fix.
        let err = VcsError::NoMergeBase {
            base: "master".into(),
            head: "9f3c".into(),
            detail: "shallow clone; set fetch-depth: 0".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("master"));
        assert!(msg.contains("fetch-depth"));
    }
}
