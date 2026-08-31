//! The `cargo vibe-check` shim.
//!
//! Cargo invokes `cargo-<name>` for an unknown subcommand and passes the
//! subcommand name back as `argv[1]`. Stripping it here means the argument
//! parser sees the same shape either way, so `cargo vibe-check plan` and
//! `vibe-check plan` are the same invocation rather than two code paths that
//! have to be kept in step.

use clap::Parser;
use eyre::Result;
use vibe_check::{Cli, panic};

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();
    // After `color_eyre::install`, for the reason given in `main.rs`.
    panic::install();

    let cli = Cli::parse_from(strip_cargo_subcommand(std::env::args()));
    std::process::exit(i32::from(panic::run_guarded(cli).await));
}

/// Drop the `vibe-check` cargo inserts as `argv[1]`.
///
/// Only when it is actually there: the binary is also runnable directly, and
/// removing the first argument unconditionally would eat a real one.
fn strip_cargo_subcommand(args: impl Iterator<Item = String>) -> Vec<String> {
    let mut args: Vec<String> = args.collect();
    if args.get(1).is_some_and(|a| a == "vibe-check") {
        args.remove(1);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(args: &[&str]) -> Vec<String> {
        strip_cargo_subcommand(args.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn removes_the_subcommand_cargo_inserts() {
        assert_eq!(
            strip(&["cargo-vibe-check", "vibe-check", "plan"]),
            ["cargo-vibe-check", "plan"]
        );
    }

    #[test]
    fn leaves_a_direct_invocation_alone() {
        assert_eq!(
            strip(&["cargo-vibe-check", "plan"]),
            ["cargo-vibe-check", "plan"]
        );
    }

    #[test]
    fn does_not_eat_a_real_argument_that_happens_to_match() {
        // `replay vibe-check` names a file; only position 1 is cargo's.
        assert_eq!(
            strip(&["cargo-vibe-check", "replay", "vibe-check"]),
            ["cargo-vibe-check", "replay", "vibe-check"]
        );
    }

    #[test]
    fn handles_no_arguments() {
        assert_eq!(strip(&["cargo-vibe-check"]), ["cargo-vibe-check"]);
    }
}
