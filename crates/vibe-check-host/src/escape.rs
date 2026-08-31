//! Recording what the tiers actually cost.
//!
//! Without this, tier boundaries are somebody's guess that never gets revisited.
//! With it, a category that proves reliable can be demoted on evidence, and one
//! that does not can be promoted on evidence.
//!
//! # Why git notes
//!
//! Two refs: one records every adjudication at merge time (the denominator), one
//! records defects attributed back to a merge (the numerator). Both travel with
//! the repository, so the loop works from a clone with no database and no
//! service. CI artifacts expire in about ninety days while a useful escape
//! window is a hundred and eighty or more — storing the denominator in artifacts
//! would mean it evaporates exactly when it starts being interesting.
//!
//! # Why JSON Lines
//!
//! Notes are merged with git's `cat_sort_uniq` strategy, which concatenates,
//! sorts, and de-duplicates *lines*. With one canonical JSON object per line the
//! ledger becomes a grow-only set that merges without conflict across concurrent
//! CI writers and developer machines. One object per note would conflict on
//! every concurrent write; pretty-printed JSON would defeat the de-duplication.
//!
//! Records are append-only. A correction is a tombstone that retracts an earlier
//! record, never a deletion — the history of what we believed is itself data.

use async_trait::async_trait;

/// The ref holding one adjudication per merge commit: the denominator.
pub const ADJUDICATIONS_REF: &str = "refs/notes/vibe-check/adjudications";

/// The ref holding defect records: the numerator.
pub const ESCAPES_REF: &str = "refs/notes/vibe-check/escapes";

/// Why an escape-store operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EscapeError {
    /// The ref does not exist yet, which is the normal state before the first
    /// record.
    #[error("no records under {0}")]
    Empty(String),
    /// Anything else.
    #[error("escape store failed: {0}")]
    Store(String),
}

/// Append-only storage for adjudication and defect records.
///
/// A trait rather than git-notes calls inline so that a future API or database
/// backend is an additional implementation rather than a rewrite of the
/// statistics.
#[async_trait]
pub trait EscapeStore: Send + Sync {
    /// Append one canonical JSON line against a commit.
    ///
    /// Must be idempotent: re-running the same command with the same inputs
    /// produces a byte-identical line, which `cat_sort_uniq` then collapses.
    async fn append(&self, note_ref: &str, commit: &str, line: &str) -> Result<(), EscapeError>;

    /// Read every line recorded against a commit.
    async fn read(&self, note_ref: &str, commit: &str) -> Result<Vec<String>, EscapeError>;

    /// Every commit that has a record under this ref.
    async fn commits_with_notes(&self, note_ref: &str) -> Result<Vec<String>, EscapeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_refs_are_namespaced_under_vibe_check() {
        // Sharing `refs/notes/commits` with anything else would make our records
        // and somebody else's indistinguishable after a `cat_sort_uniq` merge.
        assert!(ADJUDICATIONS_REF.starts_with("refs/notes/vibe-check/"));
        assert!(ESCAPES_REF.starts_with("refs/notes/vibe-check/"));
        assert_ne!(ADJUDICATIONS_REF, ESCAPES_REF);
    }
}
