//! Running tools.
//!
//! # Who can execute
//!
//! Capabilities are pure. They *plan* work — they describe a program, its
//! arguments, and the conditions it must run under — and they never perform it.
//! The engine holds the [`Exec`] and is the only thing that runs anything.
//!
//! So the enforcement is not a permission flag that could be set wrongly: a
//! capability's planning context simply contains no `Exec`, and you cannot call
//! a method on a value you were never given. This is the same shape as the
//! [`ForgeRead`](crate::forge::ForgeRead) / [`ForgeWrite`](crate::forge::ForgeWrite)
//! split, and it is deliberate that both work the same way.
//!
//! # Why the plan is a value
//!
//! Because [`ProcessPlan`] is data, it can be digested. The digest goes into the
//! evidence provenance, which means a bundle proves not just that a tool ran but
//! that it ran with retries disabled, a fixed thread count, and a seeded
//! generator. A flake probe that quietly kept retries enabled would otherwise be
//! indistinguishable from one that did not.

use std::collections::BTreeMap;

use async_trait::async_trait;
use camino::Utf8PathBuf;

/// Conditions that make a run reproducible.
///
/// Part of the plan rather than a flake-probe-specific hack, so the digest
/// recorded in provenance proves which settings were actually in force.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Determinism {
    /// Disable the harness's own retry logic.
    ///
    /// A test that passes on the third attempt has not demonstrated it passes.
    pub retries_disabled: bool,
    /// Pin the thread count, when the tool supports it.
    pub threads: Option<u32>,
    /// Seed for property-testing and fuzzing harnesses.
    ///
    /// Note this is only partially achievable: `nextest` has no global seed, so
    /// we control `PROPTEST_*`, `QUICKCHECK_*`, thread count and retries, and
    /// document the residual nondeterminism rather than implying it is gone.
    pub seed: Option<u64>,
}

impl Default for Determinism {
    fn default() -> Self {
        Self {
            retries_disabled: true,
            threads: Some(1),
            seed: Some(0),
        }
    }
}

/// Which environment variables a child may see.
///
/// An allowlist, not a denylist. A denylist means every new secret in CI is a
/// leak until someone remembers to add it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EnvPolicy {
    /// Names passed through from the parent environment, sorted.
    pub inherit: Vec<String>,
    /// Explicit values, applied after inheritance.
    pub set: BTreeMap<String, String>,
}

impl Default for EnvPolicy {
    fn default() -> Self {
        Self {
            inherit: ["CARGO_HOME", "HOME", "PATH", "RUSTUP_HOME"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            // A stable locale and timezone: tool output that varies by machine
            // locale would vary the parsed evidence and therefore the digest.
            set: BTreeMap::from([
                ("LC_ALL".to_owned(), "C".to_owned()),
                ("TZ".to_owned(), "UTC".to_owned()),
            ]),
        }
    }
}

/// Whether a child may reach the network.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum NetworkPolicy {
    /// No network. The default: a capability that fetches at run time is not
    /// reproducible.
    #[default]
    Denied,
    /// Network permitted, because the tool genuinely needs it.
    Allowed,
}

/// A described, not-yet-performed subprocess.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct ProcessPlan {
    /// Program to run.
    pub program: String,
    /// Arguments, already split — never a shell string, so there is no quoting
    /// to get wrong and no shell to inject into.
    pub args: Vec<String>,
    /// Working directory, repository-relative.
    pub cwd: Utf8PathBuf,
    /// Environment policy.
    pub env: EnvPolicy,
    /// Network policy.
    pub network: NetworkPolicy,
    /// Wall-clock limit in seconds.
    ///
    /// Enforced by us rather than by the CI job, so that exceeding it produces
    /// an inconclusive result with evidence attached instead of a killed job
    /// with none.
    pub timeout_secs: u64,
    /// Reproducibility settings.
    pub determinism: Determinism,
}

impl ProcessPlan {
    /// A plan with the safe defaults: no network, deterministic, ten minutes.
    #[must_use]
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = String>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().collect(),
            cwd: Utf8PathBuf::from("."),
            env: EnvPolicy::default(),
            network: NetworkPolicy::default(),
            timeout_secs: 600,
            determinism: Determinism::default(),
        }
    }

    /// A stable string describing this plan, for digesting into provenance.
    ///
    /// Includes everything that could change the result and nothing that could
    /// not, so the digest is comparable across runs and machines.
    #[must_use]
    pub fn digest_input(&self) -> String {
        let mut s = String::new();
        s.push_str(&self.program);
        for arg in &self.args {
            s.push('\u{1f}');
            s.push_str(arg);
        }
        s.push_str("\u{1e}cwd=");
        s.push_str(self.cwd.as_str());
        s.push_str("\u{1e}inherit=");
        s.push_str(&self.env.inherit.join(","));
        for (k, v) in &self.env.set {
            s.push('\u{1e}');
            s.push_str(k);
            s.push('=');
            s.push_str(v);
        }
        s.push_str(&format!(
            "\u{1e}network={:?}\u{1e}timeout={}\u{1e}retries_disabled={}\u{1e}threads={:?}\u{1e}seed={:?}",
            self.network,
            self.timeout_secs,
            self.determinism.retries_disabled,
            self.determinism.threads,
            self.determinism.seed,
        ));
        s
    }
}

/// What a finished process produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProcessOutput {
    /// Exit code, or `None` when killed by a signal.
    pub exit_code: Option<i32>,
    /// Captured standard output.
    pub stdout: Vec<u8>,
    /// Captured standard error.
    pub stderr: Vec<u8>,
    /// How long it took, in milliseconds. Display only.
    pub duration_ms: u64,
    /// Whether the timeout fired.
    ///
    /// A timeout is inconclusive, not a failure: we learned nothing about the
    /// code, only about the budget.
    pub timed_out: bool,
}

impl ProcessOutput {
    /// Whether the process exited zero and was not killed.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        !self.timed_out && self.exit_code == Some(0)
    }
}

/// Why a process could not be run at all.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ExecError {
    /// The program is not installed.
    #[error("`{program}` is not available: {detail}")]
    NotFound {
        /// What we tried to run.
        program: String,
        /// The underlying reason.
        detail: String,
    },
    /// Spawning or waiting failed.
    #[error("could not run `{program}`: {detail}")]
    Spawn {
        /// What we tried to run.
        program: String,
        /// The underlying reason.
        detail: String,
    },
}

/// Runs subprocesses.
///
/// Implementations own their children through a process group (POSIX) or a job
/// object (Windows). `kill_on_drop` covers unwinding and task cancellation, but
/// no destructor runs after a `SIGKILL`, so a cancelled CI job would otherwise
/// leave a `cargo` tree running and racing whatever starts next.
#[async_trait]
pub trait Exec: Send + Sync {
    /// Run a plan to completion, or until its timeout.
    async fn run(&self, plan: &ProcessPlan) -> Result<ProcessOutput, ExecError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_cautious_ones() {
        let plan = ProcessPlan::new("cargo", ["test".to_owned()]);
        assert_eq!(plan.network, NetworkPolicy::Denied);
        assert!(plan.determinism.retries_disabled);
        // No inherited environment beyond the four names needed to find a
        // toolchain: a child that can read the whole environment can read
        // whatever secrets the job was given.
        assert_eq!(
            plan.env.inherit,
            ["CARGO_HOME", "HOME", "PATH", "RUSTUP_HOME"]
        );
    }

    #[test]
    fn the_digest_distinguishes_determinism_settings() {
        // The point of digesting the plan: a run with retries silently enabled
        // must not be mistakable for one without.
        let strict = ProcessPlan::new("cargo", ["test".to_owned()]);
        let mut loose = strict.clone();
        loose.determinism.retries_disabled = false;
        assert_ne!(strict.digest_input(), loose.digest_input());
    }

    #[test]
    fn the_digest_distinguishes_arguments() {
        let all = ProcessPlan::new("cargo", ["test".to_owned(), "--all-features".to_owned()]);
        let some = ProcessPlan::new("cargo", ["test".to_owned()]);
        assert_ne!(all.digest_input(), some.digest_input());
    }

    #[test]
    fn the_digest_is_not_confusable_by_argument_splitting() {
        // Joining arguments with a space would make ["a b"] and ["a", "b"]
        // digest identically, so a plan could misrepresent what it ran.
        let joined = ProcessPlan::new("cargo", ["a b".to_owned()]);
        let split = ProcessPlan::new("cargo", ["a".to_owned(), "b".to_owned()]);
        assert_ne!(joined.digest_input(), split.digest_input());
    }

    #[test]
    fn the_digest_is_stable_across_calls() {
        let plan = ProcessPlan::new("cargo", ["miri".to_owned(), "test".to_owned()]);
        assert_eq!(plan.digest_input(), plan.digest_input());
    }

    #[test]
    fn a_timeout_is_not_a_success() {
        let out = ProcessOutput {
            exit_code: Some(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
            duration_ms: 1,
            timed_out: true,
        };
        // Exit code zero plus a timeout is a process we killed mid-write; it
        // tells us nothing about the code.
        assert!(!out.succeeded());
    }
}
