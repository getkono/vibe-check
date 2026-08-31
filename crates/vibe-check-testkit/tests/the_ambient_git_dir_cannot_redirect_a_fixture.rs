//! The guard on where a fixture repository is actually built.
//!
//! [`TestRepo`] neutralizes the ambient environment so a developer's git
//! configuration cannot change a test result. Configuration was never the
//! dangerous half. `GIT_DIR` and `GIT_WORK_TREE` are *redirection*: they outrank
//! the child process's working directory, so a fixture that inherits either one
//! stops operating on its own temporary directory and starts operating on
//! whatever repository the caller was already pointing at.
//!
//! That is not hypothetical, and it is not a developer's misconfiguration. Git
//! exports `GIT_DIR` into every hook process it runs. This repository's `hk`
//! pre-push hook runs `mise run test`, so the suite inherits a `GIT_DIR` aimed
//! at the real checkout, and every fixture `git init`, `git add --all` and
//! `git commit` in this crate lands there. The observed damage was a fixture's
//! index installed in the real `.git` — a staged two-line `Cargo.toml` and a
//! `src/lib.rs` that does not exist in this tree — presenting as dozens of
//! phantom staged deletions; on an earlier occasion the same leak set
//! `core.bare = true` on the working checkout.
//!
//! The failure is silent from inside the suite. Git is perfectly happy to build
//! the fixture history somewhere else, so every existing assertion still passes
//! and only the developer's own repository is worse off. So this test supplies
//! the victim: it builds a second repository, points the ambient variables at
//! it, drives a fixture, and proves that repository's `HEAD`, index, worktree
//! status and bare flag all came through unchanged — while the fixture itself
//! still produced its usual deterministic hash.
//!
//! # Why this is its own test binary
//!
//! It manipulates the process environment, which is process-global and which
//! every `Command` spawn reads. Sharing a binary with other tests would let
//! libtest run them concurrently and poison their git invocations too. One test,
//! one binary, phases run in sequence.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use camino::{Utf8Path, Utf8PathBuf};
use tempfile::TempDir;
use vibe_check_testkit::TestRepo;

/// Everything about a repository that a redirected fixture would disturb.
///
/// The index (`ls-files --stage`) is listed explicitly because that is what the
/// real corruption looked like: the working tree and `HEAD` were untouched, and
/// a fixture's staged entries had been written over the real one's index.
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    head: String,
    index: String,
    status: String,
    is_bare: String,
}

/// Read a repository's disturbable state.
fn snapshot(repo: &TestRepo) -> Snapshot {
    Snapshot {
        head: repo.rev_parse("HEAD"),
        index: repo.git(&["ls-files", "--stage"]),
        status: repo.git(&["status", "--porcelain"]),
        is_bare: repo.git(&["rev-parse", "--is-bare-repository"]),
    }
}

/// Build a small fixture history and return its first commit hash.
///
/// Exercises every operation that writes: `init`, `add`, `commit`, `checkout -b`
/// and `checkout`. The internal assertions catch a redirect that happens to
/// leave the fixture's own directory looking plausible.
fn drive_a_fixture() -> String {
    let mut repo = TestRepo::init();
    repo.write("Cargo.toml", "[package]\nname = \"demo\"\n");
    repo.write("src/lib.rs", "pub fn safe() {}\n");
    let base = repo.commit("chore: initial");

    repo.branch("feature");
    repo.write("src/lib.rs", "pub unsafe fn safe() {}\n");
    repo.commit("feat: unsafe");
    repo.checkout("master");

    assert!(
        repo.root().join(".git").exists(),
        "the fixture has no repository of its own at {}: it was built elsewhere",
        repo.root()
    );
    let tracked = repo.git(&["ls-tree", "-r", "--name-only", "HEAD"]);
    assert!(
        tracked.contains("src/lib.rs"),
        "the fixture's own history does not contain its own files: {tracked:?}"
    );
    base
}

/// The UTF-8 root of a temporary directory.
fn root_of(dir: &TempDir) -> Utf8PathBuf {
    Utf8Path::from_path(dir.path())
        .expect("temporary directory path is valid UTF-8")
        .to_owned()
}

/// Assert nothing was created at a path a redirected git would have written to.
fn assert_nothing_at(path: &Utf8Path, variable: &str) {
    assert!(
        !path.exists(),
        "an ambient {variable} redirected the fixture: git wrote {path}, \
         outside the fixture's own temporary directory"
    );
}

/// Set an environment variable for the phases below.
///
/// # Safety
/// This binary contains exactly one test, so no other thread is reading the
/// environment or spawning a process while it is being modified.
fn set(key: &str, value: &str) {
    unsafe { std::env::set_var(key, value) };
}

/// Clear an environment variable. See [`set`] for why this is sound here.
fn clear(key: &str) {
    unsafe { std::env::remove_var(key) };
}

#[test]
fn an_ambient_git_dir_or_work_tree_cannot_redirect_a_fixture() {
    // The victim: a real repository standing in for the developer's checkout.
    let mut victim = TestRepo::init();
    victim.write("README.md", "the repository the hook was pointing at\n");
    victim.commit("chore: a history worth not losing");
    let before = snapshot(&victim);
    let victim_root = victim.root().to_owned();
    let victim_git_dir = victim_root.join(".git");

    // The hash to beat: what a fixture produces with nothing ambient set.
    let expected = drive_a_fixture();

    // Held, not shadowed: dropping the `TempDir` would delete the paths the
    // assertions below are about to look for.
    let scratch_dir = TempDir::new().unwrap();
    let scratch = root_of(&scratch_dir);

    // `GIT_DIR` alone — the variable a git hook exports, and the one that caused
    // the observed corruption. Without the fix, the fixture's `git init`
    // reinitializes the victim and its `git add --all` stages the fixture's
    // files into the victim's index.
    set("GIT_DIR", victim_git_dir.as_str());
    let with_git_dir = drive_a_fixture();
    clear("GIT_DIR");
    assert_eq!(
        snapshot(&victim),
        before,
        "an ambient GIT_DIR let a fixture write into another repository"
    );
    assert_eq!(
        with_git_dir, expected,
        "an ambient GIT_DIR moved a fixture hash"
    );

    // `GIT_WORK_TREE` alone. Git refuses it without a `GIT_DIR`, so an inherited
    // one turns every fixture in this crate into a panic rather than a wrong
    // answer — still a failure, and still caused by the environment.
    let stray = scratch.join("work-tree-only");
    std::fs::create_dir_all(&stray).unwrap();
    set("GIT_WORK_TREE", stray.as_str());
    let with_work_tree = drive_a_fixture();
    clear("GIT_WORK_TREE");
    assert_nothing_at(&stray.join("src"), "GIT_WORK_TREE");
    assert_nothing_at(&stray.join("Cargo.toml"), "GIT_WORK_TREE");
    assert_eq!(
        with_work_tree, expected,
        "an ambient GIT_WORK_TREE moved a fixture hash"
    );

    // Both together: the shape a git hook hands its child process, with the
    // work tree aimed somewhere a checkout would deposit files.
    let both = scratch.join("both-work-tree");
    std::fs::create_dir_all(&both).unwrap();
    set("GIT_DIR", victim_git_dir.as_str());
    set("GIT_WORK_TREE", both.as_str());
    let with_both = drive_a_fixture();
    clear("GIT_DIR");
    clear("GIT_WORK_TREE");
    assert_eq!(
        snapshot(&victim),
        before,
        "an ambient GIT_DIR and GIT_WORK_TREE let a fixture write into another repository"
    );
    assert_nothing_at(&both.join("src"), "GIT_WORK_TREE");
    assert_nothing_at(&both.join("README.md"), "GIT_WORK_TREE");
    assert_eq!(
        with_both, expected,
        "an ambient GIT_DIR and GIT_WORK_TREE moved a fixture hash"
    );
}
