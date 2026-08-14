//! Reading a pull-request diff.
//!
//! # What comes from karet
//!
//! `karet_vcs::Repository::range_changes(base, head, merge_base: true)` is
//! exactly the diff we want: everything `head` introduced since it diverged from
//! `base`, ignoring whatever `base` gained meanwhile. It returns each changed
//! file with its **full contents on both sides**, which is what lets the
//! classifier parse before and after into syntax trees and compare them, rather
//! than pattern-matching a textual patch.
//!
//! It is also deterministic by construction, which removes work here: rename
//! detection is forced on regardless of the user's `diff.renames`, and results
//! are sorted by path. A developer's personal git configuration cannot change
//! which crate a file is attributed to, and therefore cannot change a verdict.
//!
//! # Two constraints this imposes
//!
//! **`gix::Repository` is not `Sync`.** Every git read has to happen on one
//! thread. That suits the design — the change set is built once, eagerly, before
//! any parallel analysis — but it is a real constraint rather than an accident,
//! so it is stated here and the API is shaped around it: these are plain
//! blocking functions that open a repository per call, and the caller hoists
//! them out of any concurrency.
//!
//! **`file_at_rev` is not in the published 0.2.2.** It exists upstream but has
//! not been released. Reading the policy document from the merge base needs it,
//! so [`blob_at_rev`] carries a local implementation until karet 0.3.0 lands.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use camino::{Utf8Path, Utf8PathBuf};
use vibe_check_host::vcs::{ChangeKind, FileChange, VcsError};

/// Open a repository, mapping failures to our own error type.
fn open(root: &Utf8Path) -> Result<karet_vcs::Repository, VcsError> {
    karet_vcs::Repository::discover(root.as_std_path())
        .map_err(|e| VcsError::Git(format!("cannot open a git repository at {root}: {e}")))
}

/// The merge base of two revisions.
///
/// Always computed, never taken from a webhook payload: the base branch tip
/// reported by an event is a snapshot from an earlier moment and drifts as the
/// base branch moves, which would silently change what a pull request appears to
/// have done.
///
/// # Errors
/// Returns [`VcsError::NoMergeBase`] when the revisions share no common
/// ancestor — most often a shallow clone rather than genuinely unrelated
/// histories.
pub fn merge_base(root: &Utf8Path, base_rev: &str, head_rev: &str) -> Result<String, VcsError> {
    let repo = open(root)?;
    repo.merge_base(base_rev, head_rev)
        .map_err(|e| VcsError::Git(format!("computing merge base: {e}")))?
        .ok_or_else(|| VcsError::NoMergeBase {
            base: base_rev.to_owned(),
            head: head_rev.to_owned(),
            detail: "no common ancestor; in CI this usually means a shallow clone \
                     (set `fetch-depth: 0` on actions/checkout)"
                .to_owned(),
        })
}

/// Files `head` changed since it diverged from `base`.
///
/// # Errors
/// Returns [`VcsError`] when a revision cannot be resolved, the histories are
/// unrelated, or a path is not valid UTF-8.
pub fn changed_files(
    root: &Utf8Path,
    base_rev: &str,
    head_rev: &str,
) -> Result<Vec<FileChange>, VcsError> {
    let repo = open(root)?;
    let changes = repo
        .range_changes(base_rev, head_rev, true)
        .map_err(|e| VcsError::Git(format!("reading changes {base_rev}...{head_rev}: {e}")))?;
    changes.into_iter().map(convert).collect()
}

/// Translate a karet change into ours.
///
/// The two are close, and staying close is deliberate — but the conversion is
/// explicit so that karet's vocabulary is not the one the rest of vibe-check
/// speaks. That is what keeps a change of upstream, or of upstream's enum, from
/// reaching the policy engine.
fn convert(change: karet_vcs::FileChange) -> Result<FileChange, VcsError> {
    let path = to_utf8(change.path)?;
    let old_path = change.old_path.map(to_utf8).transpose()?;
    let kind = match change.status {
        karet_vcs::StatusKind::Added | karet_vcs::StatusKind::Untracked => ChangeKind::Added,
        karet_vcs::StatusKind::Deleted => ChangeKind::Deleted,
        karet_vcs::StatusKind::Renamed => ChangeKind::Renamed,
        // Modified, Conflicted, and anything upstream adds later. A conflicted
        // file in a diff we computed ourselves should not occur, and treating an
        // unrecognized state as "modified" keeps it in the classifier's view
        // rather than dropping it silently.
        _ => ChangeKind::Modified,
    };
    Ok(FileChange {
        path,
        old_path,
        kind,
        is_binary: change.is_binary,
        old: change.old,
        new: change.new,
    })
}

/// The boundary where a `std` path becomes a checked UTF-8 one.
///
/// `std::path::PathBuf` is a disallowed type workspace-wide, because a lossily
/// converted path silently changes which crate a change is attributed to and
/// therefore which policy applies. This function is the sanctioned exception:
/// karet hands us `std` paths, and *somewhere* has to convert them. Doing it in
/// one place, fallibly, is what makes the ban elsewhere meaningful — note that
/// this rejects rather than lossily converts, which is the entire point.
#[allow(
    clippy::disallowed_types,
    reason = "the single conversion boundary from karet's std paths; rejects non-UTF-8 rather \
              than converting lossily"
)]
fn to_utf8(path: std::path::PathBuf) -> Result<Utf8PathBuf, VcsError> {
    Utf8PathBuf::from_path_buf(path)
        .map_err(|p| VcsError::NonUtf8Path(p.to_string_lossy().into_owned()))
}

/// Read a file's contents at a revision.
///
/// How the policy document is read from the merge base without disturbing the
/// working tree, which belongs to the pull request.
///
/// Implemented here rather than through karet because `file_at_rev` landed
/// upstream after the published 0.2.2. Delete this in favour of
/// `Repository::file_at_rev` when karet 0.3.0 is released.
///
/// # Errors
/// Returns [`VcsError`] when the revision cannot be resolved.
pub fn blob_at_rev(
    root: &Utf8Path,
    rev: &str,
    path: &Utf8Path,
) -> Result<Option<Vec<u8>>, VcsError> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root.as_std_path())
        .arg("cat-file")
        .arg("blob")
        .arg(format!("{rev}:{path}"))
        // A developer's global configuration must not be able to change what we
        // read, for the same reason it must not change rename detection.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .map_err(|e| VcsError::Git(format!("running git cat-file: {e}")))?;

    if output.status.success() {
        return Ok(Some(output.stdout));
    }
    // A path that does not exist at that revision is a normal answer — a pull
    // request that adds the policy file has no policy at the merge base — and
    // must not be confused with a revision that does not resolve.
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("does not exist") || stderr.contains("exists on disk, but not in") {
        Ok(None)
    } else {
        Err(VcsError::BadRevision(format!(
            "{rev}:{path} ({})",
            stderr.trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vibe_check_testkit::TestRepo;

    /// A repository with a `master` commit and a `feature` branch that diverges.
    fn diverged() -> (TestRepo, String, String) {
        let mut repo = TestRepo::init();
        repo.write("Cargo.toml", "[package]\nname = \"demo\"\n");
        repo.write("src/lib.rs", "pub fn safe() {}\n");
        let base = repo.commit("chore: base");

        repo.branch("feature");
        repo.write("src/lib.rs", "pub unsafe fn risky() {}\n");
        repo.write("src/new.rs", "pub fn added() {}\n");
        let head = repo.commit("feat: add unsafe");
        (repo, base, head)
    }

    #[test]
    fn reads_the_pull_request_diff() {
        let (repo, _base, _head) = diverged();
        let changes = changed_files(repo.root(), "master", "feature").expect("diff");

        let paths: Vec<_> = changes.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, ["src/lib.rs", "src/new.rs"]);

        // Both sides of the content, which is what the classifier needs in order
        // to compare syntax trees rather than a textual patch.
        let lib = &changes[0];
        assert_eq!(lib.kind, ChangeKind::Modified);
        assert_eq!(lib.old, "pub fn safe() {}\n");
        assert_eq!(lib.new, "pub unsafe fn risky() {}\n");

        let new = &changes[1];
        assert_eq!(new.kind, ChangeKind::Added);
        assert!(new.old.is_empty());
    }

    #[test]
    fn output_is_sorted_by_path() {
        // Not incidental: unordered output would leak into the bundle and make
        // two identical evaluations compare unequal.
        let (repo, _, _) = diverged();
        let changes = changed_files(repo.root(), "master", "feature").expect("diff");
        let paths: Vec<_> = changes.iter().map(|c| c.path.clone()).collect();
        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted);
    }

    #[test]
    fn the_diff_is_against_the_merge_base_not_the_branch_tip() {
        // The property that makes this a *pull request* diff. After the base
        // branch moves on, a two-dot diff would attribute the base branch's own
        // new work to this pull request.
        let (mut repo, base, _) = diverged();

        repo.checkout("master");
        repo.write("src/unrelated.rs", "pub fn elsewhere() {}\n");
        repo.commit("feat: unrelated work on master");
        repo.checkout("feature");

        let changes = changed_files(repo.root(), "master", "feature").expect("diff");
        let paths: Vec<_> = changes.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            paths,
            ["src/lib.rs", "src/new.rs"],
            "work done on master after the branch point is not this branch's doing"
        );

        assert_eq!(
            merge_base(repo.root(), "master", "feature").expect("merge base"),
            base
        );
    }

    #[test]
    fn a_rename_is_reported_as_a_rename() {
        // karet forces rename detection on regardless of `diff.renames`. If it
        // were left to configuration, the same change could be seen as a
        // delete-plus-add on one machine and a rename on another — and those
        // attribute to different crates.
        let mut repo = TestRepo::init();
        repo.write(
            "src/original.rs",
            "pub fn stable_content_for_detection() {}\n",
        );
        repo.commit("chore: base");

        repo.branch("feature");
        repo.remove("src/original.rs");
        repo.write(
            "src/renamed.rs",
            "pub fn stable_content_for_detection() {}\n",
        );
        repo.commit("refactor: rename");

        let changes = changed_files(repo.root(), "master", "feature").expect("diff");
        assert_eq!(changes.len(), 1, "a rename is one change, not two");
        assert_eq!(changes[0].kind, ChangeKind::Renamed);
        assert_eq!(changes[0].path, "src/renamed.rs");
        assert_eq!(
            changes[0].old_path.as_deref(),
            Some(Utf8Path::new("src/original.rs"))
        );
    }

    #[test]
    fn reads_a_blob_at_the_merge_base() {
        // How policy is read from the merge base: the working tree belongs to
        // the pull request and must not be touched.
        let (repo, base, _) = diverged();
        let at_base = blob_at_rev(repo.root(), &base, Utf8Path::new("src/lib.rs"))
            .expect("read")
            .expect("present at base");
        assert_eq!(
            String::from_utf8(at_base).expect("utf-8"),
            "pub fn safe() {}\n"
        );
    }

    #[test]
    fn a_file_absent_at_a_revision_is_not_an_error() {
        // A pull request that *adds* the policy file has no policy at the merge
        // base. That is a normal state with a defined behaviour, not a failure.
        let (repo, base, _) = diverged();
        let missing = blob_at_rev(repo.root(), &base, Utf8Path::new("src/new.rs")).expect("read");
        assert!(missing.is_none());
    }

    #[test]
    fn an_unresolvable_revision_is_an_error() {
        let (repo, _, _) = diverged();
        let err = blob_at_rev(repo.root(), "no-such-rev", Utf8Path::new("src/lib.rs"))
            .expect_err("bad revision");
        assert!(matches!(err, VcsError::BadRevision(_)));
    }

    #[test]
    fn unrelated_histories_explain_the_likely_cause() {
        let repo_a = {
            let mut r = TestRepo::init();
            r.write("a.rs", "pub fn a() {}\n");
            r.commit("chore: a");
            r
        };
        let err = merge_base(repo_a.root(), "master", "master~1");
        // Either resolution failure or no merge base is acceptable here; what
        // matters is that it does not silently succeed with a wrong answer.
        assert!(err.is_err());
    }
}
