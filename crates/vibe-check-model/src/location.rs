//! Source locations.
//!
//! Every risk flag and every finding carries locations, because "this pull
//! request touches unsafe code" is not actionable and "this pull request
//! introduces unsafe at `ring.rs:88` in `Ring::push_unchecked`" is.
//!
//! Locations record the **blob hash** alongside the line number. Line numbers
//! are relative to a specific version of a file; after a force-push or a rebase
//! they may point somewhere else entirely. The blob hash stays resolvable, which
//! is what lets an archived bundle still be read months later.

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A one-based, inclusive range of lines.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct LineRange {
    /// First line, one-based and inclusive.
    pub start: u32,
    /// Last line, one-based and inclusive.
    pub end: u32,
}

impl LineRange {
    /// A range covering a single line.
    #[must_use]
    pub fn single(line: u32) -> Self {
        Self {
            start: line,
            end: line,
        }
    }

    /// Whether `line` falls within this range.
    #[must_use]
    pub fn contains(&self, line: u32) -> bool {
        line >= self.start && line <= self.end
    }

    /// Whether two ranges share any line.
    ///
    /// Used to scope diff-relative capabilities: a mutation survivor or an
    /// uncovered line only counts when it overlaps a range the pull request
    /// actually changed.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

impl fmt::Display for LineRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.start == self.end {
            write!(f, "{}", self.start)
        } else {
            write!(f, "{}-{}", self.start, self.end)
        }
    }
}

/// A location in a source file at a particular revision.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct Location {
    /// Repository-relative path. UTF-8 by construction; non-UTF-8 paths are
    /// rejected at the boundary rather than lossily converted, because a lossy
    /// path silently changes which crate a change is attributed to.
    pub path: Utf8PathBuf,

    /// Git blob hash of the file this location was computed against.
    ///
    /// Present whenever the location came from a tracked file. This is what
    /// keeps the location meaningful after the branch is rewritten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_sha: Option<String>,

    /// Line range, when the location is finer-grained than the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<LineRange>,

    /// The enclosing item, e.g. `kono_core::ring::Ring::<T>::push_unchecked`.
    ///
    /// Rendered in the pull-request comment because a symbol path survives
    /// rebases that a line number does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
}

impl Location {
    /// A location referring to a whole file.
    #[must_use]
    pub fn file(path: impl Into<Utf8PathBuf>) -> Self {
        Self {
            path: path.into(),
            blob_sha: None,
            lines: None,
            item: None,
        }
    }

    /// Narrow this location to a line range.
    #[must_use]
    pub fn at_lines(mut self, lines: LineRange) -> Self {
        self.lines = Some(lines);
        self
    }

    /// Attach the enclosing item path.
    #[must_use]
    pub fn in_item(mut self, item: impl Into<String>) -> Self {
        self.item = Some(item.into());
        self
    }

    /// Attach the blob hash of the file this location was computed against.
    #[must_use]
    pub fn at_blob(mut self, blob_sha: impl Into<String>) -> Self {
        self.blob_sha = Some(blob_sha.into());
        self
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.lines {
            Some(lines) => write!(f, "{}:{lines}", self.path),
            None => write!(f, "{}", self.path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_way_an_editor_expects() {
        let loc = Location::file("crates/core/src/ring.rs").at_lines(LineRange::single(88));
        // `path:line` is clickable in most terminals and editors, which is the
        // whole reason for this format.
        assert_eq!(loc.to_string(), "crates/core/src/ring.rs:88");
    }

    #[test]
    fn renders_a_span_as_a_range() {
        let loc = Location::file("a.rs").at_lines(LineRange { start: 88, end: 94 });
        assert_eq!(loc.to_string(), "a.rs:88-94");
    }

    #[test]
    fn overlap_is_symmetric_and_inclusive() {
        let changed = LineRange { start: 10, end: 20 };
        // Touching at exactly one endpoint counts: a mutation on line 20 is
        // inside a hunk that ends at line 20.
        assert!(changed.overlaps(&LineRange::single(20)));
        assert!(LineRange::single(20).overlaps(&changed));
        assert!(!changed.overlaps(&LineRange::single(21)));
        assert!(!LineRange::single(9).overlaps(&changed));
    }

    #[test]
    fn omits_absent_fields_from_the_wire_form() {
        let json = serde_json::to_string(&Location::file("a.rs")).expect("serialize");
        assert_eq!(json, r#"{"path":"a.rs"}"#);
    }
}
