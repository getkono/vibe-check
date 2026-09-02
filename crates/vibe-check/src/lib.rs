//! vibe-check: classify a pull request, source the evidence it needs, and
//! adjudicate a verdict.
//!
//! The binaries in this crate are thin wrappers over [`run`]; the behaviour
//! lives here so it can be tested without spawning a process.
//!
//! # Milestone status
//!
//! The command surface, the exit-code contract, the local scheduler, and the
//! registration seam are in place. The stages between them — diff
//! classification, policy resolution, evidence parsing, adjudication — arrive
//! with their own milestones, and until then the commands that depend on them
//! say so plainly and exit [`exit::FAILURE`].
//!
//! That is deliberate. Exiting `0` for "not implemented yet" would be
//! indistinguishable from "this change is fine", which is exactly the confusion
//! this tool exists to remove from other people's pipelines.
//!
//! A panic is held to the same rule. [`panic::run_guarded`] is what both
//! binaries actually call: it turns an unwind into [`exit::FAILURE`] and a
//! minimal bundle, so no panic *inside* [`run`] reaches a process exit code the
//! contract in [`exit`] does not describe. Two things sit outside that
//! guarantee and are documented on `run_guarded` rather than papered over here:
//! everything each binary does before entering the guard — installing the
//! `color_eyre` and panic hooks, initialising tracing, and parsing arguments,
//! the last of which is clap's `2`; and a process killed by a signal, which is
//! not an exit code at all.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod assembly;
pub mod cli;
pub mod exit;
pub mod panic;
pub mod scheduler;

use eyre::Result;

pub use cli::{Cli, Command};

/// Run a parsed command, returning the process exit code.
///
/// Returns `Ok` with a non-zero code for "we produced a verdict and it was not
/// `auto`", and `Err` for "we could not produce a verdict at all". Those are
/// genuinely different outcomes and the caller maps them to different exit
/// codes — collapsing them is how an outage starts looking like a pass.
///
/// # Errors
/// Returns an error when the requested command cannot be completed.
pub async fn run(cli: Cli) -> Result<u8> {
    // The deliberate-panic seam, read here because this is the deepest point
    // both binaries share, so a requested panic travels exactly the path a real
    // one would. Inert unless `VIBE_CHECK_PANIC` is set; see [`panic`].
    panic::panic_if_requested();

    // Before anything reads the repository. `dist/guard.sh` refuses this too,
    // and earlier — but the action is not the only caller: the reusable
    // workflow invokes the binary directly, and so does anyone running it by
    // hand. A refusal that lives only in the wrapper is a refusal the next
    // entry point does not have.
    refuse_pull_request_target(&ProcessEnv)?;

    let registrations = assembly::builtin();
    tracing::debug!(
        command = cli.command.name(),
        registry_digest = %registrations.digest(),
        "starting"
    );

    match &cli.command {
        Command::Classify
        | Command::Plan
        | Command::Run { .. }
        | Command::Adjudicate
        | Command::Replay { .. }
        | Command::Init
        | Command::Escape { .. }
        | Command::Schema { .. } => Err(not_yet_implemented(cli.command.name())),
    }
}

/// Where the process environment is read.
///
/// A trait rather than a direct `std::env::var` call so the refusal below is
/// testable without mutating the environment of a parallel test runner, which
/// `std::env::set_var` does and which is why it is `unsafe` in edition 2024.
pub trait Env {
    /// The value of `name`, if it is set.
    fn var(&self, name: &str) -> Option<String>;
}

/// The real environment.
pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// Refuse to run under `pull_request_target`.
///
/// That trigger checks out the base branch but runs with a **write** token and
/// access to repository secrets, while evaluating a fork's code — and
/// vibe-check reads the head tree. The `ForgeRead`/`ForgeWrite` split is
/// unenforceable in that position: withholding a capability means nothing when
/// the workflow around it holds one.
///
/// There is deliberately **no escape hatch** — no flag, no environment
/// variable, no policy key. An escape hatch on this is the vulnerability rather
/// than a convenience, and a configurable refusal is one a repository turns off
/// the first time it is inconvenient.
///
/// # Errors
/// Returns an error naming the trigger when `GITHUB_EVENT_NAME` is
/// `pull_request_target`.
fn refuse_pull_request_target(env: &dyn Env) -> Result<()> {
    let event = env.var("GITHUB_EVENT_NAME");
    if event.as_deref() == Some("pull_request_target") {
        return Err(eyre::eyre!(
            "vibe-check refuses to run on `pull_request_target`.\n\
             That trigger grants a write token and repository secrets to a \
             workflow evaluating fork-authored code, which makes the \
             ForgeRead/ForgeWrite split unenforceable. Use `on: pull_request`.\n\
             There is no option that permits this."
        ));
    }
    Ok(())
}

/// An error that says which milestone a command is waiting on.
///
/// Repository convention is that errors carry actionable context. "not
/// implemented" on its own leaves the reader wondering whether they have
/// misconfigured something.
fn not_yet_implemented(command: &str) -> eyre::Report {
    eyre::eyre!(
        "`vibe-check {command}` is not implemented in this build.\n\
         The command surface, exit-code contract, and registration seam are in place; \
         the classification, policy, and adjudication stages land in later milestones.\n\
         Exiting {} rather than 0, because \"not implemented\" must never be mistaken \
         for \"this change is fine\".",
        exit::FAILURE
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[tokio::test]
    async fn unimplemented_commands_fail_rather_than_reporting_success() {
        // The property that matters most in this milestone: there is no path
        // through `run` that exits 0 without having adjudicated anything.
        for args in [
            vec!["vibe-check", "classify"],
            vec!["vibe-check", "plan"],
            vec!["vibe-check", "adjudicate"],
            vec!["vibe-check", "init"],
            vec!["vibe-check", "run", "--id", "x"],
            vec!["vibe-check", "replay", "bundle.json"],
            vec!["vibe-check", "schema", "bundle"],
            vec!["vibe-check", "escape", "9f3c", "--category", "unsafe"],
        ] {
            let cli = Cli::try_parse_from(&args).expect("parses");
            let result = run(cli).await;
            assert!(result.is_err(), "{args:?} must not report success");
        }
    }

    /// An environment with exactly the variables a test names.
    struct FakeEnv<'a>(&'a [(&'a str, &'a str)]);

    impl Env for FakeEnv<'_> {
        fn var(&self, name: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn pull_request_target_is_refused_by_name() {
        let error =
            refuse_pull_request_target(&FakeEnv(&[("GITHUB_EVENT_NAME", "pull_request_target")]))
                .expect_err("the trigger is refused");
        let message = error.to_string();
        assert!(
            message.contains("pull_request_target"),
            "names the trigger: {message}"
        );
        assert!(
            message.contains("no option that permits this"),
            "says there is no escape hatch, because there is not: {message}"
        );
    }

    #[test]
    fn every_other_trigger_is_allowed() {
        // The floor. A refusal that fired on everything would satisfy the test
        // above and stop the tool working, so the accepting cases are asserted
        // alongside — `pull_request` especially, which is the one the error
        // message tells people to use.
        for event in ["pull_request", "push", "merge_group", "schedule", ""] {
            let bindings = [("GITHUB_EVENT_NAME", event)];
            let allowed = refuse_pull_request_target(&FakeEnv(&bindings)).is_ok();
            assert!(allowed, "`{event}` must be allowed");
        }
        assert!(
            refuse_pull_request_target(&FakeEnv(&[])).is_ok(),
            "an unset GITHUB_EVENT_NAME is the local case and must be allowed"
        );
    }

    #[tokio::test]
    async fn the_error_explains_itself() {
        let cli = Cli::try_parse_from(["vibe-check", "classify"]).expect("parses");
        let message = run(cli).await.expect_err("not implemented").to_string();
        assert!(message.contains("classify"), "names the command: {message}");
        assert!(message.contains("milestone"), "says why: {message}");
    }
}
