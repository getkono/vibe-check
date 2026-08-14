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

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod assembly;
pub mod cli;
pub mod exit;
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

    #[tokio::test]
    async fn the_error_explains_itself() {
        let cli = Cli::try_parse_from(["vibe-check", "classify"]).expect("parses");
        let message = run(cli).await.expect_err("not implemented").to_string();
        assert!(message.contains("classify"), "names the command: {message}");
        assert!(message.contains("milestone"), "says why: {message}");
    }
}
