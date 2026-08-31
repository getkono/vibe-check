//! The guard on where a fixture repository is actually built.
//!
//! [`TestRepo`] neutralizes the ambient environment so a developer's git
//! configuration cannot change a test result. Configuration was never the
//! dangerous half. `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE` are
//! *redirection*: they outrank the child process's working directory, so a
//! fixture that inherits any of them stops operating on its own temporary
//! directory and starts operating on whatever repository, worktree or index the
//! caller was already pointing at.
//!
//! That is not hypothetical, and it is not a developer's misconfiguration — git
//! exports these itself. Measured on git 2.55, from a **linked worktree**, which
//! is how this repository is developed:
//!
//! | hook | `GIT_DIR` | `GIT_INDEX_FILE` |
//! | --- | --- | --- |
//! | `pre-commit`, `post-commit` | set | set |
//! | `post-index-change` | set | empty |
//! | `pre-push` | set | empty |
//!
//! From a flat clone every one of those is empty, which is why the defect
//! survived: anyone reproducing it in a plain clone finds nothing.
//!
//! `hk` runs `mise run test` on `pre-push`, so `GIT_DIR` was live and did the
//! damage — a fixture's index installed in the real `.git`, a staged two-line
//! `Cargo.toml` and a `src/lib.rs` absent from this tree, presenting as dozens
//! of phantom staged deletions; an earlier occurrence set `core.bare = true` on
//! the working checkout. `GIT_INDEX_FILE` is not reachable through `pre-push`
//! today, but it is the identical defect by the same test, and it goes live the
//! moment a test step is added to `pre-commit` or anyone runs `cargo test` from
//! a commit-time hook. It is stripped here rather than left to that discovery.
//!
//! The failure is silent from inside the suite. Git is perfectly happy to build
//! the fixture history somewhere else, so every existing assertion still passes
//! and only the developer's own repository is worse off. So this test supplies
//! the victim: it builds a second repository, points the ambient variables at
//! it, drives a fixture, and proves that repository's `HEAD`, index, worktree
//! status and bare flag all came through unchanged.
//!
//! # What this does *not* cover
//!
//! Config injection. `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n` and
//! `GIT_CONFIG_PARAMETERS` outrank `GIT_CONFIG_GLOBAL=/dev/null` instead of
//! being silenced by it, and git exports the latter into hooks. That is a
//! different defect class with a different fix (`env_clear` plus an allowlist),
//! and it is recorded in `repo.rs` rather than guarded here.
//!
//! # Why this is its own test binary
//!
//! It manipulates the process environment, which is process-global and which
//! every `Command` spawn reads. **Do not add a second `#[test]` to this file.**
//! libtest would run them concurrently and this one would poison the other's git
//! invocations intermittently, with nothing in the other test's source to
//! explain it. A new case goes in a new file, or becomes another phase below.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use camino::{Utf8Path, Utf8PathBuf};
use tempfile::TempDir;
use vibe_check_testkit::TestRepo;

/// An environment variable that exists only for the lifetime of this value.
///
/// A scope guard rather than paired calls: a phase below that fails does so by
/// panicking, and a bare `remove_var` after the panicking call never runs. The
/// unwind would then carry a live `GIT_DIR` into every later phase and turn one
/// failure into four.
struct AmbientVar(&'static str);

impl AmbientVar {
    /// Set `key` until the returned value is dropped.
    fn set(key: &'static str, value: &str) -> Self {
        // SAFETY: this binary contains exactly one test, as its module
        // documentation requires, so no other thread is reading the environment
        // or spawning a process while it is modified.
        unsafe { std::env::set_var(key, value) };
        Self(key)
    }
}

impl Drop for AmbientVar {
    fn drop(&mut self) {
        // SAFETY: as in `set`.
        unsafe { std::env::remove_var(self.0) };
    }
}

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

/// What driving a fixture produced, gathered but not yet judged.
///
/// Observations rather than assertions so the caller can compare the *victim*
/// first. A redirected fixture corrupts another repository before it looks
/// wrong itself, and the victim's diff is the diagnostic worth printing; a
/// fixture-side `assert!` that fires first would hide it behind a terse line.
struct Fixture {
    base: String,
    has_its_own_git_dir: bool,
    tracked: String,
}

/// Build a small fixture history.
///
/// Exercises every operation that writes: `init`, `add`, `commit`, `checkout -b`
/// and `checkout`.
fn drive_a_fixture() -> Fixture {
    let mut repo = TestRepo::init();
    repo.write("Cargo.toml", "[package]\nname = \"demo\"\n");
    repo.write("src/lib.rs", "pub fn safe() {}\n");
    let base = repo.commit("chore: initial");

    repo.branch("feature");
    repo.write("src/lib.rs", "pub unsafe fn safe() {}\n");
    repo.commit("feat: unsafe");
    repo.checkout("master");

    Fixture {
        base,
        has_its_own_git_dir: repo.root().join(".git").exists(),
        tracked: repo.git(&["ls-tree", "-r", "--name-only", "HEAD"]),
    }
}

/// Assert a fixture built its own repository, with its own files and hash.
fn assert_self_contained(fixture: &Fixture, expected_base: &str, variables: &str) {
    assert!(
        fixture.has_its_own_git_dir,
        "an ambient {variables} redirected the fixture: it has no repository of \
         its own, so it was built somewhere else"
    );
    assert!(
        fixture.tracked.contains("src/lib.rs"),
        "an ambient {variables} redirected the fixture: its own history does not \
         contain its own files: {:?}",
        fixture.tracked
    );
    assert_eq!(
        fixture.base, expected_base,
        "an ambient {variables} moved a fixture commit hash"
    );
}

/// The UTF-8 root of a temporary directory.
fn root_of(dir: &TempDir) -> Utf8PathBuf {
    Utf8Path::from_path(dir.path())
        .expect("temporary directory path is valid UTF-8")
        .to_owned()
}

/// Assert nothing was created at a path a redirected git would have written to.
fn assert_nothing_at(path: &Utf8Path, variables: &str) {
    assert!(
        !path.exists(),
        "an ambient {variables} redirected the fixture: git wrote {path}, \
         outside the fixture's own temporary directory"
    );
}

#[test]
fn ambient_git_redirection_variables_cannot_reach_a_fixture() {
    // The victim: a real repository standing in for the developer's checkout.
    let mut victim = TestRepo::init();
    victim.write("README.md", "the repository the hook was pointing at\n");
    victim.commit("chore: a history worth not losing");
    let before = snapshot(&victim);
    let victim_git_dir = victim.root().join(".git");
    let victim_index = victim_git_dir.join("index");

    // The hash to beat: what a fixture produces with nothing ambient set.
    let expected = drive_a_fixture().base;

    // Held, not shadowed: dropping the `TempDir` would delete the paths the
    // assertions below are about to look for.
    let scratch_dir = TempDir::new().unwrap();
    let scratch = root_of(&scratch_dir);

    // `GIT_DIR` alone — live on `pre-push` today, and the one that caused the
    // observed corruption. Unfixed, the fixture's `git init` reinitializes the
    // victim and its `git add --all` stages the fixture's files into its index.
    let fixture = {
        let _dir = AmbientVar::set("GIT_DIR", victim_git_dir.as_str());
        drive_a_fixture()
    };
    assert_eq!(
        snapshot(&victim),
        before,
        "an ambient GIT_DIR let a fixture write into another repository"
    );
    assert_self_contained(&fixture, &expected, "GIT_DIR");

    // `GIT_INDEX_FILE` alone — exported to commit-time hooks from a linked
    // worktree. Unfixed, the fixture builds its own repository but stages into
    // the victim's index, destroying it without touching its `HEAD`.
    let fixture = {
        let _index = AmbientVar::set("GIT_INDEX_FILE", victim_index.as_str());
        drive_a_fixture()
    };
    assert_eq!(
        snapshot(&victim),
        before,
        "an ambient GIT_INDEX_FILE let a fixture write over another repository's index"
    );
    assert_self_contained(&fixture, &expected, "GIT_INDEX_FILE");

    // `GIT_WORK_TREE` alone. Git refuses it without a `GIT_DIR`, so an inherited
    // one turns every fixture in this crate into a panic rather than a wrong
    // answer — still a failure, and still caused by the environment.
    let stray = scratch.join("work-tree-only");
    std::fs::create_dir_all(&stray).unwrap();
    let fixture = {
        let _tree = AmbientVar::set("GIT_WORK_TREE", stray.as_str());
        drive_a_fixture()
    };
    assert_nothing_at(&stray.join("src"), "GIT_WORK_TREE");
    assert_nothing_at(&stray.join("Cargo.toml"), "GIT_WORK_TREE");
    assert_self_contained(&fixture, &expected, "GIT_WORK_TREE");

    // All three together: the shape a commit-time hook hands its child process
    // in a linked worktree, with the work tree aimed somewhere a checkout would
    // deposit files.
    let both = scratch.join("hook-shaped-work-tree");
    std::fs::create_dir_all(&both).unwrap();
    let fixture = {
        let _dir = AmbientVar::set("GIT_DIR", victim_git_dir.as_str());
        let _index = AmbientVar::set("GIT_INDEX_FILE", victim_index.as_str());
        let _tree = AmbientVar::set("GIT_WORK_TREE", both.as_str());
        drive_a_fixture()
    };
    assert_eq!(
        snapshot(&victim),
        before,
        "the ambient variables a git hook exports let a fixture write into another repository"
    );
    assert_nothing_at(
        &both.join("src"),
        "GIT_DIR, GIT_INDEX_FILE and GIT_WORK_TREE",
    );
    assert_nothing_at(
        &both.join("README.md"),
        "GIT_DIR, GIT_INDEX_FILE and GIT_WORK_TREE",
    );
    assert_self_contained(
        &fixture,
        &expected,
        "GIT_DIR, GIT_INDEX_FILE and GIT_WORK_TREE",
    );
}
