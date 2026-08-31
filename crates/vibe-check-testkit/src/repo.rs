//! Building throwaway git repositories whose hashes are reproducible.
//!
//! # Why fixtures are trees, not repositories
//!
//! Committing a `.git` directory to the test suite means the fixtures are opaque
//! to review, awkward to update, and prone to breaking when git's on-disk format
//! moves. Instead a fixture is a plain directory of files, and this builder
//! materializes a real repository from it at test time.
//!
//! # Why the hashes have to be stable
//!
//! A commit hash is a function of its content *and* its author, committer, and
//! timestamps. Left to the ambient environment those vary per machine and per
//! second, so every hash in a golden file would be wrong on the next run.
//!
//! Pinning identity and dates makes the hashes deterministic, which means a
//! snapshot can contain a real commit hash and stay meaningful. Everything here
//! is passed explicitly rather than read from the environment, so a developer's
//! own git configuration cannot reach in and change a test result.

use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use tempfile::TempDir;

/// Fixed identity for every commit these fixtures produce.
const AUTHOR_NAME: &str = "vibe-check fixtures";
const AUTHOR_EMAIL: &str = "fixtures@vibe-check.invalid";
/// A fixed instant, so hashes do not move. 2020-01-01T00:00:00Z.
const BASE_EPOCH_SECS: i64 = 1_577_836_800;

/// A throwaway git repository with reproducible commit hashes.
///
/// The directory is removed when this value drops.
#[derive(Debug)]
pub struct TestRepo {
    dir: TempDir,
    root: Utf8PathBuf,
    commit_count: i64,
}

impl TestRepo {
    /// Create an empty repository on branch `master`.
    ///
    /// # Panics
    /// Panics if `git` is unavailable or the temporary directory cannot be
    /// created. This is test-only scaffolding; a failure here means the
    /// environment is broken, not that a behaviour under test is wrong.
    #[must_use]
    pub fn init() -> Self {
        let dir = TempDir::new().expect("create temporary directory");
        let root = Utf8Path::from_path(dir.path())
            .expect("temporary directory path is valid UTF-8")
            .to_owned();
        let repo = Self {
            dir,
            root,
            commit_count: 0,
        };
        // `master` explicitly: the default branch name depends on the git
        // version and on the user's `init.defaultBranch`, and a fixture that
        // changes branch name across machines is a fixture that fails on
        // somebody else's laptop.
        repo.git(&["init", "--quiet", "--initial-branch=master"]);
        repo
    }

    /// The repository root.
    #[must_use]
    pub fn root(&self) -> &Utf8Path {
        &self.root
    }

    /// Write a file, creating parent directories.
    ///
    /// # Panics
    /// Panics if the write fails.
    pub fn write(&self, path: impl AsRef<Utf8Path>, contents: impl AsRef<str>) {
        let full = self.root.join(path.as_ref());
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent directories");
        }
        std::fs::write(&full, contents.as_ref()).expect("write fixture file");
    }

    /// Delete a file.
    ///
    /// # Panics
    /// Panics if the file is missing or cannot be removed.
    pub fn remove(&self, path: impl AsRef<Utf8Path>) {
        std::fs::remove_file(self.root.join(path.as_ref())).expect("remove fixture file");
    }

    /// Stage everything and commit, returning the full commit hash.
    ///
    /// Each commit advances the fixed clock by one minute, so ordering is
    /// well-defined without reintroducing the wall clock.
    ///
    /// # Panics
    /// Panics if the commit fails.
    pub fn commit(&mut self, message: &str) -> String {
        self.commit_count += 1;
        let stamp = format!("{} +0000", BASE_EPOCH_SECS + self.commit_count * 60);
        self.git(&["add", "--all"]);
        self.git_with_env(
            &[
                "commit",
                "--quiet",
                "--allow-empty",
                "--no-gpg-sign",
                "-m",
                message,
            ],
            &[
                ("GIT_AUTHOR_DATE", stamp.as_str()),
                ("GIT_COMMITTER_DATE", stamp.as_str()),
            ],
        );
        self.rev_parse("HEAD")
    }

    /// Create a branch and switch to it.
    ///
    /// # Panics
    /// Panics if the branch cannot be created.
    pub fn branch(&self, name: &str) {
        self.git(&["checkout", "--quiet", "-b", name]);
    }

    /// Switch to an existing branch or revision.
    ///
    /// # Panics
    /// Panics if the checkout fails.
    pub fn checkout(&self, rev: &str) {
        self.git(&["checkout", "--quiet", rev]);
    }

    /// Resolve a revision to a full hash.
    ///
    /// # Panics
    /// Panics if the revision cannot be resolved.
    #[must_use]
    pub fn rev_parse(&self, rev: &str) -> String {
        self.git(&["rev-parse", rev]).trim().to_owned()
    }

    /// Run a git command, returning stdout.
    ///
    /// # Panics
    /// Panics if git exits non-zero, printing stderr — a fixture that half-built
    /// itself produces a far more confusing failure later.
    pub fn git(&self, args: &[&str]) -> String {
        self.git_with_env(args, &[])
    }

    fn git_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let mut cmd = Command::new("git");
        cmd.current_dir(self.dir.path())
            .args(args)
            // Neutralize the ambient environment. A developer's global git
            // configuration must not be able to change a test outcome — the same
            // reasoning that makes vibe-check itself pin these when it shells out.
            //
            // `GIT_DIR` and `GIT_WORK_TREE` are *removed* rather than pinned,
            // because they are not configuration, they are redirection: both
            // outrank `current_dir`, so an inherited one aims every command in
            // this module at a repository the fixture never created. Git exports
            // `GIT_DIR` into every hook process, so a test suite run from a git
            // hook would otherwise `git init` over the real repository and commit
            // fixture files into it.
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", AUTHOR_NAME)
            .env("GIT_AUTHOR_EMAIL", AUTHOR_EMAIL)
            .env("GIT_COMMITTER_NAME", AUTHOR_NAME)
            .env("GIT_COMMITTER_EMAIL", AUTHOR_EMAIL)
            .env("LC_ALL", "C")
            .env("TZ", "UTC");
        for (key, value) in env {
            cmd.env(key, value);
        }
        let out = cmd.output().expect("run git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("git output is valid UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_hashes_are_reproducible_across_repositories() {
        // The property the whole fixture design depends on: build the same
        // history twice and get the same hashes, so a golden file can name one.
        let build = || {
            let mut repo = TestRepo::init();
            repo.write("Cargo.toml", "[package]\nname = \"demo\"\n");
            repo.write("src/lib.rs", "pub fn f() {}\n");
            repo.commit("chore: initial")
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn distinct_content_yields_distinct_hashes() {
        let mut a = TestRepo::init();
        a.write("src/lib.rs", "pub fn f() {}\n");
        let first = a.commit("chore: initial");

        let mut b = TestRepo::init();
        b.write("src/lib.rs", "pub fn g() {}\n");
        let second = b.commit("chore: initial");

        assert_ne!(first, second);
    }

    #[test]
    fn a_branch_diverges_from_its_base() {
        let mut repo = TestRepo::init();
        repo.write("src/lib.rs", "pub fn f() {}\n");
        let base = repo.commit("chore: base");

        repo.branch("feature");
        repo.write("src/lib.rs", "pub unsafe fn f() {}\n");
        let head = repo.commit("feat: unsafe");

        assert_ne!(base, head);
        // The merge base of a linear branch is its starting point, which is what
        // a pull-request diff is computed against.
        let merge_base = repo.git(&["merge-base", "master", "feature"]);
        assert_eq!(merge_base.trim(), base);
    }

    #[test]
    fn the_default_branch_is_master_regardless_of_git_configuration() {
        // `symbolic-ref` rather than `rev-parse`: on a repository with no
        // commits `HEAD` points at an unborn branch and does not resolve to an
        // object, but it does have a name — which is the thing under test.
        let repo = TestRepo::init();
        let branch = repo.git(&["symbolic-ref", "--short", "HEAD"]);
        assert_eq!(branch.trim(), "master");
    }
}
